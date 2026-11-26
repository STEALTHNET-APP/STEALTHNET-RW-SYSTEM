import { useCallback, useEffect, useMemo, useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { api } from '../../lib/api';
import { count, dateLong, groupDigits, money, NBSP, plural } from '../../lib/format';
import { Flag } from '../../lib/Flag';
import { Icon } from '../../lib/icons';
import { useMedia, usePhone } from '../../lib/media';
import { HomePhone } from './HomePhone';
import { amountText, lowerFirst, methodText } from '../client/history';
import { PickClient } from '../clients/Clients';
import { useSession } from '../session';
import {
  agoLong,
  countryName,
  feedLine,
  nodesHeader,
  nodeStateText,
  nodesSummary,
  orderLine,
  personName,
  trafficAlert,
  weekdayLine,
  type BillingNode,
  type Dashboard,
  type FeedItem,
  type FeedLine,
  type FeedPage,
  type NodeBrief,
  type PaymentItem,
  type TicketItem,
} from './model';
import { setupTodos } from './setup';
import { desktopTodos, type Waiting, type Withdrawal } from './todo';
import './home.css';
import { Avatar } from '../Avatar';
import { ImportHint } from './ImportHint';
import { ConfigWarnings } from './ConfigWarnings';
import { PanelServer } from './PanelServer';

export function useOverview() {
  const { can } = useSession();
  const [dashboard, setDashboard] = useState<Dashboard | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [restarting, setRestarting] = useState<Set<string>>(new Set());
  const [billing, setBilling] = useState<BillingNode[]>([]);

  const load = useCallback(async () => {
    try {
      setDashboard(await api<Dashboard>('/api/admin/dashboard'));
      setError(null);
    } catch {
      setError('Сводка не загрузилась');
    }
  }, []);

  useEffect(() => {
    void load();
    const timer = window.setInterval(() => void load(), 30_000);
    return () => window.clearInterval(timer);
  }, [load]);

  const remnaUp = !!dashboard?.remna;
  useEffect(() => {
    if (!can('infrastructure') || !remnaUp) {
      return;
    }
    api<{ billingNodes?: BillingNode[] }>('/api/admin/infra/billing')
      .then((data) => {
        const soon = Date.now() + 7 * 86_400_000;
        setBilling((data.billingNodes ?? []).filter((b) => Date.parse(b.nextBillingAt) <= soon));
      })
      .catch(() => setBilling([]));
  }, [can, remnaUp]);

  const restart = useCallback(
    async (uuid: string) => {
      setRestarting((prev) => new Set(prev).add(uuid));
      try {
        await api(`/api/admin/infra/nodes/${uuid}/actions/restart`, { method: 'POST' });
        window.setTimeout(() => void load(), 4000);
      } catch {
        setRestarting((prev) => {
          const next = new Set(prev);
          next.delete(uuid);
          return next;
        });
      }
    },
    [load],
  );

  const nodes = useMemo<NodeBrief[]>(
    () =>
      (dashboard?.remna?.nodes ?? []).map((node) =>
        restarting.has(node.uuid) && node.state === 'down' ? { ...node, state: 'connecting' as const } : node,
      ),
    [dashboard, restarting],
  );

  return { dashboard, error, nodes, restart, restarting, billing, canRestart: can('infrastructure') };
}

function useTodoSources() {
  const { can, me } = useSession();
  const support = can('support');
  const referral = can('money') && me.menu.some((m) => m.module === 'referral');
  const [waiting, setWaiting] = useState<Waiting | null>(null);
  const [withdrawals, setWithdrawals] = useState<Withdrawal[] | null>(null);

  useEffect(() => {
    if (!support) {
      setWaiting(null);
      return;
    }
    api<{ items: TicketItem[]; counts: { waiting: number } }>('/api/admin/support/tickets?filter=waiting&limit=1')
      .then((page) => setWaiting({ count: page.counts.waiting, oldest: page.items[0] ?? null }))
      .catch(() => setWaiting({ count: 0, oldest: null }));
  }, [support]);

  useEffect(() => {
    if (!referral) {
      setWithdrawals([]);
      return;
    }
    api<{ items: Withdrawal[] }>('/api/admin/m/referral/withdrawals?status=pending&limit=100')
      .then((page) => setWithdrawals(page.items))
      .catch(() => setWithdrawals([]));
  }, [referral]);

  return { waiting, withdrawals: withdrawals ?? [], ready: (!support || waiting !== null) && withdrawals !== null };
}

function remnaGone(dashboard: Dashboard): string {
  return dashboard.setup?.remnawave === false
    ? 'Remnawave не подключена: укажите адрес и токен в «Настройки → Remnawave»'
    : 'Remnawave не отвечает: проверьте подключение в настройках';
}

function TodoPane({ dashboard, variant }: { dashboard: Dashboard | null; variant: 'day' | 'main' }) {
  const { can } = useSession();
  const { waiting, withdrawals, ready } = useTodoSources();
  const rows = [...setupTodos(dashboard?.setup, can), ...desktopTodos(dashboard?.decisions, dashboard?.webhooks, waiting, withdrawals)];
  const day = variant === 'day';
  return (
    <section aria-labelledby="todo-h" className={day ? 'hm-sec hm-pane' : 'hm-sec hm-nodes hm-todos'}>
      <div className={day ? 'hm-panehead' : 'hm-feedhead'}>
        <h2 id="todo-h" className={day ? 'hm-h2b' : 'hm-h2'}>
          Нужно сделать {rows.length ? <span className="muted">{rows.length}</span> : null}
        </h2>
      </div>
      {dashboard && ready && rows.length === 0 ? (
        <p className="hm-empty muted" style={day ? { padding: '12px' } : undefined}>
          Срочных дел нет
        </p>
      ) : null}
      {rows.map((todo) => (
        <div key={todo.key} className="hm-todo">
          <span className="hm-text">
            <span className="hm-todo-title">{todo.title}</span>
            {todo.note ? (
              <span className={`hm-note ${todo.tone}`} title={todo.note}>
                {todo.note}
              </span>
            ) : null}
          </span>
          {todo.to ? (
            <Link to={todo.to} className="btn-line hov" aria-label={todo.label}>
              {todo.action}
            </Link>
          ) : null}
        </div>
      ))}
    </section>
  );
}

export function Home() {
  const { me } = useSession();
  const narrow = useMedia('(max-width: 760px)');
  const phone = usePhone();
  const layout = narrow ? 'top' : me.appearance.admin;
  if (phone) {
    return <HomePhone />;
  }
  return layout === 'side' ? <MainBoard /> : <Daylight />;
}

function SearchBox({ big }: { big: boolean }) {
  const navigate = useNavigate();
  const [q, setQ] = useState('');
  const submit = (event: FormEvent) => {
    event.preventDefault();
    if (q.trim()) {
      navigate(`/admin/clients?q=${encodeURIComponent(q.trim())}`);
    }
  };
  return (
    <form className={big ? 'hm-search big' : 'hm-search'} onSubmit={submit} role="search">
      <label className="hm-search-in">
        <Icon name="search" size={big ? 22 : 18} stroke={1.9} />
        <span className="sr-only">{big ? 'Найти клиента' : 'Поиск клиента'}</span>
        <input
          type="search"
          value={q}
          onChange={(e) => setQ(e.target.value)}
          placeholder={big ? 'Найти клиента: имя, @username, Telegram ID, email или ссылка подписки' : 'Найти клиента или платёж'}
          maxLength={200}
        />
      </label>
    </form>
  );
}

function Bars({ values }: { values: number[] }) {
  const max = Math.max(...values, 1);
  return (
    <span className="hm-bars" aria-hidden="true">
      {values.map((v, i) => (
        <span key={i} className={i === values.length - 1 ? 'on' : ''} style={{ height: `${Math.max(4, Math.round((v / max) * 40))}px` }} />
      ))}
    </span>
  );
}

function FeedAvatar({ line, item }: { line: FeedLine; item: FeedItem }) {
  if (line.icon === 'person' && item.client_id) {
    return (
      <Avatar id={item.client_id} name={line.who} className="hm-ava" />
    );
  }
  const icon = line.icon === 'server' ? 'server' : line.icon === 'wallet' ? 'card' : 'users';
  return (
    <span className="hm-ava icon" aria-hidden="true">
      <Icon name={icon} size={20} stroke={1.8} />
    </span>
  );
}

const TONES = ['var(--muted)', 'var(--warn)', 'var(--bad)'];

type FeedFilter = 'highlights' | 'payments' | 'support' | 'nodes';

type FeedProps = {
  nodes: NodeBrief[];
  restart: (uuid: string) => Promise<void>;
  restarting: Set<string>;
  canRestart: boolean;
  variant: 'main' | 'day';
};

function Feed({ nodes, restart, restarting, canRestart, variant }: FeedProps) {
  const { can } = useSession();
  const navigate = useNavigate();
  const [filter, setFilter] = useState<FeedFilter>('highlights');
  const [feed, setFeed] = useState<FeedPage | null>(null);

  useEffect(() => {
    let alive = true;
    const load = () =>
      api<FeedPage>(`/api/admin/feed?kind=${filter}&limit=12`)
        .then((page) => alive && setFeed(page))
        .catch(() => alive && setFeed({ items: [], remna_before: null, core_before: null, has_more: false }));
    setFeed(null);
    void load();
    const timer = window.setInterval(load, 30_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [filter]);

  const lines = (feed?.items ?? []).map((item) => ({ item, line: feedLine(item, nodes) })).filter((x): x is { item: FeedItem; line: FeedLine } => x.line !== null);
  const chips: Array<[FeedFilter, string, boolean]> = [
    ['highlights', 'Всё', true],
    ['payments', 'Деньги', can('money')],
    ['support', 'Поддержка', true],
    ['nodes', 'Ноды', can('infrastructure')],
  ];

  return (
    <section aria-labelledby="feed-h" className={variant === 'main' ? 'hm-sec hm-feed' : 'hm-sec hm-dayfeed'}>
      <div className="hm-feedhead">
        <h2 id="feed-h" className={variant === 'main' ? 'hm-h2' : 'hm-h2b'}>
          Что происходит
        </h2>
        <div role="group" aria-label="Фильтр ленты" className="hm-chips">
          {chips
            .filter(([, , ok]) => ok)
            .map(([key, label]) => (
              <button key={key} type="button" aria-pressed={filter === key} onClick={() => setFilter(key)}>
                {label}
              </button>
            ))}
        </div>
      </div>
      {feed === null ? <p className="hm-empty muted">Загружаем события…</p> : null}
      {feed && lines.length === 0 ? <p className="hm-empty muted">Пока тихо: событий нет</p> : null}
      {lines.map(({ item, line }) => (
        <div key={line.key} className="hm-row">
          <FeedAvatar line={line} item={item} />
          <span className="hm-text">
            <span className="hm-line" title={line.clientId ? `${line.who}: ${line.what}` : `${line.who} ${line.what}`}>
              {line.clientId ? (
                <>
                  <Link to={`/admin/clients/${line.clientId}`} className="hm-who">
                    {line.who}
                  </Link>
                  {line.what ? `: ${line.what}` : ''}
                </>
              ) : (
                <>
                  <strong>{line.who}</strong> {line.what}
                </>
              )}
            </span>
            {line.note ? (
              <span className="hm-note" style={{ color: TONES[line.tone] }} title={line.note}>
                {line.note}
              </span>
            ) : null}
          </span>
          {line.action && line.action.kind === 'restart' && canRestart ? (
            <button type="button" className="b34 hov hm-act" aria-label={line.action.aria} onClick={() => void restart(line.action!.target)} disabled={restarting.has(line.action.target)}>
              {restarting.has(line.action.target) ? 'Перезапускаем…' : line.action.label}
            </button>
          ) : null}
          {line.action && line.action.kind === 'reply' ? (
            <button type="button" className="b34 hov hm-act" aria-label={line.action.aria} onClick={() => navigate(`/admin/support?ticket=${line.action!.target}`)}>
              {line.action.label}
            </button>
          ) : null}
          <span className="hm-time num">{line.when}</span>
        </div>
      ))}
    </section>
  );
}

function MainBoard() {
  const { can } = useSession();
  const { dashboard, error, nodes, restart, restarting, canRestart } = useOverview();
  const [picking, setPicking] = useState(false);

  const revenue = dashboard?.revenue ?? null;
  const today = dashboard?.today ?? null;
  const subs = dashboard?.subscriptions ?? null;
  const delta =
    revenue && revenue.yesterday.minor > 0 ? Math.round(((revenue.today.minor - revenue.yesterday.minor) / revenue.yesterday.minor) * 100) : null;
  const summary = nodesSummary(nodes);
  const remnaOff = dashboard?.setup?.remnawave === false;
  const seesNodes = can('infrastructure');

  return (
    <div className="hm">
      <header className="hm-head">
        <div className="hm-title">
          <span className="hm-date">{weekdayLine()}</span>
          <h1 className="page-title tight">Главная</h1>
        </div>
        <SearchBox big={false} />
        {can('clients_write') ? (
          <button type="button" className="cl-new btn" onClick={() => setPicking(true)}>
            <Icon name="plus" size={18} stroke={2.4} />
            Выдать подписку
          </button>
        ) : null}
      </header>

      <ConfigWarnings />
      <ImportHint />

      {error && !dashboard ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}

      <section aria-label="Сегодня" className="hm-sec hm-today">
        {revenue ? (
          <div className="hm-money">
            <span className="hm-cap">Заработали сегодня</span>
            <span className="hm-big-row">
              <span className="hm-big num">{money(revenue.today, true)}</span>
              <Bars values={revenue.by_day.map((d) => d.amount.minor)} />
            </span>
            <span className="muted">
              {delta !== null ? (
                <>
                  <span style={{ color: delta >= 0 ? 'var(--ok)' : 'var(--bad)', fontWeight: 600 }}>
                    {delta >= 0 ? '+' : '−'}
                    {Math.abs(delta)}%
                  </span>{' '}
                  к вчерашнему дню ·{' '}
                </>
              ) : null}
              за месяц {money(revenue.month, true)}
            </span>
          </div>
        ) : null}
        <div className="hm-stat">
          <span className="hm-cap">Новые клиенты</span>
          <span className="hm-num num">{today ? groupDigits(today.new_clients) : '—'}</span>
          <span className="muted">{today && today.new_clients_paid !== null ? (today.new_clients_paid ? `${groupDigits(today.new_clients_paid)} уже с оплатой` : 'пока без оплат') : ''}</span>
        </div>
        <div className="hm-stat">
          <span className="hm-cap">Онлайн сейчас</span>
          <span className="hm-num num">{dashboard?.remna ? groupDigits(dashboard.remna.online_now) : '—'}</span>
          <span style={{ color: dashboard?.remna && seesNodes && summary.tone === 'bad' ? 'var(--bad)' : 'var(--muted)' }}>
            {dashboard?.remna ? (seesNodes ? summary.text : '') : remnaOff ? 'Remnawave не подключена' : 'Remnawave не отвечает'}
          </span>
        </div>
        <div className="hm-stat">
          <span className="hm-cap">Активные подписки</span>
          <span className="hm-num num">{subs ? groupDigits(subs.active_paid) : '—'}</span>
          <span className="muted">{subs ? `${groupDigits(subs.expiring_soon)} ${plural(subs.expiring_soon, 'истекает', 'истекают', 'истекают')} за 3${NBSP}дня` : ''}</span>
        </div>
      </section>

      <div className="hm-cols">
        <Feed nodes={nodes} restart={restart} restarting={restarting} canRestart={canRestart} variant="main" />

        <div className="hm-side">
        <TodoPane dashboard={dashboard} variant="main" />
        {seesNodes ? (
          <section aria-labelledby="nodes-h" className="hm-sec hm-nodes">
            <div className="hm-feedhead">
              <h2 id="nodes-h" className="hm-h2">
                Ноды
              </h2>
              <span className="hm-small muted">{dashboard?.remna ? nodesHeader(nodes) : ''}</span>
            </div>
            {dashboard && !dashboard.remna ? <p className="hm-empty muted">{remnaGone(dashboard)}</p> : null}
            {nodes.map((node) => {
              const state = nodeStateText(node, true);
              const alert = trafficAlert(node);
              return (
                <div key={node.uuid} className="hm-node">
                  <span className="hm-code" title={node.name}>
                    {node.name}
                  </span>
                  <span className="hm-city muted" title={countryName(node.country_code) || undefined}>
                    <span className="hm-node-flag">
                      <Flag code={node.country_code} />
                    </span>
                    <span>{countryName(node.country_code)}</span>
                  </span>
                  <span className="hm-state num">
                    <span style={{ color: state.tone === 'bad' ? 'var(--bad)' : state.tone === 'warn' ? 'var(--warn)' : state.tone === 'muted' ? 'var(--muted)' : 'var(--ink-2)', fontWeight: state.tone === 'bad' ? 600 : 400 }}>
                      <span className={`st-dot ${state.tone}`} aria-hidden="true" />
                      {state.text}
                    </span>
                    {alert ? (
                      <span style={{ color: 'var(--warn)' }} title={alert.title}>
                        {' '}
                        · {alert.text}
                      </span>
                    ) : null}
                  </span>
                </div>
              );
            })}
          </section>
        ) : null}
        <PanelServer load={dashboard?.server} variant="side" />
        </div>
      </div>
      {picking ? <PickClient onClose={() => setPicking(false)} /> : null}
    </div>
  );
}

function Daylight() {
  const { can, me } = useSession();
  const { dashboard, nodes, restart, restarting, billing, canRestart } = useOverview();
  const [payments, setPayments] = useState<PaymentItem[] | null>(null);
  const [picking, setPicking] = useState(false);

  useEffect(() => {
    if (can('money')) {
      api<{ items: PaymentItem[] }>('/api/admin/payments?limit=5')
        .then((page) => setPayments(page.items))
        .catch(() => setPayments([]));
    }
  }, [can]);

  const revenue = dashboard?.revenue ?? null;
  const today = dashboard?.today ?? null;
  const subs = dashboard?.subscriptions ?? null;
  const online = dashboard?.remna?.online_now ?? null;
  const flagged = nodes.find((n) => n.state === 'down') ?? nodes.find((n) => n.state === 'connecting') ?? null;
  const modules = new Set(me.menu.map((m) => m.module));
  const quick: Array<{ label: string; to?: string; run?: () => void }> = [];
  if (can('clients_write')) {
    quick.push({ label: 'Выдать подписку', run: () => setPicking(true) });
  }
  if (modules.has('promo') && can('marketing')) {
    quick.push({ label: 'Создать промокод', to: '/admin/m/promo' });
  }
  if (modules.has('broadcast') && can('marketing')) {
    quick.push({ label: 'Написать всем', to: '/admin/m/broadcast/new' });
  }
  if (can('infrastructure')) {
    quick.push({ label: 'Добавить ноду', to: '/admin/infra' });
  }

  return (
    <div className="hm">
      <header className="hm-col" style={{ gap: 14 }}>
        <h1 className="page-title tight">Главная</h1>
        <SearchBox big />
        {quick.length ? (
          <div className="cc-btns" style={{ gap: 8 }}>
            {quick.map((q) =>
              q.to ? (
                <Link key={q.label} to={q.to} className="hm-quick hov">
                  {q.label}
                </Link>
              ) : (
                <button key={q.label} type="button" className="hm-quick hov" onClick={q.run}>
                  {q.label}
                </button>
              ),
            )}
          </div>
        ) : null}
      </header>

      <ConfigWarnings />
      <ImportHint />

      <section aria-label="Сводка за сегодня" className="hm-col" style={{ gap: 6 }}>
        <p className="hm-phrase">
          {today && today.payments !== null ? (
            <>
              Сегодня {count(today.payments, 'оплата', 'оплаты', 'оплат')}
              {revenue ? ` на ${money(revenue.today)}` : ''} и {groupDigits(today.new_clients)}
              {NBSP}
              {plural(today.new_clients, 'новый клиент', 'новых клиента', 'новых клиентов')}.
            </>
          ) : today ? (
            <>
              Сегодня {groupDigits(today.new_clients)}
              {NBSP}
              {plural(today.new_clients, 'новый клиент', 'новых клиента', 'новых клиентов')}.
            </>
          ) : null}
          {online !== null ? (
            <>
              {' '}
              Сейчас онлайн {groupDigits(online)}
              {NBSP}
              {plural(online, 'человек', 'человека', 'человек')}
              {flagged ? (
                <>
                  , а{' '}
                  <a href="#servers" className={flagged.state === 'down' ? 'hm-flag bad' : 'hm-flag'}>
                    нода {flagged.name} {flagged.state === 'down' ? 'не отвечает' : 'подключается'}
                  </a>
                  .
                </>
              ) : (
                '.'
              )}
            </>
          ) : dashboard ? (
            dashboard.setup?.remnawave === false ? ' Remnawave пока не подключена.' : ' Remnawave сейчас не отвечает.'
          ) : null}
        </p>
        <p className="muted">
          {[
            revenue ? `За месяц ${money(revenue.month, true)}` : null,
            subs ? `активных подписок ${groupDigits(subs.active_paid)}` : null,
            subs ? `${groupDigits(subs.expiring_soon)} ${plural(subs.expiring_soon, 'истекает', 'истекают', 'истекают')} за 3${NBSP}дня` : null,
          ]
            .filter(Boolean)
            .join(' · ')}
          {subs && subs.expiring_soon > 0 ? (
            <>
              ,{' '}
              <Link to="/admin/clients?segment=expiring_soon" className="hm-link">
                напомнить им
              </Link>
            </>
          ) : null}
        </p>
      </section>

      <div className="hm-cols">
        {dashboard ? <TodoPane dashboard={dashboard} variant="day" /> : null}
        {payments ? (
          <section aria-labelledby="pay-h" className="hm-sec hm-pane">
            <div className="hm-panehead">
              <h2 id="pay-h" className="hm-h2b">
                Последние оплаты
              </h2>
            </div>
            {payments.length === 0 ? <p className="hm-empty muted" style={{ padding: '12px' }}>Оплат пока не было</p> : null}
            {payments.map((p) => {
              const name = personName(p.client, p.client_id);
              const status =
                p.status === 'paid' || p.status === 'partially_refunded'
                  ? { text: agoLong(p.paid_at ?? p.created_at), color: 'var(--muted)' }
                  : p.status === 'pending'
                    ? { text: 'ждём подтверждения', color: 'var(--warn)' }
                    : p.status === 'refunded'
                      ? { text: 'возвращена', color: 'var(--muted)' }
                      : { text: 'не прошла', color: 'var(--bad)' };
              return (
                <Link key={p.id} to={`/admin/clients/${p.client_id}`} className="hm-link-row tight">
                  <Avatar id={p.client_id} name={name} className="hm-ava mid" />
                  <span className="hm-text">
                    <span style={{ fontWeight: 600 }}>{name}</span>
                    <span className="hm-note" style={{ color: 'var(--muted)' }}>
                      {orderLine(p.order)} · {lowerFirst(methodText(p.provider, p.method))}
                    </span>
                  </span>
                  <span className="hm-sum">
                    <strong className="num">{amountText(p.amount)}</strong>
                    <span style={{ color: status.color }}>{status.text}</span>
                  </span>
                </Link>
              );
            })}
          </section>
        ) : null}
      </div>

      {can('infrastructure') ? (
        <section id="servers" aria-labelledby="srv-h" className="hm-sec hm-servers">
          <div className="hm-panehead" style={{ padding: 0 }}>
            <h2 id="srv-h" className="hm-h2b">
              Ноды
            </h2>
            <Link to="/admin/infra" className="hm-link">
              Управление
            </Link>
          </div>
          {dashboard && !dashboard.remna ? <p className="muted">{remnaGone(dashboard)}</p> : null}
          <div className="hm-flow-wrap">
            <div className="hm-flow">
              {nodes.map((node) => {
                const state = nodeStateText(node, false);
                const alert = trafficAlert(node);
                return (
                  <span key={node.uuid} className="hm-flow-item">
                    <span className="hm-bar" aria-hidden="true" />
                    <strong title={node.name}>{node.name}</strong>
                    <span className="muted">
                      <Flag code={node.country_code} />
                      {countryName(node.country_code)}
                    </span>
                    <span className="num" style={{ color: state.tone === 'bad' ? 'var(--bad)' : state.tone === 'warn' ? 'var(--warn)' : state.tone === 'muted' ? 'var(--muted)' : 'var(--ink)', fontWeight: state.tone === 'bad' ? 600 : 400 }}>
                      <span className={`st-dot ${state.tone}`} aria-hidden="true" />
                      {state.text}
                    </span>
                    {alert ? (
                      <span style={{ color: 'var(--warn)' }} title={alert.title}>
                        · {alert.text}
                      </span>
                    ) : null}
                    {canRestart && (node.state === 'down' || restarting.has(node.uuid)) ? (
                      <button type="button" className="b36 hov" onClick={() => void restart(node.uuid)} disabled={restarting.has(node.uuid)} style={{ fontWeight: 600 }}>
                        {restarting.has(node.uuid) ? 'Перезапускаем…' : 'Перезапустить'}
                      </button>
                    ) : null}
                  </span>
                );
              })}
            </div>
          </div>
          {billing.length ? (
            <div className="hm-billing">
              {billing.map((b) => {
                const nodeName = b.node?.name ?? b.name ?? 'нода';
                return (
                  <span key={b.uuid} className="hm-billing-row">
                    <span style={{ color: 'var(--warn)' }}>
                      Оплата хостинга: {nodeName} в {b.provider.name} до {dateLong(b.nextBillingAt)}
                    </span>
                    {b.provider.loginUrl ? (
                      <a href={b.provider.loginUrl} target="_blank" rel="noreferrer" className="hm-link" aria-label={`Открыть ${b.provider.name}, чтобы оплатить ${nodeName}`}>
                        Открыть
                      </a>
                    ) : null}
                  </span>
                );
              })}
            </div>
          ) : null}
        </section>
      ) : null}
      <PanelServer load={dashboard?.server} variant="day" />
      <Feed nodes={nodes} restart={restart} restarting={restarting} canRestart={canRestart} variant="day" />
      {picking ? <PickClient onClose={() => setPicking(false)} /> : null}
    </div>
  );
}
