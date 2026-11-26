import { useCallback, useEffect, useState } from 'react';
import { Link, Navigate, Route, Routes, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { dateLong, NBSP } from '../../lib/format';
import { usePhone } from '../../lib/media';
import { Avatar } from '../Avatar';
import { ConfirmDialog } from '../client/dialogs';
import { amountText, lowerFirst, methodText } from '../client/history';
import { RefundDialog } from '../client/RefundDialog';
import { agoLong, orderLine, personName, type PaymentItem } from '../home/model';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import { Decisions } from './Decisions';
import { MoneyTabs } from './MoneyTabs';
import { WebhookCard } from './WebhookCard';
import { Webhooks } from './Webhooks';
import '../promo/promo.css';
import './money.css';

type Filter = 'pending' | 'canceled' | 'paid' | 'all';

const FILTERS: Array<[Filter, string]> = [
  ['pending', 'Ждут оплаты'],
  ['canceled', 'Истекли и отменены'],
  ['paid', 'Оплачены'],
  ['all', 'Все'],
];

const MANUAL = new Set(['pending', 'canceled', 'failed']);
const AUTORENEW_METHOD = 'autorenew';
const NOT_MANUAL = new Set(['stars', 'balance']);
const REFUNDABLE = new Set(['paid', 'partially_refunded']);

type PageData = { items: PaymentItem[]; next: number | null };

function stateOf(p: PaymentItem): { text: string; color: string } {
  switch (p.status) {
    case 'paid':
    case 'partially_refunded':
      return { text: `оплачен ${lowerFirst(agoLong(p.paid_at ?? p.created_at))}`, color: 'var(--muted)' };
    case 'pending':
      return p.order?.status === 'review'
        ? { text: 'задержан проверкой', color: 'var(--warn)' }
        : { text: 'ждём оплату', color: 'var(--warn)' };
    case 'canceled':
      return { text: 'истёк или отменён', color: 'var(--muted)' };
    case 'refunded':
      return { text: 'возвращён', color: 'var(--muted)' };
    default:
      return { text: 'не прошёл', color: 'var(--bad)' };
  }
}

function closesText(p: PaymentItem, now = Date.now()): string | null {
  if (p.status !== 'pending' || p.order?.status === 'review' || !p.expires_at) {
    return null;
  }
  const at = new Date(p.expires_at).getTime();
  if (Number.isNaN(at)) {
    return null;
  }
  const minutes = Math.ceil((at - now) / 60_000);
  if (minutes <= 0) {
    return `срок вышел ${agoLong(p.expires_at)}`;
  }
  if (minutes < 60) {
    return `закроется через ${minutes}${NBSP}мин`;
  }
  const hours = Math.floor(minutes / 60);
  return hours < 48 ? `закроется через ${hours}${NBSP}ч` : `закроется ${dateLong(p.expires_at)}`;
}

function cancelable(p: PaymentItem): boolean {
  return p.status === 'pending' && p.order?.status !== 'review' && p.method !== AUTORENEW_METHOD;
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function Money() {
  return (
    <Routes>
      <Route index element={<Payments />} />
      <Route path="webhooks" element={<Webhooks />} />
      <Route path="webhooks/:id" element={<WebhookCard />} />
      <Route path="*" element={<Navigate to="/admin/money" replace />} />
    </Routes>
  );
}

function onePayment(params: URLSearchParams): number | null {
  const raw = params.get('payment');
  const id = raw && /^\d{1,15}$/.test(raw) ? Number(raw) : null;
  return id && id > 0 ? id : null;
}

function Payments() {
  const { can } = useSession();
  const phone = usePhone();
  const [params, setParams] = useSearchParams();
  const single = onePayment(params);
  const refunds = can('refunds');
  const [refund, setRefund] = useState<PaymentItem | null>(null);
  const [filter, setFilter] = useState<Filter>('pending');
  const [items, setItems] = useState<PaymentItem[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [confirm, setConfirm] = useState<PaymentItem | null>(null);
  const [cancel, setCancel] = useState<PaymentItem | null>(null);
  const [done, setDone] = useState<string | null>(null);

  const query = useCallback(
    (after: number | null) => {
      const query = new URLSearchParams({ limit: '30' });
      if (single !== null) {
        query.set('id', String(single));
      } else if (filter !== 'all') {
        query.set('status', filter);
      }
      if (after !== null) {
        query.set('after', String(after));
      }
      return api<PageData>(`/api/admin/payments?${query.toString()}`);
    },
    [filter, single],
  );

  const load = useCallback(async () => {
    setError(null);
    try {
      const page = await query(null);
      setItems(page.items);
      setNext(page.next);
    } catch (err) {
      setItems([]);
      setError(failText(err, 'Платежи не загрузились'));
    }
  }, [query]);

  useEffect(() => {
    setItems(null);
    setDone(null);
    void load();
  }, [load]);

  const reload = useCallback(() => void load(), [load]);

  const more = async () => {
    if (next === null) {
      return;
    }
    setBusy(true);
    try {
      const page = await query(next);
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Платежи не загрузились'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="pr-page">
      {phone ? (
        <PhoneHeader title="Деньги" />
      ) : (
        <div className="pr-head">
          <h1 className="page-title tight">Деньги</h1>
        </div>
      )}
      <MoneyTabs />
      <Decisions onChanged={reload} />
      <section className="card mn-money" aria-labelledby="mo-h">
        <div className="mo-head">
          <h2 id="mo-h" className="section-title">
            {single !== null ? `Платёж №${single}` : 'Платежи'}
          </h2>
          {single !== null ? (
            <button type="button" className="btn-line hov" onClick={() => setParams(new URLSearchParams(), { replace: true })}>
              Все платежи
            </button>
          ) : (
            <div role="group" aria-label="Какие платежи показать" className="segs">
              {FILTERS.map(([key, label]) => (
                <button key={key} type="button" aria-pressed={filter === key} onClick={() => setFilter(key)}>
                  {label}
                </button>
              ))}
            </div>
          )}
        </div>
        {done ? (
          <p className="mo-done" role="status">
            {done}
          </p>
        ) : null}
        {error ? (
          <p className="err mo-pad" role="alert">
            {error}
          </p>
        ) : null}
        {items === null ? <p className="muted mo-pad">Загружаем платежи…</p> : null}
        {items !== null && items.length === 0 && !error ? (
          <p className="muted mo-pad">{single !== null ? 'Такого платежа нет' : 'Здесь пока пусто'}</p>
        ) : null}
        {items?.map((p) => {
          const name = personName(p.client, p.client_id);
          const state = stateOf(p);
          const manual = MANUAL.has(p.status) && !NOT_MANUAL.has(p.provider) && p.amount.currency !== 'XTR';
          const returnable = refunds && REFUNDABLE.has(p.status);
          const closes = closesText(p);
          const canCancel = cancelable(p);
          const several = [manual, canCancel, returnable].filter(Boolean).length > 1;
          return (
            <div key={p.id} className="mo-row">
              <Link to={`/admin/clients/${p.client_id}`} className="mo-who hov">
                <Avatar id={p.client_id} name={name} className="mo-ava" />
                <span className="mo-text">
                  <span className="mo-name">{name}</span>
                  <span className="muted">
                    {orderLine(p.order)} · {lowerFirst(methodText(p.provider, p.method))}
                  </span>
                </span>
              </Link>
              <span className="mo-sum">
                <strong className="num">{amountText(p.amount)}</strong>
                <span style={{ color: state.color }}>{state.text}</span>
                {closes ? <span className="muted">{closes}</span> : null}
              </span>
              <span className={several ? 'mo-act mo-acts' : 'mo-act'}>
                {manual ? (
                  <button type="button" className="btn-line hov" onClick={() => setConfirm(p)}>
                    Отметить оплаченным
                  </button>
                ) : null}
                {canCancel ? (
                  <button type="button" className="btn-line hov" onClick={() => setCancel(p)}>
                    Отменить
                  </button>
                ) : null}
                {returnable ? (
                  <button type="button" className="btn-line hov" onClick={() => setRefund(p)}>
                    Вернуть
                  </button>
                ) : null}
              </span>
              {p.status === 'failed' && p.failure_reason ? <span className="mo-reason muted">{p.failure_reason}</span> : null}
            </div>
          );
        })}
        {next !== null ? (
          <div className="mo-pad">
            <button type="button" className="btn-line hov" onClick={() => void more()} disabled={busy}>
              {busy ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>
      {refund ? (
        <RefundDialog
          paymentId={refund.id}
          onClose={() => setRefund(null)}
          onDone={(_, text) => {
            setRefund(null);
            setDone(`${personName(refund.client, refund.client_id)}: ${lowerFirst(text)}`);
            void load();
          }}
        />
      ) : null}
      {confirm ? (
        <ConfirmDialog
          title="Отметить платёж оплаченным?"
          text={`${personName(confirm.client, confirm.client_id)}, ${amountText(confirm.amount)}, ${lowerFirst(methodText(confirm.provider, confirm.method))}. Заказ выполнится так же, как после обычной оплаты: клиент получит подписку и сообщение. Отмечайте, только если деньги действительно пришли.`}
          confirm="Отметить оплаченным"
          run={async () => {
            await api(`/api/admin/payments/${confirm.id}/mark-paid`, {
              method: 'POST',
              body: { comment: 'Отмечено вручную в разделе «Деньги»' },
            });
            setDone(`Платёж клиента ${personName(confirm.client, confirm.client_id)} отмечен оплаченным`);
            setConfirm(null);
            await load();
          }}
          onClose={() => setConfirm(null)}
        />
      ) : null}
      {cancel ? (
        <ConfirmDialog
          title="Отменить неоплаченный счёт?"
          text={`${personName(cancel.client, cancel.client_id)}, ${amountText(cancel.amount)}, ${lowerFirst(methodText(cancel.provider, cancel.method))}. Счёт уйдёт в «Истекли и отменены», а заказ закроется, если других счетов по нему нет. Если клиент всё же оплатит этот счёт, оплата засчитается как обычно.`}
          confirm="Отменить счёт"
          dismiss="Не отменять"
          run={async () => {
            await api(`/api/admin/payments/${cancel.id}/cancel`, {
              method: 'POST',
              body: { comment: 'Отменено вручную в разделе «Деньги»' },
            });
            setDone(`Счёт клиента ${personName(cancel.client, cancel.client_id)} отменён`);
            setCancel(null);
            await load();
          }}
          onClose={() => setCancel(null)}
        />
      ) : null}
    </div>
  );
}
