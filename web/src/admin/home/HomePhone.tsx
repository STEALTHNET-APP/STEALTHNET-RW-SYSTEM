import { useEffect, useRef, useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { api } from '../../lib/api';
import { count, dateLong, groupDigits, money } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { amountText, lowerFirst, methodText } from '../client/history';
import { PhoneHeader } from '../phone';
import { ATTENTION_LINK, attentionCount, attentionNote, attentionTitle } from '../money/journal';
import { useSession } from '../session';
import { useOverview } from './Home';
import { PanelServer } from './PanelServer';
import { agoLong, minutesSince, orderLine, personName, spanText, type PaymentItem, type TicketItem } from './model';
import { setupTodos } from './setup';
import { moneyTodos, refundTodo, type Todo, type Withdrawal } from './todo';
import './phone.css';
import { Avatar } from '../Avatar';
import { ImportHint } from './ImportHint';
import { ConfigWarnings } from './ConfigWarnings';
import { PhoneWeek } from './WeekLedger';

function longSpan(minutes: number): string {
  return minutes < 60 ? count(Math.max(1, minutes), 'минуту', 'минуты', 'минут') : spanText(minutes);
}

export function HomePhone() {
  const { can, me } = useSession();
  const navigate = useNavigate();
  const { dashboard, nodes, restart, restarting, billing, canRestart } = useOverview();
  const [tickets, setTickets] = useState<TicketItem[] | null>(null);
  const [waiting, setWaiting] = useState(0);
  const [payments, setPayments] = useState<PaymentItem[] | null>(null);
  const [withdrawals, setWithdrawals] = useState<Withdrawal[]>([]);
  const [query, setQuery] = useState('');
  const searchRef = useRef<HTMLInputElement>(null);
  const referral = me.menu.some((m) => m.module === 'referral');

  useEffect(() => {
    if (can('support')) {
      api<{ items: TicketItem[]; counts: { waiting: number } }>('/api/admin/support/tickets?filter=waiting&order=longest&limit=1')
        .then((page) => {
          setTickets(page.items);
          setWaiting(page.counts.waiting);
        })
        .catch(() => setTickets([]));
    }
    if (can('money')) {
      api<{ items: PaymentItem[] }>('/api/admin/payments?limit=3')
        .then((page) => setPayments(page.items))
        .catch(() => setPayments([]));
      if (referral) {
        api<{ items: Withdrawal[] }>('/api/admin/m/referral/withdrawals?status=pending&limit=100')
          .then((page) => setWithdrawals(page.items))
          .catch(() => setWithdrawals([]));
      }
    }
  }, [can, referral]);

  const search = (event: FormEvent) => {
    event.preventDefault();
    const q = query.trim();
    navigate(q ? `/admin/clients?q=${encodeURIComponent(q)}` : '/admin/clients');
  };

  const focusSearch = () => {
    window.scrollTo({ top: 0 });
    searchRef.current?.focus({ preventScroll: true });
  };

  const revenue = dashboard?.revenue ?? null;
  const today = dashboard?.today ?? null;
  const subs = dashboard?.subscriptions ?? null;
  const online = dashboard?.remna?.online_now ?? null;
  const change =
    revenue && revenue.yesterday.minor > 0 ? Math.round(((revenue.today.minor - revenue.yesterday.minor) / revenue.yesterday.minor) * 100) : null;

  const todos: Todo[] = setupTodos(dashboard?.setup, can);
  for (const node of nodes) {
    if (node.state !== 'down' && !(node.state === 'connecting' && restarting.has(node.uuid))) {
      continue;
    }
    const minutes = minutesSince(node.down_since ?? node.last_status_change);
    const busy = restarting.has(node.uuid);
    todos.push({
      key: `node-${node.uuid}`,
      title: busy ? `Нода ${node.name} подключается` : `Нода ${node.name} не отвечает`,
      note: minutes === null ? '' : busy ? `Не отвечала ${longSpan(minutes)}` : longSpan(minutes),
      tone: busy ? 'muted' : 'bad',
      action: busy ? 'Перезапускаем…' : 'Перезапустить',
      label: busy ? `Перезапускаем ноду ${node.name}` : `Перезапустить ноду ${node.name}`,
      run: canRestart && !busy ? () => void restart(node.uuid) : undefined,
      busy: busy || !canRestart,
    });
  }
  const refund = refundTodo(dashboard?.decisions?.refund_actions);
  if (refund) {
    todos.push(refund);
  }
  todos.push(...moneyTodos(dashboard?.decisions));
  for (const bill of billing) {
    const nodeName = bill.node?.name ?? bill.name ?? 'сервер';
    todos.push({
      key: `bill-${bill.uuid}`,
      title: `Оплатить хостинг ${nodeName}`,
      note: `${bill.provider.name} · до ${dateLong(bill.nextBillingAt)}`,
      tone: 'warn',
      action: 'Открыть',
      label: `Открыть ${bill.provider.name}, чтобы оплатить ${nodeName}`,
      href: bill.provider.loginUrl ?? undefined,
      to: bill.provider.loginUrl ? undefined : '/admin/infra/billing',
    });
  }
  const attention = dashboard?.webhooks ?? null;
  if (attention && attentionCount(attention) > 0) {
    todos.push({
      key: 'webhooks',
      title: attentionTitle(attention),
      note: attentionNote(attention),
      tone: 'bad',
      action: 'Открыть',
      label: 'Открыть журнал уведомлений платёжек',
      to: ATTENTION_LINK,
    });
  }
  if (waiting > 0) {
    const oldest = (tickets ?? []).map((t) => minutesSince(t.waiting_since)).filter((m): m is number => m !== null);
    const longest = oldest.length ? Math.max(...oldest) : null;
    todos.push({
      key: 'support',
      title: `${count(waiting, 'обращение', 'обращения', 'обращений')} без ответа`,
      note: longest === null ? '' : `Самое старое ждёт ${spanText(longest)}`,
      tone: 'muted',
      action: 'Ответить',
      label: 'Ответить на обращения',
      to: '/admin/support',
    });
  }
  if (withdrawals.length > 0) {
    const first = withdrawals[0];
    const total = first ? { minor: withdrawals.reduce((sum, w) => sum + w.amount.minor, 0), currency: first.amount.currency } : null;
    todos.push({
      key: 'withdrawals',
      title: `${count(withdrawals.length, 'заявка', 'заявки', 'заявок')} на вывод`,
      note: total ? `Всего ${money(total)}` : '',
      tone: 'muted',
      action: 'Рассмотреть',
      label: 'Рассмотреть заявки на вывод',
      to: '/admin/m/referral',
    });
  }

  return (
    <div className="hp">
      <PhoneHeader title="Главная">
        <button type="button" className="ph-circ press" aria-label="Поиск клиента" onClick={focusSearch}>
          <span className="ph-dot">
            <Icon name="search" size={19} />
          </span>
        </button>
      </PhoneHeader>

      <ConfigWarnings compact />
      <ImportHint compact />

      <form role="search" onSubmit={search}>
        <label className="hp-search">
          <Icon name="search" size={18} />
          <span className="sr-only">Найти клиента</span>
          <input ref={searchRef} type="search" value={query} onChange={(e) => setQuery(e.target.value)} placeholder="Найти клиента: имя, @username, ID" enterKeyHint="search" />
        </label>
      </form>

      <section aria-label="Сегодня" className="hp-card hp-today">
        {revenue ? (
          <>
            <span className="hp-cap">Выручка сегодня</span>
            <strong className="hp-big">{money(revenue.today)}</strong>
            <span className="hp-cap" style={{ marginTop: 4 }}>
              {change !== null ? (
                <>
                  <span style={{ color: change >= 0 ? 'var(--ok)' : 'var(--bad)' }}>
                    {change >= 0 ? '+' : '−'}
                    {Math.abs(change)}%
                  </span>{' '}
                  к вчера ·{' '}
                </>
              ) : null}
              за месяц {money(revenue.month, true)}
            </span>
          </>
        ) : (
          <span className="hp-cap">Сегодня</span>
        )}
        <dl className={revenue ? 'hp-stats lined' : 'hp-stats'}>
          {today ? (
            <div>
              <dt>Новые клиенты</dt>
              <dd className="num">{groupDigits(today.new_clients)}</dd>
            </div>
          ) : null}
          {online !== null ? (
            <div>
              <dt>Онлайн</dt>
              <dd className="num">{groupDigits(online)}</dd>
            </div>
          ) : null}
          {subs ? (
            <div>
              <dt>Активные подписки</dt>
              <dd className="num">{groupDigits(subs.active_paid)}</dd>
            </div>
          ) : null}
        </dl>
      </section>

      {dashboard?.days?.length ? <PhoneWeek days={dashboard.days} before={revenue?.week_before} /> : null}

      <section aria-labelledby="hp-todo" className="hp-group">
        <h2 id="hp-todo" className="hp-h2">
          Нужно сделать
        </h2>
        <div className="hp-card hp-list">
          {dashboard && todos.length === 0 ? <p className="hp-quiet">Срочных дел нет</p> : null}
          {todos.map((t) => (
            <div key={t.key} className="hp-todo">
              <span className="hp-text">
                <span className="hp-title">{t.title}</span>
                {t.note ? <span className={`hp-note ${t.tone}`}>{t.note}</span> : null}
              </span>
              {t.href ? (
                <a href={t.href} target="_blank" rel="noreferrer" className="hp-pill press" aria-label={t.label}>
                  <span>{t.action}</span>
                </a>
              ) : t.to ? (
                <Link to={t.to} className="hp-pill press" aria-label={t.label}>
                  <span>{t.action}</span>
                </Link>
              ) : (
                <button type="button" className="hp-pill press" aria-label={t.label} aria-disabled={t.busy ? true : undefined} onClick={t.busy ? undefined : t.run}>
                  <span>{t.action}</span>
                </button>
              )}
            </div>
          ))}
        </div>
      </section>

      {payments ? (
        <section aria-labelledby="hp-pay" className="hp-group">
          <h2 id="hp-pay" className="hp-h2">
            Последние оплаты
          </h2>
          <div className="hp-card hp-list">
            {payments.length === 0 ? <p className="hp-quiet">Оплат пока не было</p> : null}
            {payments.map((p) => {
              const name = personName(p.client, p.client_id);
              const status =
                p.status === 'paid' || p.status === 'partially_refunded'
                  ? { text: agoLong(p.paid_at ?? p.created_at), tone: 'muted' }
                  : p.status === 'pending'
                    ? { text: 'Ждём подтверждения', tone: 'warn' }
                    : p.status === 'refunded'
                      ? { text: 'Возвращена', tone: 'muted' }
                      : { text: 'Не прошла', tone: 'bad' };
              return (
                <Link key={p.id} to={`/admin/clients/${p.client_id}`} className="hp-pay press">
                  <Avatar id={p.client_id} name={name} className="hp-ava" />
                  <span className="hp-pay-body">
                    <span className="hp-pay-top">
                      <span className="hp-title hp-ell">{name}</span>
                      <strong className="num">{amountText(p.amount)}</strong>
                    </span>
                    <span className="hp-note muted hp-ell">
                      {orderLine(p.order)} · {lowerFirst(methodText(p.provider, p.method))}
                    </span>
                    <span className={`hp-note ${status.tone}`}>{status.text.charAt(0).toUpperCase() + status.text.slice(1)}</span>
                  </span>
                </Link>
              );
            })}
          </div>
        </section>
      ) : null}
      <PanelServer load={dashboard?.server} variant="phone" />
    </div>
  );
}
