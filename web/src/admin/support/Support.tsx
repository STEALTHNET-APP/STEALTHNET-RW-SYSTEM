import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, dateNumeric, days, daysLeft, groupDigits, money, NBSP, timeHM, traffic, trafficPair } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { useMedia, usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import { planTitle } from '../client/SubscriptionBlock';
import { emailOf, liveSubscriptions, nextReset, personName, platformName, seenText, tariffOf, telegramOf } from '../client/model';
import type { ClientCard, Device, SubscriptionDetails } from '../client/types';
import { minutesSince, spanText } from '../home/model';
import '../form.css';
import './support.css';
import { Avatar } from '../Avatar';
import type { HandoffReason, TicketAi } from '../../lib/supportChat';
import { AttachButton, DraftList, MessageFiles, useAttachments, type Attachment, type FileLimits } from '../../lib/ticketFiles';
import { appendPage, collect, cursorOf, DAYS_MAX, listPath, PAGE, PICK_MAX, readDays, readFilter, type Cursor, type Filter, type Paged } from './list';

const FILES = '/api/admin/support/files';
const CLOSE_MANY = '/api/admin/support/tickets/close-many';
const NARROW = '(max-width: 760px)';

type Ticket = {
  id: number;
  client_id: number;
  status: 'open' | 'closed';
  channel: 'bot' | 'cabinet' | 'miniapp' | 'unknown';
  subject: string | null;
  last_author: 'client' | 'support' | 'ai' | 'system';
  created_at: string;
  updated_at: string;
  closed_at: string | null;
};

type ListItem = {
  ticket: Ticket;
  client_name: string | null;
  telegram_username: string | null;
  email: string | null;
  waiting_since: string | null;
  last_message_at: string | null;
  preview: string | null;
  last_author: string | null;
  unread: boolean;
  ai: TicketAi | null;
};

type Counts = { waiting: number; answered: number; closed: number };

type ListPage = Paged<ListItem> & { counts: Counts; files?: FileLimits | null };

type Message = { author: 'client' | 'support' | 'ai' | 'system'; admin_id: number | null; body: string; attachments: Attachment[]; created_at: string };

type Thread = { ticket: Ticket; messages: Message[]; admins: Array<{ id: number; email: string }>; ai: TicketAi | null };

const FILTERS: Array<[Filter, string, keyof Counts]> = [
  ['waiting', 'Ждут ответа', 'waiting'],
  ['answered', 'Ответ дан', 'answered'],
  ['closed', 'Закрытые', 'closed'],
];

const TEMPLATES: Array<[string, string]> = [
  ['Обновите подписку', 'Обновите подписку в приложении. Если не поможет, напишите, разберёмся.'],
  ['Подключитесь заново', 'Отключили все устройства. Обновите подписку в приложении и подключитесь заново. Если не поможет, напишите, разберёмся.'],
  ['Дата и время', 'Проверьте дату и время на телефоне. Если не поможет, напишите, разберёмся.'],
];

const HANDOFF_WORD: Record<HandoffReason, string> = {
  human: 'клиент попросил оператора',
  money: 'вопрос про оплату, деньги или возврат',
  unsure: 'ИИ не уверен в ответе',
  no_answer: 'ИИ не ответил',
  groq: 'ИИ не ответил',
  client_limit: 'клиент исчерпал ответы ИИ на сегодня',
  budget: 'дневной бюджет ИИ закончился',
  operator: 'оператор ответил сам',
  disabled: 'ИИ-чат выключен',
};

function aiMark(ai: TicketAi | null, status: Ticket['status'], lastAuthor: string | null): string | null {
  if (!ai || status === 'closed') {
    return null;
  }
  if (ai.state === 'active') {
    return lastAuthor === 'client' ? 'ИИ отвечает' : 'Ответил ИИ';
  }
  return ai.reason && ai.reason !== 'operator' && ai.reason !== 'disabled' ? 'От ИИ' : null;
}

function handoffLine(ai: TicketAi | null): string | null {
  if (!ai || ai.state !== 'handed_off' || !ai.handoff_at) {
    return null;
  }
  return ai.reason ? `ИИ передал обращение оператору: ${HANDOFF_WORD[ai.reason]}` : 'ИИ передал обращение оператору';
}

const CHANNEL_WORD: Record<Ticket['channel'], string> = {
  bot: 'через бота',
  cabinet: 'через кабинет',
  miniapp: 'через мини-приложение',
  unknown: '',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function itemName(item: ListItem): string {
  return item.client_name ?? (item.telegram_username ? `@${item.telegram_username}` : (item.email ?? `Клиент ${item.ticket.client_id}`));
}

function ticketState(ticket: Ticket, ai: TicketAi | null): { text: string; color: string } {
  if (ticket.status === 'closed') {
    return { text: 'Закрыто', color: 'var(--muted)' };
  }
  if (ai?.state === 'active') {
    return ticket.last_author === 'client' ? { text: 'ИИ отвечает', color: 'var(--ink-2)' } : { text: 'Ответил ИИ', color: 'var(--ink-2)' };
  }
  if (ticket.last_author === 'client') {
    return { text: 'Ждёт ответа', color: 'var(--warn)' };
  }
  return { text: 'Ответ дан', color: 'var(--ink-2)' };
}

function dayLabel(iso: string): string {
  const date = new Date(iso);
  const start = (d: Date) => new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = Math.round((start(new Date()) - start(date)) / 86_400_000);
  if (diff === 0) {
    return `Сегодня, ${timeHM(iso)}`;
  }
  if (diff === 1) {
    return `Вчера, ${timeHM(iso)}`;
  }
  return `${dateLong(iso)}, ${timeHM(iso)}`;
}

export function Support() {
  const { me, supportWaiting } = useSession();
  const phone = usePhone();
  const single = useMedia(NARROW);
  const [params, setParams] = useSearchParams();
  const filter = readFilter(params.get('filter'));
  const ticketParam = params.get('ticket');
  const selected = ticketParam && /^\d{1,18}$/.test(ticketParam) ? Number(ticketParam) : null;
  const [q, setQ] = useState('');
  const [query, setQuery] = useState('');
  const [items, setItems] = useState<ListItem[] | null>(null);
  const [cursor, setCursor] = useState<Cursor | null>(null);
  const [loadingMore, setLoadingMore] = useState(false);
  const [listError, setListError] = useState<string | null>(null);
  const [round, setRound] = useState(0);
  const [counts, setCounts] = useState<Counts>({ waiting: 0, answered: 0, closed: 0 });
  const [picking, setPicking] = useState(false);
  const [picked, setPicked] = useState<Set<number>>(() => new Set());
  const [bulk, setBulk] = useState<'picked' | 'old' | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [asideOpen, setAsideOpen] = useState(false);
  const [person, setPerson] = useState<Person | null>(null);
  const [thread, setThread] = useState<Thread | null>(null);
  const [threadError, setThreadError] = useState<string | null>(null);
  const [reply, setReply] = useState('');
  const [closeAfter, setCloseAfter] = useState(false);
  const [sending, setSending] = useState(false);
  const [sendError, setSendError] = useState<string | null>(null);
  const [fileLimits, setFileLimits] = useState<FileLimits | null>(null);
  const [askDelete, setAskDelete] = useState(false);
  const files = useAttachments(FILES, fileLimits);
  const resetFiles = files.reset;
  const feedRef = useRef<HTMLDivElement | null>(null);
  const listRef = useRef<HTMLDivElement | null>(null);
  const moreRef = useRef<HTMLButtonElement | null>(null);
  const itemsRef = useRef<ListItem[] | null>(null);
  const cursorRef = useRef<Cursor | null>(null);
  const generation = useRef(0);
  const busy = useRef<symbol | null>(null);
  const listScroll = useRef(0);
  const live = useRef({ single, selected, setParams });
  const showList = !single || selected === null;
  const showThread = !single || selected !== null;

  useEffect(() => {
    live.current = { single, selected, setParams };
  });

  useEffect(() => {
    const timer = window.setTimeout(() => setQuery(q.trim()), 300);
    return () => window.clearTimeout(timer);
  }, [q]);

  const apply = useCallback(
    (page: ListPage, list: ListItem[], next: Cursor | null) => {
      itemsRef.current = list;
      cursorRef.current = next;
      setItems(list);
      setCursor(next);
      setCounts(page.counts);
      setFileLimits(page.files ?? null);
      supportWaiting(page.counts.waiting);
    },
    [supportWaiting],
  );

  const fetchPage = useCallback((after: Cursor | null, limit: number) => api<ListPage>(listPath(filter, query, after, limit)), [filter, query]);

  useEffect(() => {
    const mine = ++generation.current;
    const token = Symbol('list');
    busy.current = token;
    itemsRef.current = null;
    cursorRef.current = null;
    setItems(null);
    setCursor(null);
    setListError(null);
    setPicked(new Set());
    listRef.current?.scrollTo({ top: 0 });
    fetchPage(null, PAGE)
      .then((page) => {
        if (mine !== generation.current) {
          return;
        }
        apply(page, page.items, cursorOf(page));
        const now = live.current;
        if (!now.single && now.selected === null && page.items[0]) {
          const first = page.items[0].ticket.id;
          now.setParams(
            (prev) => {
              const next = new URLSearchParams(prev);
              next.set('ticket', String(first));
              return next;
            },
            { replace: true },
          );
        }
      })
      .catch((err: unknown) => {
        if (mine === generation.current) {
          itemsRef.current = [];
          setItems([]);
          setListError(failText(err, 'Список не загрузился'));
        }
      })
      .finally(() => {
        if (busy.current === token) {
          busy.current = null;
        }
      });
  }, [fetchPage, apply, round]);

  const refresh = useCallback(async () => {
    if (busy.current) {
      return;
    }
    const mine = generation.current;
    const token = Symbol('refresh');
    busy.current = token;
    try {
      const got = await collect<ListItem, ListPage>(fetchPage, Math.max(PAGE, itemsRef.current?.length ?? 0));
      if (mine === generation.current) {
        apply(got.last, got.items, got.next);
      }
    } catch {
      return;
    } finally {
      if (busy.current === token) {
        busy.current = null;
      }
    }
  }, [fetchPage, apply]);

  const loadMore = useCallback(async () => {
    const after = cursorRef.current;
    if (busy.current || !after) {
      return;
    }
    const mine = generation.current;
    const token = Symbol('more');
    busy.current = token;
    setLoadingMore(true);
    try {
      const page = await fetchPage(after, PAGE);
      if (mine === generation.current) {
        apply(page, appendPage(itemsRef.current ?? [], page.items), cursorOf(page));
        setListError(null);
      }
    } catch (err) {
      if (mine === generation.current) {
        setListError(failText(err, 'Не удалось загрузить ещё'));
      }
    } finally {
      if (busy.current === token) {
        busy.current = null;
      }
      setLoadingMore(false);
    }
  }, [fetchPage, apply]);

  useEffect(() => {
    const timer = window.setInterval(() => {
      if (document.visibilityState === 'visible') {
        void refresh();
      }
    }, 30_000);
    return () => window.clearInterval(timer);
  }, [refresh]);

  useEffect(() => {
    const target = moreRef.current;
    if (!target || !cursor || !showList || typeof IntersectionObserver === 'undefined') {
      return;
    }
    const observer = new IntersectionObserver(
      (entries) => {
        if (entries.some((entry) => entry.isIntersecting)) {
          void loadMore();
        }
      },
      { root: single ? null : listRef.current, rootMargin: '0px 0px 600px 0px' },
    );
    observer.observe(target);
    return () => observer.disconnect();
  }, [cursor, loadMore, single, showList]);

  const patch = useCallback((id: number, change: (item: ListItem) => ListItem) => {
    const list = itemsRef.current;
    if (!list || !list.some((item) => item.ticket.id === id)) {
      return;
    }
    const next = list.map((item) => (item.ticket.id === id ? change(item) : item));
    itemsRef.current = next;
    setItems(next);
  }, []);

  const loadThread = useCallback(
    async (id: number) => {
      try {
        const data = await api<Thread>(`/api/admin/support/tickets/${id}`);
        setThread(data);
        setThreadError(null);
        patch(id, (item) => ({ ...item, unread: false }));
      } catch (err) {
        setThread(null);
        setThreadError(failText(err, 'Обращение не загрузилось'));
      }
    },
    [patch],
  );

  useEffect(() => {
    setReply('');
    setSendError(null);
    setCloseAfter(false);
    setAskDelete(false);
    setAsideOpen(false);
    resetFiles();
    if (selected) {
      setThread((current) => (current?.ticket.id === selected ? current : null));
      setThreadError(null);
      void loadThread(selected);
    } else {
      setThread(null);
      setThreadError(null);
    }
  }, [selected, loadThread, resetFiles]);

  useEffect(() => {
    feedRef.current?.scrollTo({ top: feedRef.current.scrollHeight });
  }, [thread]);

  useLayoutEffect(() => {
    if (single) {
      window.scrollTo(0, selected === null ? listScroll.current : 0);
    }
  }, [single, selected]);

  useEffect(() => {
    if (!notice) {
      return;
    }
    const timer = window.setTimeout(() => setNotice(null), 8_000);
    return () => window.clearTimeout(timer);
  }, [notice]);

  const pick = (id: number) => {
    if (single) {
      listScroll.current = window.scrollY;
    }
    const next = new URLSearchParams(params);
    next.set('ticket', String(id));
    setParams(next);
  };

  const pickFilter = (value: Filter) => {
    const next = new URLSearchParams(params);
    next.set('filter', value);
    next.delete('ticket');
    setPicking(false);
    setParams(next);
  };

  const listHref = `/admin/support?filter=${filter}`;

  const togglePick = (id: number) => {
    const next = new Set(picked);
    if (next.has(id)) {
      next.delete(id);
    } else if (next.size < PICK_MAX) {
      next.add(id);
    }
    setPicked(next);
  };

  const shown = items ?? [];
  const allOn = shown.length > 0 && shown.slice(0, PICK_MAX).every((item) => picked.has(item.ticket.id));
  const toggleAll = () => setPicked(allOn ? new Set() : new Set(shown.slice(0, PICK_MAX).map((item) => item.ticket.id)));
  const pickedIds = shown.filter((item) => picked.has(item.ticket.id)).map((item) => item.ticket.id);
  const canPick = filter !== 'closed' && shown.length > 0;

  const stopPicking = () => {
    setPicking(false);
    setPicked(new Set());
  };

  const afterBulk = (closed: number) => {
    setBulk(null);
    stopPicking();
    setNotice(closed ? `Закрыто: ${count(closed, 'обращение', 'обращения', 'обращений')}` : 'Закрывать было нечего');
    setRound((value) => value + 1);
    if (selected) {
      void loadThread(selected);
    }
  };

  const canSend = (reply.trim() !== '' || files.ids.length > 0) && files.ready;

  const send = async () => {
    if (!thread || !canSend) {
      return;
    }
    setSending(true);
    setSendError(null);
    try {
      const data = await api<Thread>(`/api/admin/support/tickets/${thread.ticket.id}`, {
        method: 'POST',
        body: { body: reply.trim(), close: closeAfter && thread.ticket.status === 'open', files: files.ids },
      });
      setThread(data);
      setReply('');
      setCloseAfter(false);
      files.reset();
      void refresh();
    } catch (err) {
      setSendError(failText(err, 'Ответ не отправился'));
    } finally {
      setSending(false);
    }
  };

  const toggleClosed = async () => {
    if (!thread) {
      return;
    }
    const action = thread.ticket.status === 'closed' ? 'reopen' : 'close';
    try {
      await api(`/api/admin/support/tickets/${thread.ticket.id}/${action}`, { method: 'POST', body: {} });
      await loadThread(thread.ticket.id);
      void refresh();
    } catch (err) {
      setSendError(failText(err, 'Статус не изменился'));
    }
  };

  const removeTicket = async () => {
    if (!thread) {
      return;
    }
    const id = thread.ticket.id;
    setSending(true);
    try {
      await api(`/api/admin/support/tickets/${id}`, { method: 'DELETE' });
      setAskDelete(false);
      setThread(null);
      const rest = (itemsRef.current ?? []).filter((item) => item.ticket.id !== id);
      itemsRef.current = rest;
      setItems(rest);
      const next = new URLSearchParams(params);
      next.delete('ticket');
      setParams(next);
      void refresh();
    } catch (err) {
      setSendError(failText(err, 'Обращение не удалилось'));
    } finally {
      setSending(false);
    }
  };

  const listed = items?.find((i) => i.ticket.id === selected) ?? null;
  const adminEmail = useMemo(() => new Map((thread?.admins ?? []).map((a) => [a.id, a.email])), [thread]);
  const state = thread ? ticketState(thread.ticket, thread.ai) : null;
  const handoff = thread ? handoffLine(thread.ai) : null;
  const handoffAt = thread?.ai?.handoff_at ? Date.parse(thread.ai.handoff_at) : null;
  const handoffIndex = handoff && handoffAt !== null && thread ? thread.messages.filter((m) => Date.parse(m.created_at) <= handoffAt).length : null;
  const known = thread && person?.id === thread.ticket.client_id ? person : null;
  const who = listed ? itemName(listed) : known ? known.name : thread ? `Клиент ${thread.ticket.client_id}` : 'Обращение';
  const handle = listed ? (listed.telegram_username ? `@${listed.telegram_username}` : '') : (known?.handle ?? '');
  const rootClass = ['sp', showThread ? 'page-fit' : '', single && showThread ? 'no-nav' : '', phone ? 'phone' : ''].filter(Boolean).join(' ');

  const chips = (
    <div role="group" aria-label="Какие обращения показать" className={phone ? 'chips flt-scroll' : 'chips'}>
      {FILTERS.map(([key, label, countKey]) => (
        <button key={key} type="button" className="chip" aria-pressed={filter === key} onClick={() => pickFilter(key)}>
          <span>{label}</span>
          <span className="chip-n">{groupDigits(counts[countKey])}</span>
        </button>
      ))}
    </div>
  );

  return (
    <div className={rootClass}>
      {phone ? (
        showThread ? (
          <PhoneHeader title={who} back={{ to: listHref, label: 'Все обращения' }} center>
            {thread ? (
              <button type="button" className="ph-circ press" aria-label="О клиенте" aria-expanded={asideOpen} onClick={() => setAsideOpen(!asideOpen)}>
                <span className="ph-dot">
                  <Icon name="user" size={19} />
                </span>
              </button>
            ) : null}
          </PhoneHeader>
        ) : (
          <>
            <PhoneHeader title="Поддержка" />
            {chips}
          </>
        )
      ) : showList ? (
        <header className="sp-head">
          <h1 className="page-title tight">Поддержка</h1>
          {chips}
        </header>
      ) : null}

      <div className="sp-cols">
        {showList ? (
          <section aria-label="Обращения" className="sp-sec sp-list">
            <label className="sp-search">
              <Icon name="search" size={17} stroke={1.9} />
              <span className="sr-only">Поиск обращений</span>
              <input type="search" value={q} onChange={(e) => setQ(e.target.value)} placeholder="Имя, тема или текст" maxLength={100} />
            </label>
            {picking ? (
              <div role="group" aria-label="Отмеченные обращения" className="sp-tools">
                <input type="checkbox" className="sp-check" checked={allOn} onChange={toggleAll} aria-label="Отметить все показанные" />
                <span className="sp-picked num">Отмечено: {groupDigits(pickedIds.length)}</span>
                <button type="button" className="sp-tool hov" onClick={() => setBulk('picked')} disabled={pickedIds.length === 0}>
                  Закрыть
                </button>
                <button type="button" className="sp-tool hov" onClick={stopPicking}>
                  Отменить
                </button>
              </div>
            ) : canPick || (filter === 'waiting' && counts.waiting > 0) ? (
              <div className="sp-tools">
                {canPick ? (
                  <button type="button" className="sp-tool hov" onClick={() => setPicking(true)}>
                    Выбрать
                  </button>
                ) : null}
                {filter === 'waiting' && counts.waiting > 0 ? (
                  <button type="button" className="sp-tool hov" onClick={() => setBulk('old')}>
                    Закрыть старые
                  </button>
                ) : null}
              </div>
            ) : null}
            {notice ? (
              <p role="status" className="sp-notice">
                {notice}
              </p>
            ) : null}
            <div className="sp-items" ref={listRef}>
              {items === null ? <p className="sp-empty">Загружаем…</p> : null}
              {items && items.length === 0 && !listError ? <p className="sp-empty">{query ? 'Ничего не нашли' : 'Обращений нет'}</p> : null}
              {items?.map((item) => {
                const name = itemName(item);
                const lastAt = item.last_message_at ?? item.ticket.created_at;
                const minutes = minutesSince(lastAt);
                const waitColor =
                  filter === 'waiting' && minutes !== null ? (minutes >= 120 ? 'var(--bad)' : minutes >= 30 ? 'var(--warn)' : 'var(--muted)') : 'var(--muted)';
                const time = filter === 'waiting' && minutes !== null ? spanText(minutes) : shortWhen(lastAt);
                const since =
                  filter === 'waiting' && item.waiting_since && item.waiting_since !== item.last_message_at
                    ? `Ждёт ответа с ${dateLong(item.waiting_since)}, ${timeHM(item.waiting_since)}`
                    : undefined;
                const preview = item.preview ? `${item.last_author === 'support' ? 'Вы: ' : item.last_author === 'ai' ? 'ИИ: ' : ''}${item.preview}` : '';
                const mark = aiMark(item.ai, item.ticket.status, item.last_author);
                const body = (
                  <>
                    <Avatar id={item.ticket.client_id} name={name} className="sp-ava" />
                    <span className="sp-item-main">
                      <span className="sp-item-top">
                        <strong className="sp-ell">{name}</strong>
                        <span className="sp-time num" style={{ color: waitColor }} title={since}>
                          {time}
                        </span>
                      </span>
                      <span className="sp-subj-row">
                        <span className="sp-ell sp-subj">{item.ticket.subject ?? 'Без темы'}</span>
                        {mark ? <span className="sp-ai">{mark}</span> : null}
                      </span>
                      <span className="sp-ell sp-prev">{preview}</span>
                    </span>
                    {item.unread ? (
                      <span className="sp-dot" aria-hidden="true">
                        <span className="sr-only">Не прочитано</span>
                      </span>
                    ) : null}
                  </>
                );
                if (picking) {
                  const on = picked.has(item.ticket.id);
                  return (
                    <label key={item.ticket.id} className={on ? 'sp-item pick on' : 'sp-item pick'}>
                      <input type="checkbox" className="sp-check" checked={on} onChange={() => togglePick(item.ticket.id)} />
                      {body}
                    </label>
                  );
                }
                return (
                  <button
                    key={item.ticket.id}
                    type="button"
                    className="sp-item"
                    aria-pressed={!single && item.ticket.id === selected}
                    onClick={() => pick(item.ticket.id)}
                  >
                    {body}
                  </button>
                );
              })}
              {listError ? (
                <p className="sp-empty err" role="alert">
                  {listError}
                </p>
              ) : null}
              {cursor ? (
                <button ref={moreRef} type="button" className="sp-more hov" onClick={() => void loadMore()} disabled={loadingMore}>
                  {loadingMore ? 'Загружаем…' : 'Показать ещё'}
                </button>
              ) : null}
            </div>
          </section>
        ) : null}

        {showThread ? (
          <section aria-label="Переписка" className="sp-sec sp-thread">
            {!thread && !threadError ? <p className="sp-empty">{selected ? 'Загружаем переписку…' : 'Выберите обращение слева'}</p> : null}
            {threadError ? (
              <div className="sp-empty">
                <p className="err" role="alert">
                  {threadError}
                </p>
                {single && !phone ? (
                  <Link to={listHref} className="sp-open">
                    Все обращения
                  </Link>
                ) : null}
              </div>
            ) : null}
            {thread && state ? (
              <>
                <div className="sp-thead">
                  {single && !phone ? (
                    <Link to={listHref} className="sp-back hov" aria-label="Все обращения">
                      <Icon name="chevron-left" size={20} stroke={2} />
                    </Link>
                  ) : null}
                  <div className="sp-col sp-title">
                    <h2 className="sp-h2">{thread.ticket.subject ?? 'Без темы'}</h2>
                    {phone ? null : (
                      <span className="muted sp-ell">
                        {who}
                        {handle && handle !== who ? ` · ${handle}` : ''}
                      </span>
                    )}
                  </div>
                  <span className="sp-state" style={{ color: state.color }}>
                    <span aria-hidden="true" className="sp-state-dot" style={{ background: state.color }} />
                    {state.text}
                  </span>
                  <span className="sp-thead-btns">
                    {phone ? null : (
                      <button type="button" className="b38 hov sp-info" aria-expanded={asideOpen} onClick={() => setAsideOpen(!asideOpen)}>
                        Клиент
                      </button>
                    )}
                    <button type="button" className="b38 hov" onClick={() => void toggleClosed()}>
                      {thread.ticket.status === 'closed' ? 'Открыть снова' : 'Закрыть'}
                    </button>
                    <button type="button" className="b38 hov" aria-expanded={askDelete} onClick={() => setAskDelete(!askDelete)}>
                      Удалить
                    </button>
                  </span>
                </div>
                {askDelete ? (
                  <div role="status" className="sp-confirm sp-delete">
                    <span>
                      <strong>Удалить обращение?</strong> Переписка и файлы удалятся без возможности вернуть.
                    </span>
                    <div className="sp-delete-btns">
                      <button type="button" className="b34 on-surface" onClick={() => void removeTicket()} disabled={sending}>
                        Удалить обращение
                      </button>
                      <button type="button" className="b34 on-surface" onClick={() => setAskDelete(false)} disabled={sending}>
                        Отменить
                      </button>
                    </div>
                  </div>
                ) : null}
                <div className="sp-feed" ref={feedRef}>
                  {thread.messages.map((m, index) => {
                    const prev = thread.messages[index - 1];
                    const handoffHere = handoff && handoffIndex === index;
                    const newDay = !prev || new Date(prev.created_at).toDateString() !== new Date(m.created_at).toDateString();
                    const mine = m.author === 'support' || m.author === 'ai';
                    const author =
                      m.author === 'support'
                        ? m.admin_id === me.admin.admin_id
                          ? 'вы'
                          : m.admin_id && adminEmail.get(m.admin_id)
                            ? (adminEmail.get(m.admin_id) ?? '').split('@')[0]
                            : 'поддержка'
                        : m.author === 'ai'
                          ? 'ИИ-помощник'
                          : '';
                    const fresh = (minutesSince(m.created_at) ?? 99) < 1;
                    const stamp = `${fresh ? 'сейчас' : timeHM(m.created_at)}${author ? ` · ${author}` : ''}`;
                    return (
                      <div key={`${m.created_at}-${index}`} className="sp-msg-wrap">
                        {handoffHere ? <span className="sp-day">{handoff}</span> : null}
                        {newDay ? (
                          <span className="sp-day">
                            {dayLabel(m.created_at)}
                            {index === 0 && CHANNEL_WORD[thread.ticket.channel] ? ` · ${CHANNEL_WORD[thread.ticket.channel]}` : ''}
                          </span>
                        ) : null}
                        {m.author === 'system' ? (
                          <span className="sp-day">{m.body}</span>
                        ) : (
                          <div className={m.author === 'ai' ? 'sp-msg mine ai' : mine ? 'sp-msg mine' : 'sp-msg'}>
                            {m.body ? <div className="sp-bubble">{m.body}</div> : null}
                            <MessageFiles files={m.attachments} base={FILES} prefix="sp" />
                            <span className="sp-stamp">{stamp}</span>
                          </div>
                        )}
                      </div>
                    );
                  })}
                  {handoff && handoffIndex !== null && handoffIndex >= thread.messages.length ? <span className="sp-day">{handoff}</span> : null}
                </div>
                <div className="sp-reply">
                  {thread.ai?.state === 'active' && thread.ticket.status === 'open' ? (
                    <p className="sp-ai-note">Сейчас отвечает ИИ. Если ответите сами, ИИ в этом обращении замолчит.</p>
                  ) : null}
                  <div role="group" aria-label="Шаблоны ответов" className="sp-templates">
                    {TEMPLATES.map(([label, text]) => (
                      <button key={label} type="button" className="hov" onClick={() => setReply(text)}>
                        {label}
                      </button>
                    ))}
                  </div>
                  <label className="sr-only" htmlFor="sp-reply">
                    Ответ клиенту
                  </label>
                  <textarea
                    id="sp-reply"
                    className="sp-field"
                    rows={3}
                    value={reply}
                    maxLength={4000}
                    onChange={(e) => setReply(e.target.value)}
                    placeholder={
                      thread.ticket.channel === 'bot'
                        ? 'Ответ уйдёт клиенту в Telegram'
                        : thread.ticket.channel === 'miniapp'
                          ? 'Ответ появится в мини-приложении и придёт клиенту в Telegram'
                          : me.menu.some((m) => m.module === 'mail')
                            ? 'Ответ уйдёт клиенту в кабинет и на почту'
                            : 'Ответ появится у клиента в кабинете'
                    }
                    disabled={sending}
                    onKeyDown={(e) => {
                      if (e.key === 'Enter' && (e.metaKey || e.ctrlKey)) {
                        void send();
                      }
                    }}
                    onPaste={(e) => {
                      if (files.enabled && e.clipboardData.files.length) {
                        e.preventDefault();
                        files.add(e.clipboardData.files);
                      }
                    }}
                  />
                  <DraftList files={files} prefix="sp" disabled={sending} />
                  {sendError ? (
                    <p className="err" role="alert">
                      {sendError}
                    </p>
                  ) : null}
                  <div className="sp-send">
                    {thread.ticket.status === 'open' ? (
                      <label className="tf-check">
                        <input type="checkbox" checked={closeAfter} onChange={() => setCloseAfter(!closeAfter)} disabled={sending} />
                        Закрыть после ответа
                      </label>
                    ) : (
                      <span />
                    )}
                    <span className="sp-send-side">
                      <AttachButton files={files} className="sp-clip hov" disabled={sending} />
                      <button type="button" className="btn-main btn" onClick={() => void send()} disabled={sending || !canSend} style={{ height: 42 }}>
                        {sending ? 'Отправляем…' : 'Отправить'}
                      </button>
                    </span>
                  </div>
                </div>
              </>
            ) : null}
          </section>
        ) : null}

        {showThread && thread ? (
          <ClientAside key={thread.ticket.client_id} clientId={thread.ticket.client_id} open={asideOpen} onHide={() => setAsideOpen(false)} onPerson={setPerson} />
        ) : !single ? (
          <aside className="sp-sec sp-aside" aria-label="Клиент" />
        ) : null}
      </div>

      {bulk === 'picked' ? <ClosePicked ids={pickedIds} onCancel={() => setBulk(null)} onDone={afterBulk} /> : null}
      {bulk === 'old' ? <CloseOld onCancel={() => setBulk(null)} onDone={afterBulk} /> : null}
    </div>
  );
}

function ClosePicked({ ids, onCancel, onDone }: { ids: number[]; onCancel: () => void; onDone: (closed: number) => void }) {
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const run = async () => {
    setBusy(true);
    setError(null);
    try {
      const data = await api<{ closed: number }>(CLOSE_MANY, { method: 'POST', body: { ids } });
      onDone(data.closed);
    } catch (err) {
      setError(failText(err, 'Не получилось закрыть'));
      setBusy(false);
    }
  };
  return (
    <div className="dialog-back" role="presentation" onClick={() => (busy ? undefined : onCancel())}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="sp-picked-h" onClick={(e) => e.stopPropagation()}>
        <h2 id="sp-picked-h">Закрыть отмеченные: {groupDigits(ids.length)}</h2>
        <p className="muted">Клиентам ничего не придёт. В журнал действий попадёт одна запись. Закрытое обращение можно открыть снова в списке «Закрытые».</p>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <div className="dialog-actions">
          <button type="button" className="btn-main btn" onClick={() => void run()} disabled={busy || ids.length === 0}>
            {busy ? 'Закрываем…' : `Закрыть ${groupDigits(ids.length)}`}
          </button>
          <button type="button" className="btn-line hov" onClick={onCancel} disabled={busy}>
            Отменить
          </button>
        </div>
      </div>
    </div>
  );
}

function CloseOld({ onCancel, onDone }: { onCancel: () => void; onDone: (closed: number) => void }) {
  const [text, setText] = useState('30');
  const [found, setFound] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const daysValue = readDays(text);

  useEffect(() => {
    setFound(null);
    setError(null);
    if (daysValue === null) {
      return;
    }
    let alive = true;
    const timer = window.setTimeout(() => {
      api<{ count: number }>(`${CLOSE_MANY}?waiting_days=${daysValue}`)
        .then((data) => {
          if (alive) {
            setFound(data.count);
          }
        })
        .catch((err: unknown) => {
          if (alive) {
            setError(failText(err, 'Не получилось посчитать'));
          }
        });
    }, 250);
    return () => {
      alive = false;
      window.clearTimeout(timer);
    };
  }, [daysValue]);

  const run = async () => {
    if (daysValue === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const data = await api<{ closed: number }>(CLOSE_MANY, { method: 'POST', body: { waiting_days: daysValue } });
      onDone(data.closed);
    } catch (err) {
      setError(failText(err, 'Не получилось закрыть'));
      setBusy(false);
    }
  };

  const line =
    daysValue === null
      ? `Укажите число от 1 до ${groupDigits(DAYS_MAX)}`
      : found === null
        ? 'Считаем…'
        : found === 0
          ? 'Таких обращений нет'
          : `Закроется ${count(found, 'обращение', 'обращения', 'обращений')}`;

  return (
    <div className="dialog-back" role="presentation" onClick={() => (busy ? undefined : onCancel())}>
      <div className="dialog" role="dialog" aria-modal="true" aria-labelledby="sp-old-h" onClick={(e) => e.stopPropagation()}>
        <h2 id="sp-old-h">Закрыть старые обращения</h2>
        <p className="muted">
          Закроются обращения, которые ждут ответа дольше указанного срока, если клиент за это время ничего не писал. Клиентам ничего не придёт. В журнал действий попадёт одна запись.
        </p>
        <label className="sp-days-row">
          <span>Ждут ответа дольше, дней</span>
          <input
            className="fld"
            inputMode="numeric"
            value={text}
            maxLength={4}
            aria-invalid={daysValue === null}
            onChange={(e) => setText(e.target.value.replace(/\D/g, ''))}
            disabled={busy}
            autoFocus
          />
        </label>
        <p role="status" className="sp-found">
          {line}
        </p>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <div className="dialog-actions">
          <button type="button" className="btn-main btn" onClick={() => void run()} disabled={busy || daysValue === null || !found}>
            {busy ? 'Закрываем…' : found ? `Закрыть ${groupDigits(found)}` : 'Закрыть'}
          </button>
          <button type="button" className="btn-line hov" onClick={onCancel} disabled={busy}>
            Отменить
          </button>
        </div>
      </div>
    </div>
  );
}

function shortWhen(iso: string): string {
  const minutes = minutesSince(iso) ?? 0;
  if (minutes < 1) {
    return 'сейчас';
  }
  if (minutes < 60 * 24) {
    return spanText(minutes);
  }
  return dateLong(iso);
}

type Ask = '' | 'reset' | 'devices' | 'link';

type Person = { id: number; name: string; handle: string };

function ClientAside({ clientId, open, onHide, onPerson }: { clientId: number; open: boolean; onHide: () => void; onPerson: (person: Person) => void }) {
  const { can } = useSession();
  const writable = can('clients_write');
  const [card, setCard] = useState<ClientCard | null>(null);
  const [details, setDetails] = useState<SubscriptionDetails | null>(null);
  const [device, setDevice] = useState<Device | null>(null);
  const [ask, setAsk] = useState<Ask>('');
  const [busy, setBusy] = useState(false);
  const [note, setNote] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      const data = await api<ClientCard>(`/api/admin/clients/${clientId}`);
      setCard(data);
      const sub = liveSubscriptions(data)[0];
      if (sub) {
        api<SubscriptionDetails>(`/api/admin/subscriptions/${sub.id}`)
          .then(setDetails)
          .catch(() => setDetails(null));
        if (sub.remna) {
          api<Device[]>(`/api/admin/subscriptions/${sub.id}/actions/devices`, { method: 'POST' })
            .then((list) => setDevice([...list].sort((a, b) => Date.parse(b.added_at) - Date.parse(a.added_at))[0] ?? null))
            .catch(() => setDevice(null));
        }
      }
    } catch {
      setCard(null);
    }
  }, [clientId]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (!open) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onHide();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [open, onHide]);

  useEffect(() => {
    if (card) {
      const tg = telegramOf(card);
      const email = emailOf(card);
      onPerson({ id: clientId, name: personName(card), handle: tg?.meta?.username ? `@${tg.meta.username}` : (email?.value ?? '') });
    }
  }, [card, clientId, onPerson]);

  const asideClass = open ? 'sp-sec sp-aside open' : 'sp-sec sp-aside';
  const hide = (
    <button type="button" className="sp-aside-x hov" onClick={onHide} aria-label="Скрыть данные клиента">
      <Icon name="close" size={18} stroke={2} />
    </button>
  );

  if (!card) {
    return (
      <aside className={asideClass} aria-label="Клиент">
        {hide}
      </aside>
    );
  }
  const sub = liveSubscriptions(card)[0] ?? null;
  const live = details?.subscription ?? sub;
  const tariff = tariffOf(card, sub);
  const tg = telegramOf(card);
  const email = emailOf(card);
  const name = personName(card);
  const blocked = card.client.status === 'blocked';
  const mirror = live?.mirror;
  const left = mirror?.expire_at ? daysLeft(mirror.expire_at) : null;
  const expired = !!live && (mirror?.status === 'expired' || (left !== null && left <= 0));
  const limitBytes = mirror?.traffic_limit_bytes && mirror.traffic_limit_bytes > 0 ? mirror.traffic_limit_bytes : null;
  const exhausted = mirror?.status === 'limited';
  const status = !live
    ? { text: 'Нет подписки', color: 'var(--muted)' }
    : blocked
      ? { text: 'Заблокирован', color: 'var(--bad)' }
      : expired
        ? { text: 'Истекла', color: 'var(--muted)' }
        : exhausted
          ? { text: 'Трафик исчерпан', color: 'var(--warn)' }
          : live.kind === 'trial'
            ? { text: 'Пробный', color: 'var(--ink-2)' }
            : left !== null && left <= 3
              ? { text: 'Истекает', color: 'var(--warn)' }
              : { text: 'Активна', color: 'var(--ink-2)' };
  const reset = live ? nextReset(live, tariff) : '';
  const trafficText = !live
    ? ''
    : limitBytes
      ? `${trafficPair(mirror?.traffic_used_bytes ?? 0, limitBytes)}${reset ? `, сброс ${reset}` : `${NBSP}на весь срок`}`
      : details?.traffic_30d_bytes !== null && details?.traffic_30d_bytes !== undefined
        ? `${traffic(details.traffic_30d_bytes)} за 30${NBSP}дней, без лимита`
        : `${traffic(mirror?.traffic_used_bytes ?? 0)}, без лимита`;
  const seen = mirror?.online_at ? `${seenText(mirror.online_at)}${details?.last_node ? ` через ${details.last_node}` : ''}` : live ? 'ни разу' : '';
  const app = device ? [device.app, [platformName(device.platform), device.os_version].filter(Boolean).join(' ')].filter(Boolean).join(', ') : '';

  const act = async (action: string, body: unknown, done: string) => {
    if (!live) {
      return;
    }
    setBusy(true);
    setNote(null);
    try {
      await api(`/api/admin/subscriptions/${live.id}/actions/${action}`, { method: 'POST', body });
      setAsk('');
      setNote(done);
      await load();
    } catch (err) {
      setNote(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const unblock = async () => {
    setBusy(true);
    try {
      await api(`/api/admin/clients/${clientId}/unblock`, { method: 'POST' });
      setNote('Клиент разблокирован');
      await load();
    } catch (err) {
      setNote(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const confirm = (key: Ask, head: string, tail: string, label: string, run: () => void) =>
    ask === key ? (
      <div role="status" className="sp-confirm">
        <span>
          <strong>{head}</strong> {tail}
        </span>
        <button type="button" className="b34 on-surface" onClick={run} disabled={busy}>
          {label}
        </button>
        <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
          Отменить
        </button>
      </div>
    ) : null;

  return (
    <aside className={asideClass} aria-label="Клиент">
      {hide}
      <div className="sp-person">
        <Avatar id={card.client.id} name={name} className="sp-ava big" retry />
        <span className="sp-col" style={{ minWidth: 0 }}>
          <strong style={{ fontSize: 16 }}>{name}</strong>
          <span className="muted" style={{ overflowWrap: 'anywhere' }}>
            {tg?.meta?.username ? `@${tg.meta.username}` : (email?.value ?? (tg ? `Telegram ${tg.value}` : ''))}
          </span>
        </span>
      </div>
      {live ? (
        <div className="sp-col">
          <span className="sp-small muted">{planTitle(live, tariff)}</span>
          <span className="sp-days">
            <strong>{days(expired ? 0 : Math.max(left ?? 0, 0))}</strong>
            <span style={{ color: status.color, fontWeight: 600 }}>{status.text}</span>
          </span>
          <span className="sp-small muted">{mirror?.expire_at ? `${expired ? 'была до' : 'до'} ${dateNumeric(mirror.expire_at)}` : ''}</span>
        </div>
      ) : (
        <p className="muted">{status.text}</p>
      )}
      <div className="sp-col">
        {live ? (
          <>
            <div className="sp-fact">
              <span className="muted">Устройства</span>
              <span className="num">
                {mirror?.devices_count ?? 0}
                {mirror?.hwid_limit ? `${NBSP}из${NBSP}${mirror.hwid_limit}` : ''}
              </span>
            </div>
            {app ? (
              <div className="sp-fact">
                <span className="muted">Приложение</span>
                <span>{app}</span>
              </div>
            ) : null}
            <div className="sp-fact">
              <span className="muted">Трафик</span>
              <span style={{ color: exhausted ? 'var(--warn)' : undefined }}>{trafficText}</span>
            </div>
            <div className="sp-fact">
              <span className="muted">В сети</span>
              <span style={{ color: seen === 'ни разу' ? 'var(--warn)' : undefined }}>{seen}</span>
            </div>
          </>
        ) : null}
        {card.balance && card.balance.minor > 0 ? (
          <div className="sp-fact">
            <span className="muted">Баланс</span>
            <span className="num">{money(card.balance)}</span>
          </div>
        ) : null}
        {card.stats.paid_total ? (
          <div className="sp-fact">
            <span className="muted">Всего оплачено</span>
            <span className="num">{money(card.stats.paid_total)}</span>
          </div>
        ) : null}
      </div>
      {writable ? (
        <div className="sp-col" style={{ gap: 8 }}>
          {blocked ? (
            <button type="button" className="sp-wide hov" onClick={() => void unblock()} disabled={busy}>
              Разблокировать
            </button>
          ) : null}
          {live && exhausted && limitBytes ? (
            <>
              <button type="button" className="sp-wide hov" aria-expanded={ask === 'reset'} onClick={() => setAsk(ask === 'reset' ? '' : 'reset')} disabled={busy}>
                Сбросить трафик
              </button>
              {confirm('reset', 'Обнулить трафик?', `Сейчас ${trafficPair(mirror?.traffic_used_bytes ?? 0, limitBytes)}, станет 0. Отменить будет нельзя.`, 'Сбросить трафик', () => void act('reset-traffic', undefined, 'Трафик обнулён'))}
            </>
          ) : null}
          {live ? (
            <>
              <button type="button" className="sp-wide hov" onClick={() => void act('extend', { days: 3 }, 'Добавлено 3 дня')} disabled={busy}>
                Добавить 3 дня
              </button>
              {live.remna ? (
                <>
                  <button type="button" className="sp-wide hov" aria-expanded={ask === 'devices'} onClick={() => setAsk(ask === 'devices' ? '' : 'devices')} disabled={busy}>
                    Отключить все устройства
                  </button>
                  {confirm(
                    'devices',
                    'Все устройства клиента отключатся.',
                    'Подключатся снова, когда клиент обновит подписку в приложении, ссылка останется прежней.',
                    'Отключить все устройства',
                    () => void act('disconnect-devices', undefined, 'Все устройства отключены'),
                  )}
                  <button type="button" className="sp-wide hov" aria-expanded={ask === 'link'} onClick={() => setAsk(ask === 'link' ? '' : 'link')} disabled={busy}>
                    Выпустить новую ссылку
                  </button>
                  {confirm(
                    'link',
                    'Старая ссылка и все устройства клиента перестанут работать.',
                    tg ? 'Новую ссылку клиент получит в боте и в кабинете.' : 'Новая ссылка будет в кабинете клиента.',
                    'Выпустить новую ссылку',
                    () => void act('reissue-link', undefined, 'Новая ссылка выпущена'),
                  )}
                </>
              ) : null}
            </>
          ) : null}
          {note ? (
            <p role="status" className="sp-small" style={{ color: 'var(--ink-2)' }}>
              {note}
            </p>
          ) : null}
        </div>
      ) : null}
      <Link to={`/admin/clients/${clientId}`} className="sp-open">
        Открыть карточку клиента
        <Icon name="chevron-right" size={16} stroke={2} />
      </Link>
    </aside>
  );
}
