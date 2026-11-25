import { useEffect, useId, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { bonusTaken } from '../../lib/bonus';
import { days, moneyExact, type Money } from '../../lib/format';
import { amountText } from './history';
import { Dialog, parseMinor } from './dialogs';
import type { Amount } from './types';

type Way = { available: boolean; note: string | null };

type Options = {
  payment_id: number;
  status: string;
  provider: string;
  provider_title: string;
  amount: Amount;
  refunded: Amount;
  waiting: Amount;
  remaining: Amount;
  topup: boolean;
  subscription: boolean;
  days: number;
  note: string | null;
  to_balance: Way;
  via_provider: Way;
  outside: Way;
  bonus?: { left: Money };
  gift: { client_id: number; subscription_id: number | null } | null;
};

export type RefundResult = { refund_id: number | null; request_id: number | null; pending: boolean; amount: Amount; subscription_error: string | null };

type Target = 'balance' | 'provider' | 'outside';
type Action = 'keep' | 'cut' | 'disable';

type Props = { paymentId: number; onClose: () => void; onDone: (result: RefundResult, text: string) => void };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function units(amount: Amount): string {
  const whole = Math.trunc(amount.minor / 100);
  const cents = amount.minor % 100;
  return amount.currency === 'XTR' ? String(amount.minor) : cents ? `${whole}.${String(cents).padStart(2, '0')}` : String(whole);
}

function takenBonus(options: Options, minor: number): Money | null {
  const { bonus, remaining } = options;
  if (!bonus) {
    return null;
  }
  const taken = minor <= remaining.minor ? bonusTaken(bonus.left.minor, remaining.minor, minor) : 0;
  return taken > 0 ? { minor: taken, currency: bonus.left.currency } : null;
}

function newKey(): string {
  if (typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  const bytes = crypto.getRandomValues(new Uint8Array(16));
  bytes[6] = ((bytes[6] ?? 0) & 0x0f) | 0x40;
  bytes[8] = ((bytes[8] ?? 0) & 0x3f) | 0x80;
  const hex = Array.from(bytes, (b) => b.toString(16).padStart(2, '0')).join('');
  return `${hex.slice(0, 8)}-${hex.slice(8, 12)}-${hex.slice(12, 16)}-${hex.slice(16, 20)}-${hex.slice(20)}`;
}

export function RefundDialog({ paymentId, onClose, onDone }: Props) {
  const [options, setOptions] = useState<Options | null>(null);
  const [target, setTarget] = useState<Target>('balance');
  const [action, setAction] = useState<Action>('keep');
  const [sum, setSum] = useState('');
  const [reason, setReason] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const key = useMemo(newKey, []);
  const sumId = useId();
  const reasonId = useId();

  useEffect(() => {
    let cancelled = false;
    api<Options>(`/api/admin/payments/${paymentId}/refund`)
      .then((data) => {
        if (cancelled) {
          return;
        }
        setOptions(data);
        setSum(units(data.remaining));
        if (data.gift) {
          setAction('disable');
        }
        setTarget(data.via_provider.available ? 'provider' : data.to_balance.available ? 'balance' : 'outside');
      })
      .catch((err) => {
        if (!cancelled) {
          setError(failText(err, 'Данные платежа не загрузились'));
        }
      });
    return () => {
      cancelled = true;
    };
  }, [paymentId]);

  const stars = options?.amount.currency === 'XTR';
  const minor = stars ? (/^\d+$/.test(sum.trim()) ? Number(sum.trim()) : null) : parseMinor(sum);
  const tooMuch = !!options && minor !== null && minor > options.remaining.minor;
  const ways: Array<[Target, string, Way]> = options
    ? [
        ['provider', `Через ${options.provider_title}`, options.via_provider],
        ['balance', 'На баланс', options.to_balance],
        ['outside', 'Уже вернули вне панели', options.outside],
      ]
    : [];
  const open = ways.filter(([, , way]) => way.available);
  const closedNotes = ways.filter(([, , way]) => !way.available && way.note).map(([, , way]) => way.note as string);
  const bonus = options && minor !== null ? takenBonus(options, minor) : null;
  const shown = minor !== null && options ? amountText({ minor, currency: options.amount.currency }) : '';

  const explain: Record<Target, string> = {
    provider: 'Деньги вернутся на карту или счёт, с которого клиент платил. Платёжная система может подтверждать возврат до нескольких дней.',
    balance: 'Сумма сразу появится на балансе клиента в панели, её можно потратить на подписку.',
    outside: 'Панель только отметит возврат: деньги вы уже вернули в кабинете платёжной системы или вручную.',
  };
  const gift = options?.gift ?? null;
  const actions: Array<[Action, string, string]> = gift
    ? [
        ['keep', 'Оставить', 'Подписка получателя продолжит работать, хотя деньги за подарок вернутся.'],
        ['cut', 'Укоротить', options ? `Срок подписки получателя уменьшится на ${days(options.days)} — столько дал подарок.` : ''],
        ['disable', 'Отключить', 'Подписка получателя перестанет работать, он не сможет подключиться.'],
      ]
    : [
        ['keep', 'Оставить', 'Подписка продолжит работать как сейчас.'],
        ['cut', 'Снять дни', options ? `Срок подписки уменьшится на ${days(options.days)} — столько добавил этот заказ.` : ''],
        ['disable', 'Отключить', 'Подписка перестанет работать, клиент не сможет подключиться.'],
      ];

  const submit = async () => {
    if (!options || minor === null || minor <= 0 || tooMuch) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const body: Record<string, unknown> = {
        amount_minor: minor,
        method: target === 'balance' ? 'balance' : 'provider',
        outside: target === 'outside',
        subscription_action: options.subscription ? action : 'keep',
        key,
      };
      if (reason.trim()) {
        body.reason = reason.trim();
      }
      const result = await api<{ refund: RefundResult }>(`/api/admin/payments/${paymentId}/refund`, { method: 'POST', body });
      const refund = result.refund;
      const text = refund.pending
        ? `Возврат ${amountText(refund.amount)} запрошен: ждём подтверждения ${options.provider_title}`
        : target === 'outside'
          ? `Возврат ${amountText(refund.amount)} отмечен как сделанный вне панели`
          : target === 'balance'
            ? `${amountText(refund.amount)} вернули на баланс клиента`
            : `Возврат ${amountText(refund.amount)} оформлен`;
      onDone(refund, refund.subscription_error ? `${text}. Подписку изменить не удалось: ${refund.subscription_error}` : text);
    } catch (err) {
      setError(failText(err, 'Возврат не оформился'));
      setBusy(false);
    }
  };

  return (
    <Dialog title="Вернуть деньги" busy={busy} onClose={onClose} wide>
      {!options && !error ? <p className="muted">Загружаем платёж…</p> : null}
      {options ? (
        <p className="muted">
          {amountText(options.amount)}, {options.provider_title}
          {options.refunded.minor > 0 ? `. Уже вернули ${amountText(options.refunded)}` : ''}
          {options.waiting.minor > 0 ? `. Ждёт подтверждения ${amountText(options.waiting)}` : ''}
        </p>
      ) : null}
      {options?.note ? <p>{options.note}</p> : null}
      {options && !options.note && open.length === 0 ? <p>{closedNotes[0] ?? 'Этот платёж вернуть нельзя'}</p> : null}
      {options && !options.note && open.length > 0 ? (
        <>
          <div className="field-row">
            <label htmlFor={sumId}>Сумма, {stars ? 'звёзд' : options.amount.currency === 'RUB' ? '₽' : '$'}</label>
            <input
              id={sumId}
              className="field num"
              inputMode="decimal"
              value={sum}
              onChange={(event) => setSum(event.target.value)}
              disabled={busy}
              autoFocus
            />
            <span className={tooMuch ? 'err' : 'cap'}>
              {tooMuch ? `Больше, чем можно вернуть: до ${amountText(options.remaining)}` : `Можно вернуть до ${amountText(options.remaining)}`}
            </span>
          </div>
          <div className="field-row">
            <span style={{ fontWeight: 600 }}>Куда вернуть</span>
            <div role="group" aria-label="Куда вернуть" className="segs" style={{ alignSelf: 'flex-start' }}>
              {open.map(([value, label]) => (
                <button key={value} type="button" aria-pressed={target === value} onClick={() => setTarget(value)} disabled={busy}>
                  {label}
                </button>
              ))}
            </div>
            <span className="cap">{explain[target]}</span>
            {options.topup && target !== 'balance' ? (
              <span className="cap">
                Эта сумма спишется с баланса клиента: пополнение отменяется.{bonus ? ` Вместе с ней снимется бонус ${moneyExact(bonus)}.` : ''}
              </span>
            ) : null}
            {closedNotes.map((note) => (
              <span key={note} className="cap">
                {note}
              </span>
            ))}
          </div>
          {options.subscription ? (
            <div className="field-row">
              <span style={{ fontWeight: 600 }}>{gift ? 'Подписка получателя подарка' : 'Подписка'}</span>
              {gift ? (
                <span className="warn-line" role="note">
                  Подарок уже активировал{' '}
                  <a href={`/admin/clients/${gift.client_id}`} target="_blank" rel="noreferrer">
                    клиент №{gift.client_id}
                  </a>
                  . Если подписку оставить, он продолжит пользоваться ею после возврата денег.
                </span>
              ) : null}
              <div role="group" aria-label="Что сделать с подпиской" className="segs" style={{ alignSelf: 'flex-start' }}>
                {actions.map(([value, label]) => (
                  <button key={value} type="button" aria-pressed={action === value} onClick={() => setAction(value)} disabled={busy}>
                    {label}
                  </button>
                ))}
              </div>
              <span className="cap">{actions.find(([value]) => value === action)?.[2]}</span>
            </div>
          ) : null}
          <div className="field-row">
            <label htmlFor={reasonId}>Причина</label>
            <input
              id={reasonId}
              className="field"
              maxLength={300}
              value={reason}
              onChange={(event) => setReason(event.target.value)}
              placeholder="Например: не подошёл тариф"
              disabled={busy}
            />
            <span className="cap">Увидят администраторы в истории клиента</span>
          </div>
        </>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        {options && !options.note && open.length > 0 ? (
          <button type="button" className="btn-bad" onClick={() => void submit()} disabled={busy || minor === null || minor <= 0 || tooMuch}>
            {busy ? 'Возвращаем…' : target === 'outside' ? `Отметить возврат ${shown}` : `Вернуть ${shown}`}
          </button>
        ) : null}
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          {options && !options.note && open.length > 0 ? 'Отменить' : 'Понятно'}
        </button>
      </div>
    </Dialog>
  );
}
