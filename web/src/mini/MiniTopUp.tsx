import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { money } from '../lib/format';
import { t } from '../lib/i18n';
import { heldBalance } from '../cabinet/model';
import { bonusMark, bonusText, finished, limitsText, parseAmount, wireAmount, type PaymentProgress, type TopUpOptions } from '../cabinet/payments';
import { BackHead, useMini } from './MiniApp';
import { openExternal, webApp } from './telegram';
import { methodKey } from '../lib/methods';
import { ReceiptEmail, useReceiptEmail } from '../lib/receiptEmail';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniTopUp() {
  const { me, reload } = useMini();
  const receipt = useReceiptEmail(me);
  const held = heldBalance(me);
  const [options, setOptions] = useState<TopUpOptions | null>(null);
  const [loadError, setLoadError] = useState<{ code: string; message: string } | null>(null);
  const [preset, setPreset] = useState<number | null>(null);
  const [custom, setCustom] = useState('');
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [otherWallet, setOtherWallet] = useState<string | null>(null);
  const [watch, setWatch] = useState<string | null>(null);
  const off = me.balance_enabled === false;

  useEffect(() => {
    if (off) {
      return;
    }
    let alive = true;
    api<TopUpOptions>('/api/client/topup')
      .then((next) => {
        if (alive) {
          setOptions(next);
          setPreset(next.presets[Math.min(1, next.presets.length - 1)]?.minor ?? null);
        }
      })
      .catch((err) => {
        if (alive) {
          setLoadError(err instanceof ApiError ? { code: err.code, message: err.message } : { code: 'network', message: t('Сервер не ответил') });
        }
      });
    return () => {
      alive = false;
    };
  }, [off]);

  useEffect(() => {
    if (!watch) {
      return;
    }
    let alive = true;
    let timer = 0;
    const tick = async (attempt: number) => {
      try {
        const progress = await api<PaymentProgress>(`/api/client/payments/${watch}`);
        if (!alive) {
          return;
        }
        if (finished(progress)) {
          setWatch(null);
          setOtherWallet(null);
          if (progress.order_status === 'fulfilled') {
            await reload();
            setNote(t('Баланс пополнен.'));
          } else if (progress.order_status === 'review') {
            setNote(t('Оплата принята. Проверяем платёж, деньги поступят на баланс в ближайшее время.'));
          } else {
            setNote(null);
            setError(t('Оплата не прошла. Попробуйте ещё раз или выберите другой способ.'));
          }
          return;
        }
      } catch {
        if (!alive) {
          return;
        }
      }
      if (attempt < 75) {
        timer = window.setTimeout(() => void tick(attempt + 1), 4000);
      }
    };
    timer = window.setTimeout(() => void tick(0), 4000);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [watch, reload]);

  const currency = options?.currency ?? me.currency;
  const methods = options?.methods ?? [];
  const chosen = methods.find((m) => methodKey(m) === method) ?? methods[0] ?? null;
  const fixed = chosen?.amounts && chosen.amounts.length > 0 ? chosen.amounts : null;
  const typed = custom.trim() && !fixed ? parseAmount(custom, currency) : null;
  const amount = custom.trim() && !fixed ? typed : preset;
  const inRange = !!options && amount !== null && amount >= options.min.minor && amount <= options.max.minor;
  const amountError =
    options && custom.trim() && !fixed
      ? typed === null
        ? currency === 'RUB'
          ? t('Сумма — целое число рублей, например 700')
          : t('Сумма — число, например 15 или 12.50')
        : !inRange
          ? t('Сумма должна быть {limits}', { limits: limitsText(options) })
          : null
      : null;
  const inTelegramWallet = chosen?.provider === 'gram' && webApp() !== null;
  const sum = amount !== null && inRange ? money({ minor: amount, currency }) : null;
  const bonus = amount !== null && inRange ? bonusText(amount, currency, chosen) : null;

  useEffect(() => {
    if (!fixed || (preset !== null && fixed.some((m) => m.minor === preset) && !custom.trim())) {
      return;
    }
    const wanted = (custom.trim() ? parseAmount(custom, currency) : preset) ?? 0;
    const next = fixed.find((m) => m.minor >= wanted) ?? fixed[fixed.length - 1];
    setPreset(next?.minor ?? null);
    setCustom('');
  }, [fixed, preset, custom, currency]);

  const pay = async () => {
    if (!options || amount === null || !inRange || !chosen) {
      return;
    }
    const email = receipt.take(chosen);
    if (email === false) {
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    setOtherWallet(null);
    try {
      const res = await api<{ type: 'redirect'; payment_id: string; pay_url: string } | { type: 'completed'; order_id: string }>('/api/client/topup', {
        method: 'POST',
        body: { email: email ?? undefined, amount: wireAmount(amount, currency), provider: chosen.provider, method: chosen.method ?? undefined },
      });
      if (res.type !== 'redirect') {
        await reload();
        setNote(t('Баланс пополнен.'));
        return;
      }
      setWatch(res.payment_id);
      if (inTelegramWallet) {
        try {
          const sent = await api<{ chat_url: string }>(`/api/client/payments/${res.payment_id}/telegram`, { method: 'POST' });
          setOtherWallet(res.pay_url);
          setNote(t('Счёт отправлен в чат с ботом. Нажмите там «Оплатить в Telegram» — кошелёк откроется с готовой суммой.'));
          openExternal(sent.chat_url);
          return;
        } catch (err) {
          setError(failText(err, t('Счёт в чат не отправился, открываем другой кошелёк')));
        }
      }
      openExternal(res.pay_url);
      setNote(t('Оплата открылась в новом окне. Деньги поступят на баланс сами, как только платёжная система подтвердит оплату.'));
    } catch (err) {
      if (!receipt.refused(err)) {
        setError(failText(err, t('Оплата не началась')));
      }
    } finally {
      setBusy(false);
    }
  };

  if (off || loadError?.code === 'balance.disabled') {
    return (
      <main className="mn-page">
        <BackHead title={t('Пополнение')} />
        <div className="mn-card mn-pad">
          <strong>{t('Пополнение выключено')}</strong>
          <span className="mn-ink2">{t('Пополнить баланс сейчас нельзя. Оплатите подписку напрямую одним из способов оплаты.')}</span>
          <Link to="/app/plans" className="mn-main sm press">
            {t('К тарифам')}
          </Link>
        </div>
      </main>
    );
  }

  return (
    <main className="mn-page topup">
      <BackHead title={t('Пополнение')} />
      <p className="mn-notes">
        {t('Сейчас на балансе {amount}. С баланса можно оплатить подписку и продление, если денег хватает на всю сумму.', { amount: money(me.balance) })}
      </p>
      {held ? <p className="mn-notes">{t('Удержано до ответа платёжной системы: {amount}', { amount: money(held) })}</p> : null}
      {!options ? <p className="mn-muted">{loadError?.message ?? t('Загружаем…')}</p> : null}
      {options && (fixed ?? options.presets).length > 0 ? (
        <div role="radiogroup" aria-label={t('Сумма пополнения')} className="mn-amounts">
          {(fixed ?? options.presets).map((p) => (
            <button
              key={p.minor}
              type="button"
              role="radio"
              aria-checked={!custom.trim() && preset === p.minor}
              className="mn-amount press num"
              onClick={() => {
                setPreset(p.minor);
                setCustom('');
              }}
            >
              {money(p)}
            </button>
          ))}
        </div>
      ) : null}
      {options && fixed && chosen ? <p className="mn-muted small">{t('Через {method} пополнение идёт только этими суммами', { method: chosen.title })}</p> : null}
      {options && !fixed ? (
        <div className="mn-field">
          <input
            value={custom}
            onChange={(e) => setCustom(e.target.value)}
            inputMode={currency === 'RUB' ? 'numeric' : 'decimal'}
            placeholder={options.presets.length > 0 ? t('Другая сумма') : t('Сумма пополнения')}
            aria-label={options.presets.length > 0 ? t('Другая сумма') : t('Сумма пополнения')}
            aria-invalid={amountError ? true : undefined}
            aria-describedby="mini-topup-hint"
            maxLength={12}
          />
          <span id="mini-topup-hint" className={amountError ? 'mn-error' : 'mn-muted small'}>
            {amountError ?? t('Можно {limits}', { limits: limitsText(options) })}
          </span>
        </div>
      ) : null}
      {options ? (
        <div className="mn-pay">
          <div className="mn-pay-head">
            <h2>{t('Оплата')}</h2>
          </div>
          {methods.length === 0 ? (
            <p className="mn-muted small">
              {t('В мини-приложении пополнить сейчас нельзя.')}{' '}
              <button type="button" className="mn-link press" onClick={() => openExternal(`${window.location.origin}/cabinet/topup`)}>
                {t('Пополнить в кабинете на сайте')}
              </button>
            </p>
          ) : null}
          <div role="radiogroup" aria-label={t('Способ оплаты')} className="mn-chips">
            {methods.map((m) => (
              <button key={methodKey(m)} type="button" role="radio" aria-checked={chosen === m} className="mn-chip press" onClick={() => setMethod(methodKey(m))}>
                {m.title}
                {bonusMark(m, inRange ? amount : null) ? ` ${bonusMark(m, inRange ? amount : null)}` : null}
              </button>
            ))}
          </div>
          {bonus ? <p className="mn-note">{bonus}</p> : null}
          {error ? <p className="mn-error">{error}</p> : null}
          {note ? (
            <p role="status" className="mn-note">
              {note}
              {otherWallet ? (
                <>
                  {' '}
                  <button type="button" className="mn-link press" onClick={() => openExternal(otherWallet)}>
                    {t('Оплатить другим кошельком')}
                  </button>
                </>
              ) : null}
            </p>
          ) : null}
        </div>
      ) : null}
      {receipt.asks(chosen) ? <ReceiptEmail value={receipt.email} onChange={receipt.change} error={receipt.error} disabled={busy} mini /> : null}
      {options ? (
        <button type="button" className="mn-main mn-paybtn press" onClick={() => void pay()} disabled={busy || !sum || !chosen}>
          {busy ? t('Открываем оплату…') : sum ? (inTelegramWallet ? t('Пополнить на {amount} в Telegram', { amount: sum }) : t('Пополнить на {amount}', { amount: sum })) : t('Пополнить')}
        </button>
      ) : null}
    </main>
  );
}
