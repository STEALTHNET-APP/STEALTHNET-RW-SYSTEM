import { useCallback, useEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { dateNumeric, dayMonthNumeric, days, daysLeft, groupDigits, money, NBSP, periodLabel } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { BalanceDialog, BlockDialog, Dialog, GrantDialog, MessageDialog } from '../client/dialogs';
import { useSession } from '../session';
import { ClientsTabs } from '../protect/ClientsTabs';
import { RowMenu, type RowAction, type RowItem } from './RowMenu';
import { displayName, STATUS_WORD, type ClientRow, type Segment, type SegmentCounts } from './types';
import './clients.css';
import { Avatar } from '../Avatar';
import { usePhone } from '../../lib/media';
import { FilterBar, filterCount } from './Conditions';
import { EMPTY, apiQuery, found, readParams, sameConditions, writeParams, type Conditions, type FilterOptions } from './filters';

const SEGMENTS: Array<[Segment, string, keyof SegmentCounts]> = [
  ['all', 'Все', 'total'],
  ['active', 'Активные', 'active'],
  ['expiring_soon', 'Истекают за 3 дня', 'expiring_soon'],
  ['trial', 'Пробный период', 'trial'],
  ['never_connected', 'Не подключились', 'never_connected'],
  ['expired', 'Истекли', 'expired'],
  ['limited', 'Трафик исчерпан', 'limited'],
  ['blocked', 'Заблокированы', 'blocked'],
  ['no_subscription', 'Нет подписки', 'no_subscription'],
];

type Page = { items: ClientRow[]; next: number | null };

type Bulk = 'extend' | 'reset' | 'block' | 'unblock' | 'message';

type Notice = { text: string; undo?: () => void };

const EXTEND_DAYS = 30;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function rowPrint(row: ClientRow): string {
  const sub = row.subscription;
  return sub ? `${sub.id}:${sub.tariff_id ?? ''}:${sub.mirror.expire_at ?? ''}:${sub.period_days ?? ''}:${sub.kind}` : '';
}

const BULK_TEXT: Record<Bulk, { title: string; confirm: string; text: string }> = {
  extend: { title: 'Продлить на 7 дней', confirm: 'Продлить', text: 'Каждому выбранному клиенту с подпиской добавится 7 дней.' },
  reset: { title: 'Сбросить трафик', confirm: 'Сбросить', text: 'Счётчик трафика обнулится у выбранных подписок с лимитом трафика.' },
  block: { title: 'Заблокировать', confirm: 'Заблокировать', text: 'Подписки выбранных клиентов отключатся, вход в кабинет закроется, неоплаченные счета отменятся.' },
  unblock: { title: 'Разблокировать', confirm: 'Разблокировать', text: 'Подписки выбранных клиентов снова включатся.' },
  message: { title: 'Написать', confirm: 'Отправить', text: 'Сообщение придёт от бота каждому выбранному клиенту с Telegram.' },
};

export function PickClient({ onClose }: { onClose: () => void }) {
  const navigate = useNavigate();
  const [q, setQ] = useState('');
  const [rows, setRows] = useState<ClientRow[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    const query = q.trim();
    if (!query) {
      setRows(null);
      return;
    }
    const timer = window.setTimeout(() => {
      api<{ items: ClientRow[] }>(`/api/admin/clients?limit=8&q=${encodeURIComponent(query)}`)
        .then((page) => {
          setRows(page.items);
          setError(null);
        })
        .catch((err) => setError(err instanceof ApiError ? err.message : 'Поиск не сработал'));
    }, 300);
    return () => window.clearTimeout(timer);
  }, [q]);
  return (
    <Dialog title="Кому выдать подписку" onClose={onClose} wide>
      <label className="cl-search" style={{ flex: 'none' }}>
        <Icon name="search" size={18} stroke={1.9} />
        <span className="sr-only">Поиск клиента</span>
        <input type="search" placeholder="Имя, @username, ID или email" value={q} onChange={(e) => setQ(e.target.value)} maxLength={200} autoFocus />
      </label>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {rows && rows.length === 0 ? <p className="muted">Никого не нашли</p> : null}
      {rows && rows.length > 0 ? (
        <div className="pick-list">
          {rows.map((row) => {
            const name = displayName(row);
            const handle = row.telegram_username ? `@${row.telegram_username}` : (row.email ?? (row.telegram_id ? `Telegram ${row.telegram_id}` : ''));
            return (
              <button key={row.client.id} type="button" className="pick-row hov" onClick={() => navigate(`/admin/clients/${row.client.id}?grant=1`)}>
                <Avatar id={row.client.id} name={name} className="av" />
                <span className="two">
                  <span className="main">{name}</span>
                  <span className="sub">
                    {handle}
                    {handle ? ' · ' : ''}
                    {STATUS_WORD[row.status][0]}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      ) : null}
      {rows === null ? <p className="cap">Найдите клиента, затем выберите тариф и срок в его карточке</p> : null}
      <div className="dialog-actions">
        <button type="button" className="btn-line hov" onClick={onClose}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

function rowView(row: ClientRow) {
  const name = displayName(row);
  const handle = row.telegram_username ? `@${row.telegram_username}` : row.email ?? 'без username';
  const sub = row.subscription;
  const [word, color] = STATUS_WORD[row.status];
  let plan = '';
  if (row.status === 'blocked') {
    plan = row.client.block_reason ?? '';
  } else if (sub?.kind === 'trial') {
    plan = sub.period_days ? `${days(sub.period_days)} бесплатно` : 'Пробный период';
  } else if (sub) {
    plan = [row.tariff_name, periodLabel(sub.period_days)].filter(Boolean).join(`${NBSP}· `);
  } else if (row.balance && row.balance.minor > 0) {
    plan = 'Пополнение баланса';
  }
  const expire = sub?.mirror.expire_at ?? null;
  const left = daysLeft(expire);
  let leftText = '—';
  let until = '';
  const dim = row.status === 'expired' || row.status === 'blocked' || row.status === 'no_subscription';
  if (row.status === 'expired') {
    until = expire ? `была до ${dateNumeric(expire)}` : '';
  } else if (!dim && left !== null) {
    leftText = days(Math.max(left, 0));
    until = sub?.autorenew.enabled && row.status === 'expiring' ? `автопродление ${dayMonthNumeric(expire)}` : `до ${dateNumeric(expire)}`;
  }
  const devices =
    sub && sub.mirror.hwid_limit !== null && row.status !== 'no_subscription'
      ? `${sub.mirror.devices_count ?? 0}${NBSP}из${NBSP}${sub.mirror.hwid_limit}`
      : '—';
  return { name, handle, word, color, plan, leftText, until, dim, devices };
}

export function Clients() {
  const navigate = useNavigate();
  const { can, me } = useSession();
  const phone = usePhone();
  const [params, setParams] = useSearchParams();
  const paramsKey = params.toString();
  const cond = useMemo(() => readParams(new URLSearchParams(paramsKey)), [paramsKey]);
  const segment: Segment = cond.status ?? 'all';
  const query = params.get('q') ?? '';
  const [input, setInput] = useState(query);
  const [counts, setCounts] = useState<SegmentCounts | null>(null);
  const [scoped, setScoped] = useState<SegmentCounts | null>(null);
  const [options, setOptions] = useState<FilterOptions | null>(null);
  const [page, setPage] = useState<Page | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [selected, setSelected] = useState<Set<number>>(new Set());
  const [bulk, setBulk] = useState<Bulk | null>(null);
  const [bulkBusy, setBulkBusy] = useState(false);
  const [notice, setNotice] = useState<Notice | null>(null);
  const [bulkText, setBulkText] = useState('');
  const [picking, setPicking] = useState(false);
  const [act, setAct] = useState<{ kind: RowAction; row: ClientRow } | null>(null);
  const requestRef = useRef(0);
  const writable = can('clients_write');
  const moneyOk = can('money');
  const scope = useMemo(() => ({ ...cond, status: null }), [cond]);
  const filtered = !sameConditions(scope, EMPTY) || query.trim() !== '';

  const url = useCallback(
    (after?: number | null) => {
      const search = new URLSearchParams({ limit: '50' });
      if (query.trim()) {
        search.set('q', query.trim());
      }
      if (after) {
        search.set('after', String(after));
      }
      return `/api/admin/clients?${apiQuery(cond, search).toString()}`;
    },
    [cond, query],
  );

  const countsUrl = useMemo(() => {
    const search = new URLSearchParams();
    if (query.trim()) {
      search.set('q', query.trim());
    }
    return `/api/admin/clients/segments?${apiQuery(scope, search).toString()}`;
  }, [scope, query]);

  useEffect(() => {
    let alive = true;
    api<FilterOptions>('/api/admin/clients/filter-options')
      .then((value) => {
        if (alive) {
          setOptions(value);
        }
      })
      .catch(() => undefined);
    return () => {
      alive = false;
    };
  }, []);

  const reload = useCallback(async (): Promise<Page | null> => {
    const id = ++requestRef.current;
    setError(null);
    const all = api<SegmentCounts>('/api/admin/clients/segments').then((value) => {
      if (id === requestRef.current) {
        setCounts(value);
      }
    });
    const part = filtered
      ? api<SegmentCounts>(countsUrl).then((value) => {
          if (id === requestRef.current) {
            setScoped(value);
          }
        })
      : Promise.resolve(setScoped(null));
    try {
      const list = await api<Page>(url());
      if (id === requestRef.current) {
        setPage(list);
      }
      await Promise.all([all, part]);
      return list;
    } catch (err) {
      if (id === requestRef.current) {
        setError(err instanceof ApiError ? err.message : 'Список не загрузился');
      }
      return null;
    }
  }, [url, countsUrl, filtered]);

  useEffect(() => {
    setSelected(new Set());
    void reload();
  }, [reload]);

  useEffect(() => {
    setInput(query);
  }, [query]);

  useEffect(() => {
    const timer = window.setTimeout(() => {
      if (input.trim() === query.trim()) {
        return;
      }
      const next = new URLSearchParams(params);
      if (input.trim()) {
        next.set('q', input.trim());
      } else {
        next.delete('q');
      }
      setParams(next, { replace: true });
    }, 350);
    return () => window.clearTimeout(timer);
  }, [input, query, params, setParams]);

  const pickSegment = (value: Segment) => {
    setParams(writeParams({ ...cond, status: value === 'all' ? null : value }, params));
  };

  const setCond = (next: Conditions) => {
    setParams(writeParams({ ...next, status: cond.status }, params), { replace: true });
  };

  const resetAll = () => {
    setInput('');
    setParams(new URLSearchParams());
  };

  const more = async () => {
    if (!page?.next || loadingMore) {
      return;
    }
    setLoadingMore(true);
    try {
      const list = await api<Page>(url(page.next));
      setPage({ items: [...page.items, ...list.items], next: list.next });
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Не удалось догрузить');
    } finally {
      setLoadingMore(false);
    }
  };

  const rows = page?.items ?? [];
  const allOn = rows.length > 0 && rows.every((row) => selected.has(row.client.id));
  const toggleAll = () => setSelected(allOn ? new Set() : new Set(rows.map((row) => row.client.id)));
  const toggle = (id: number) => {
    const next = new Set(selected);
    if (next.has(id)) {
      next.delete(id);
    } else {
      next.add(id);
    }
    setSelected(next);
  };

  const chosen = useMemo(() => rows.filter((row) => selected.has(row.client.id)), [rows, selected]);
  const segKey = SEGMENTS.find(([key]) => key === segment)?.[2] ?? 'total';
  const chipCounts = filtered ? scoped : counts;
  const total = chipCounts ? chipCounts[segKey] : null;
  const extra = filterCount(cond, 'clients');
  const mailable = (extra > 0 || segment !== 'all') && !query.trim() && cond.status !== 'blocked' && can('marketing') && (me.modules ?? []).includes('broadcast');
  const mailLink = `/admin/m/broadcast/new?${writeParams(cond, new URLSearchParams()).toString()}`;
  const tail =
    extra > 0 || segment !== 'all' || query.trim() ? (
      <>
        {total !== null ? <span className="flt-found">{found(total)}</span> : null}
        {mailable ? (
          <Link to={mailLink} className="flt-reset hov">
            Рассылка по этому отбору
          </Link>
        ) : null}
        <button type="button" className="flt-reset hov" onClick={resetAll}>
          Сбросить всё
        </button>
      </>
    ) : null;

  const runBulk = async (action: Bulk) => {
    setBulkBusy(true);
    let done = 0;
    let failed = 0;
    let firstError: string | null = null;
    for (const row of chosen) {
      try {
        if (action === 'extend' && row.subscription && !row.subscription.ended_at) {
          await api(`/api/admin/subscriptions/${row.subscription.id}/actions/extend`, { method: 'POST', body: { days: 7 } });
          done += 1;
        } else if (action === 'reset' && row.subscription?.mirror.traffic_limit_bytes) {
          await api(`/api/admin/subscriptions/${row.subscription.id}/actions/reset-traffic`, { method: 'POST' });
          done += 1;
        } else if (action === 'block' && row.client.status !== 'blocked') {
          await api(`/api/admin/clients/${row.client.id}/block`, { method: 'POST' });
          done += 1;
        } else if (action === 'unblock' && row.client.status === 'blocked') {
          await api(`/api/admin/clients/${row.client.id}/unblock`, { method: 'POST' });
          done += 1;
        } else if (action === 'message' && row.telegram_id) {
          await api(`/api/admin/clients/${row.client.id}/message`, { method: 'POST', body: { text: bulkText } });
          done += 1;
        }
      } catch (err) {
        failed += 1;
        if (!firstError && err instanceof ApiError) {
          firstError = err.message;
        }
      }
    }
    setBulkBusy(false);
    setBulk(null);
    setNotice({
      text: failed
        ? `Готово: ${groupDigits(done)}, не получилось: ${groupDigits(failed)}${firstError ? ` (${firstError})` : ''}`
        : `Готово: ${groupDigits(done)}`,
    });
    setBulkText('');
    setSelected(new Set());
    void reload();
  };

  const rowItems = (row: ClientRow): RowItem[] => {
    const sub = row.subscription && !row.subscription.ended_at ? row.subscription : null;
    const blocked = row.client.status === 'blocked';
    const items: RowItem[] = [];
    if (writable && sub) {
      items.push({ key: 'extend', label: `Продлить на ${days(EXTEND_DAYS)}`, icon: 'plus' });
      items.push({ key: 'change', label: 'Сменить тариф', icon: 'tag' });
    }
    if (writable && !sub && !blocked) {
      items.push({ key: 'grant', label: 'Выдать подписку', icon: 'gift' });
    }
    if (writable && row.telegram_id) {
      items.push({ key: 'message', label: 'Написать', icon: 'send' });
    }
    if (moneyOk && row.balance) {
      items.push({ key: 'balance', label: 'Пополнить баланс', icon: 'wallet' });
    }
    if (writable) {
      items.push(blocked ? { key: 'unblock', label: 'Разблокировать', icon: 'refresh' } : { key: 'block', label: 'Заблокировать', icon: 'close', bad: true });
    }
    return items;
  };

  const undoExtend = async (subId: number, name: string) => {
    setNotice({ text: `${name}: отменяем продление…` });
    try {
      await api(`/api/admin/subscriptions/${subId}/actions/undo-extend`, { method: 'POST', body: { count: 1 } });
      setNotice({ text: `${name}: продление отменено` });
      void reload();
    } catch (err) {
      setNotice({ text: `${name}: ${failText(err, 'отменить не получилось')}` });
    }
  };

  const waitGrant = async (clientId: number, name: string, before: string) => {
    setNotice({ text: `${name}: выдаём подписку…` });
    for (let attempt = 0; attempt < 10; attempt += 1) {
      await new Promise((resolve) => window.setTimeout(resolve, 1500));
      const list = await reload();
      if (!list) {
        continue;
      }
      const row = list.items.find((item) => item.client.id === clientId);
      if (!row || rowPrint(row) !== before) {
        setNotice({ text: `${name}: подписка обновлена` });
        return;
      }
    }
    setNotice({ text: `${name}: заказ на выдачу создан, подписка появится в течение пары минут` });
  };

  const runRow = async (kind: RowAction, row: ClientRow) => {
    const name = displayName(row);
    const sub = row.subscription && !row.subscription.ended_at ? row.subscription : null;
    if (kind === 'extend' && sub) {
      try {
        await api(`/api/admin/subscriptions/${sub.id}/actions/extend`, { method: 'POST', body: { days: EXTEND_DAYS } });
        setNotice({ text: `${name}: добавлено ${days(EXTEND_DAYS)}`, undo: () => void undoExtend(sub.id, name) });
        void reload();
      } catch (err) {
        setNotice({ text: `${name}: ${failText(err, 'дни не добавились')}` });
      }
      return;
    }
    if (kind === 'unblock') {
      try {
        await api(`/api/admin/clients/${row.client.id}/unblock`, { method: 'POST' });
        setNotice({ text: `${name}: доступ снова открыт` });
        void reload();
      } catch (err) {
        setNotice({ text: `${name}: ${failText(err, 'разблокировать не получилось')}` });
      }
      return;
    }
    setAct({ kind, row });
  };

  const openCard = (event: ReactMouseEvent<HTMLElement>, id: number) => {
    const target = event.target as HTMLElement;
    const middle = event.button === 1;
    if (event.defaultPrevented || (event.button !== 0 && !middle) || !event.currentTarget.contains(target) || target.closest('a, button, input, label')) {
      return;
    }
    if (!middle && window.getSelection()?.toString()) {
      return;
    }
    const path = `/admin/clients/${id}`;
    if (middle || event.metaKey || event.ctrlKey || event.shiftKey) {
      window.open(path, '_blank', 'noopener');
    } else {
      navigate(path);
    }
  };

  const closeAct = () => setAct(null);
  const actName = act ? displayName(act.row) : '';
  const actSub = act?.row.subscription && !act.row.subscription.ended_at ? act.row.subscription : null;

  return (
    <>
      <header className="cl-head">
        <div className="cl-head-title">
          <h1 className="page-title">Клиенты</h1>
          {counts ? (
            <span className="muted">
              {groupDigits(counts.total)}
              {NBSP}
              {counts.total % 10 >= 2 && counts.total % 10 <= 4 && (counts.total % 100 < 12 || counts.total % 100 > 14) ? 'человека' : 'человек'}, у {groupDigits(counts.active)} активная подписка
            </span>
          ) : null}
        </div>
        <label className="cl-search">
          <Icon name="search" size={18} stroke={1.9} />
          <span className="sr-only">Поиск клиента</span>
          <input type="search" placeholder="Имя, @username, ID, email или ссылка" value={input} onChange={(e) => setInput(e.target.value)} maxLength={200} />
        </label>
        {writable ? (
          <button type="button" className="cl-new btn" onClick={() => setPicking(true)}>
            <Icon name="plus" size={18} stroke={2.4} />
            Выдать подписку
          </button>
        ) : null}
      </header>

      <ClientsTabs />

      <div role="group" aria-label="Кого показать" className="chips flt-scroll">
        {SEGMENTS.map(([key, label, countKey]) => (
          <button key={key} type="button" className="chip" aria-pressed={segment === key} onClick={() => pickSegment(key)}>
            <span>{label}</span>
            <span className="chip-n">{chipCounts ? groupDigits(chipCounts[countKey]) : ''}</span>
          </button>
        ))}
      </div>

      <FilterBar value={cond} onChange={setCond} options={options} currency={me.currency} money={moneyOk} mode="clients" phone={phone} tail={tail} />

      <section aria-label="Список клиентов" className="cl-sec">
        {chosen.length > 0 && writable ? (
          <div role="group" aria-label="Действия с выбранными" className="cl-bulk">
            <b>Выбрано: {chosen.length}</b>
            <button type="button" className="b3 hov" onClick={() => setBulk('extend')}>
              Продлить
            </button>
            <button type="button" className="b3 hov" onClick={() => setBulk('reset')}>
              Сбросить трафик
            </button>
            <button type="button" className="b3 hov" onClick={() => setBulk('message')}>
              Написать
            </button>
            {cond.status === 'blocked' ? (
              <button type="button" className="b3 hov" onClick={() => setBulk('unblock')}>
                Разблокировать
              </button>
            ) : (
              <button type="button" className="b3 hov" style={{ color: 'var(--bad)' }} onClick={() => setBulk('block')}>
                Заблокировать
              </button>
            )}
            <span style={{ flex: 1 }} />
            <button type="button" className="quiet hov" onClick={() => setSelected(new Set())}>
              Снять выбор
            </button>
          </div>
        ) : null}
        {notice ? (
          <div className="cl-bulk" role="status">
            <span>{notice.text}</span>
            <span style={{ flex: 1 }} />
            {notice.undo ? (
              <button type="button" className="b3 hov" onClick={notice.undo}>
                Отменить
              </button>
            ) : null}
            <button type="button" className="quiet hov" onClick={() => setNotice(null)}>
              Скрыть
            </button>
          </div>
        ) : null}
        <div className="cl-scroll">
          <div role="table" aria-label="Клиенты" className="cl-table">
            <div role="row" className="cl-row head">
              <span role="columnheader" style={{ display: 'flex' }}>
                {writable ? <input type="checkbox" aria-label="Выбрать всех" checked={allOn} onChange={toggleAll} /> : null}
              </span>
              <span role="columnheader">Клиент</span>
              <span role="columnheader">Подписка</span>
              <span role="columnheader">Осталось</span>
              <span role="columnheader">Устройства</span>
              <span role="columnheader" className="right">
                Баланс
              </span>
              <span role="columnheader" className="right">
                Оплачено
              </span>
              <span role="columnheader">
                <span className="sr-only">Действия</span>
              </span>
            </div>
            {rows.map((row) => {
              const view = rowView(row);
              const on = selected.has(row.client.id);
              const balanceZero = !row.balance || row.balance.minor === 0;
              const paidZero = !row.paid_total || row.paid_total.minor === 0;
              return (
                <div
                  role="row"
                  key={row.client.id}
                  className={on ? 'cl-row hov opens sel' : 'cl-row hov opens'}
                  onClick={(event) => openCard(event, row.client.id)}
                  onAuxClick={(event) => openCard(event, row.client.id)}
                >
                  <span role="cell" style={{ display: 'flex' }}>
                    {writable ? <input type="checkbox" aria-label={`Выбрать: ${view.name}`} checked={on} onChange={() => toggle(row.client.id)} /> : null}
                  </span>
                  <div role="cell" className="cl-who">
                    <Avatar id={row.client.id} name={view.name} className="av" />
                    <span className="two">
                      <Link to={`/admin/clients/${row.client.id}`} className="main" title={view.name}>
                        {view.name}
                      </Link>
                      <span className="sub">{view.handle}</span>
                    </span>
                  </div>
                  <div role="cell" className="two">
                    <span style={{ fontWeight: 600, color: view.color }}>{view.word}</span>
                    <span className="sub" title={view.plan}>
                      {view.plan}
                    </span>
                  </div>
                  <div role="cell" className="two num">
                    <span style={{ fontWeight: 700, color: view.dim ? 'var(--muted)' : 'var(--ink)' }}>{view.leftText}</span>
                    <span className="sub">{view.until}</span>
                  </div>
                  <span role="cell" className="num">
                    {view.devices}
                  </span>
                  <span role="cell" className="num right" style={{ fontWeight: 600, color: balanceZero ? 'var(--muted)' : 'var(--ink)' }}>
                    {money(row.balance)}
                  </span>
                  <span role="cell" className="num right" style={{ color: paidZero ? 'var(--muted)' : 'var(--ink-2)' }}>
                    {money(row.paid_total)}
                  </span>
                  <span role="cell">
                    <RowMenu name={view.name} href={`/admin/clients/${row.client.id}`} items={rowItems(row)} onPick={(key) => void runRow(key, row)} />
                  </span>
                </div>
              );
            })}
            {page && rows.length === 0 ? (
              <div className="cl-empty">{filtered ? 'Никого не нашли. Проверьте условия или сбросьте фильтры.' : 'В этом списке пока никого нет.'}</div>
            ) : null}
            {error ? (
              <div className="cl-empty" role="alert" style={{ color: 'var(--bad)' }}>
                {error}
              </div>
            ) : null}
          </div>
        </div>
        <div className="cl-foot">
          <span>
            Показано {groupDigits(rows.length)}
            {total !== null ? ` из ${groupDigits(total)}` : ''}
          </span>
          {page?.next ? (
            <button type="button" className="cl-more hov" onClick={() => void more()} disabled={loadingMore}>
              {loadingMore ? 'Загружаем…' : 'Показать ещё 50'}
            </button>
          ) : null}
        </div>
      </section>

      {bulk ? (
        <div className="dialog-back" role="presentation" onClick={() => (bulkBusy ? undefined : setBulk(null))}>
          <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="bulk-h" onClick={(e) => e.stopPropagation()}>
            <h2 id="bulk-h">
              {BULK_TEXT[bulk].title}: {chosen.length}
            </h2>
            <p className="muted">{BULK_TEXT[bulk].text}</p>
            {bulk === 'message' ? (
              <textarea
                className="field"
                rows={5}
                maxLength={3500}
                aria-label="Текст сообщения"
                value={bulkText}
                onChange={(e) => setBulkText(e.target.value)}
                disabled={bulkBusy}
                autoFocus
              />
            ) : null}
            <div className="dialog-actions">
              <button
                type="button"
                className={bulk === 'block' ? 'btn-bad' : 'btn-main btn'}
                onClick={() => void runBulk(bulk)}
                disabled={bulkBusy || (bulk === 'message' && !bulkText.trim())}
              >
                {bulkBusy ? 'Выполняем…' : BULK_TEXT[bulk].confirm}
              </button>
              <button type="button" className="btn-line hov" onClick={() => setBulk(null)} disabled={bulkBusy}>
                Отменить
              </button>
            </div>
          </div>
        </div>
      ) : null}
      {picking ? <PickClient onClose={() => setPicking(false)} /> : null}
      {act?.kind === 'message' ? (
        <MessageDialog
          clientId={act.row.client.id}
          blockedReason={act.row.client.bot_unreachable ? 'Клиент остановил бота, сообщение не дойдёт.' : null}
          onClose={closeAct}
          onSent={() => {
            closeAct();
            setNotice({ text: `${actName}: сообщение в очереди, бот отправит его в течение минуты` });
          }}
        />
      ) : null}
      {act?.kind === 'balance' && act.row.balance ? (
        <BalanceDialog
          clientId={act.row.client.id}
          currency={me.currency}
          balance={act.row.balance}
          initial="add"
          onClose={closeAct}
          onDone={(next) => {
            closeAct();
            setNotice({ text: `${actName}: баланс теперь ${money(next)}` });
            void reload();
          }}
        />
      ) : null}
      {act && (act.kind === 'grant' || act.kind === 'change') ? (
        <GrantDialog
          clientId={act.row.client.id}
          subscriptionId={act.kind === 'change' && actSub ? actSub.id : null}
          currentTariffId={act.kind === 'change' && actSub ? actSub.tariff_id : null}
          currentTrial={act.kind === 'change' && actSub?.kind === 'trial'}
          onClose={closeAct}
          onDone={() => {
            const { row } = act;
            closeAct();
            void waitGrant(row.client.id, actName, rowPrint(row));
          }}
        />
      ) : null}
      {act?.kind === 'block' ? (
        <BlockDialog
          clientId={act.row.client.id}
          onClose={closeAct}
          onDone={() => {
            closeAct();
            setNotice({ text: `${actName}: доступ заблокирован` });
            void reload();
          }}
        />
      ) : null}
    </>
  );
}
