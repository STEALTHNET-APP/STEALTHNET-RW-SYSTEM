import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, days, daysLeft, money, NBSP, traffic, trafficPair } from '../../lib/format';
import { Icon, type IconName } from '../../lib/icons';
import type { Renewal, Subscription } from '../clients/types';
import { PhoneHeader } from '../phone';
import { useSession } from '../session';
import { BalanceDialog, BlockDialog, ConfirmDialog, GrantDialog, LimitsDialog, MessageDialog } from './dialogs';
import { historyLine } from './history';
import { PasswordDialog } from './PasswordDialog';
import { EmailDialog } from './EmailDialog';
import { RefundDialog } from './RefundDialog';
import { devicesWord, emailOf, liveSubscriptions, nextReset, oauthLogins, personName, seenText, tariffOf, telegramOf } from './model';
import { RemnaAddDialog, RemnaLinkNote } from './RemnaLink';
import { planTitle, renewalLine, renewalNote } from './SubscriptionBlock';
import { ClientProtect } from '../protect/ClientProtect';
import type { ClientCard, Device, HistoryPage, SubscriptionDetails } from './types';
import '../home/phone.css';
import './phone.css';
import { registrationText, useClientUtm, UtmValue } from '../utm/ClientSource';
import { Avatar } from '../Avatar';
import { ExtendDialog } from './ExtendDialog';

type View = { subscription: Subscription; subscription_url?: string | null } & Partial<SubscriptionDetails>;
type Toast = { id: number; kind: 'extend' | 'device' | 'info'; head: string; tail?: string; undo?: () => void };
type Dialogs =
  | ''
  | 'message'
  | 'balance'
  | 'grant'
  | 'change'
  | 'extend'
  | 'limits'
  | 'reissue'
  | 'renewal'
  | 'block'
  | 'reset-2fa'
  | 'delete-sub'
  | 'delete'
  | 'password'
  | 'remna-add'
  | 'email';
type Drop = { device: Device; index: number; subId: number; timer: number };
type Tone = 'ink2' | 'warn' | 'muted' | 'bad';
type SheetItem = { key: Dialogs; icon: IconName; label: string };

const TOAST_MS = 6000;
const FIRST_PAGE = 3;
const MORE_PAGE = 20;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function fingerprint(card: ClientCard): string {
  return JSON.stringify(card.subscriptions.map((s) => [s.id, s.tariff_id, s.mirror.expire_at, s.ended_at, s.period_days]));
}

function shortLink(url: string): string {
  return url.length > 38 ? `${url.slice(0, 30)}…${url.slice(-4)}` : url;
}

function appShort(app: string | null): string | null {
  const name = (app ?? '').replace(/\s+\d[\d.]*$/, '').trim();
  return name || null;
}

function restore(list: Device[] | null, drop: { device: Device; index: number }): Device[] {
  const rest = (list ?? []).filter((d) => d.hwid !== drop.device.hwid);
  return [...rest.slice(0, drop.index), drop.device, ...rest.slice(drop.index)];
}

function statusOf(details: SubscriptionDetails | null, sub: Subscription, blocked: boolean, expired: boolean): { text: string; tone: Tone } {
  if (blocked) {
    return { text: 'Заблокирован', tone: 'bad' };
  }
  if (sub.mirror.status === 'limited') {
    return { text: 'Трафик исчерпан', tone: 'warn' };
  }
  if (expired) {
    return { text: 'Истекла', tone: 'muted' };
  }
  if (sub.mirror.status === 'disabled') {
    return { text: 'Отключена', tone: 'bad' };
  }
  if (sub.kind === 'trial') {
    return { text: 'Пробный период', tone: 'ink2' };
  }
  if (details?.status === 'expiring') {
    return { text: 'Истекает', tone: 'warn' };
  }
  return { text: 'Активна', tone: 'ink2' };
}

export function ClientPhone() {
  const { id } = useParams();
  const clientId = Number(id);
  const utm = useClientUtm(clientId);
  const navigate = useNavigate();
  const { can, me } = useSession();
  const writable = can('clients_write');
  const loginOk = can('clients_login');
  const moneyOk = can('money');
  const refundsOk = can('refunds');
  const [refundFor, setRefundFor] = useState<number | null>(null);
  const [card, setCard] = useState<ClientCard | null>(null);
  const [details, setDetails] = useState<SubscriptionDetails | null>(null);
  const [detailsKey, setDetailsKey] = useState(0);
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [devicesFailed, setDevicesFailed] = useState(false);
  const [history, setHistory] = useState<HistoryPage | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState('');
  const [busy, setBusy] = useState(false);
  const [askReset, setAskReset] = useState(false);
  const [toast, setToast] = useState<Toast | null>(null);
  const [dialog, setDialog] = useState<Dialogs>('');
  const [sheet, setSheet] = useState(false);
  const [copied, setCopied] = useState(false);
  const [fullLink, setFullLink] = useState(false);
  const toastRef = useRef<Toast | null>(null);
  const seq = useRef(0);
  const added = useRef(0);
  const steps = useRef(0);
  const drop = useRef<Drop | null>(null);

  const loadCard = useCallback(async () => {
    try {
      const data = await api<ClientCard>(`/api/admin/clients/${clientId}`);
      setCard(data);
      setNote(data.client.note ?? '');
      setError(null);
      return data;
    } catch (err) {
      setError(failText(err, 'Карточка не загрузилась'));
      return null;
    }
  }, [clientId]);

  const loadHistory = useCallback(async () => {
    try {
      setHistory(await api<HistoryPage>(`/api/admin/clients/${clientId}/history?limit=${FIRST_PAGE}`));
    } catch {
      setHistory((prev) => prev ?? { items: [], next: null, total: 0 });
    }
  }, [clientId]);

  useEffect(() => {
    setCard(null);
    setHistory(null);
    void loadCard();
    void loadHistory();
  }, [loadCard, loadHistory]);

  const live = useMemo(() => (card ? liveSubscriptions(card) : []), [card]);
  const sub: Subscription | null = live[0] ?? null;
  const subId = sub?.id ?? null;
  const linked = !!sub?.remna;
  const base = `/api/admin/subscriptions/${subId ?? 0}/actions`;

  useEffect(() => {
    setDetails(null);
    setDevices(null);
    setDevicesFailed(false);
    setAskReset(false);
    if (subId === null) {
      return;
    }
    let alive = true;
    api<SubscriptionDetails>(`/api/admin/subscriptions/${subId}`)
      .then((data) => {
        if (alive) {
          setDetails(data);
          setCard((prev) => (prev ? { ...prev, subscriptions: prev.subscriptions.map((s) => (s.id === data.subscription.id ? data.subscription : s)) } : prev));
        }
      })
      .catch(() => {
        if (alive) {
          setDetails(null);
        }
      });
    if (linked) {
      api<Device[]>(`/api/admin/subscriptions/${subId}/actions/devices`, { method: 'POST' })
        .then((list) => {
          if (alive) {
            setDevices(list);
          }
        })
        .catch(() => {
          if (alive) {
            setDevices([]);
            setDevicesFailed(true);
          }
        });
    } else {
      setDevices([]);
    }
    return () => {
      alive = false;
    };
  }, [subId, linked, detailsKey]);

  useEffect(() => {
    toastRef.current = toast;
    if (!toast) {
      return;
    }
    const timer = window.setTimeout(() => setToast((t) => (t?.id === toast.id ? null : t)), TOAST_MS);
    return () => window.clearTimeout(timer);
  }, [toast]);

  useEffect(() => {
    if (!sheet) {
      return;
    }
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        setSheet(false);
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [sheet]);

  const show = useCallback((t: Omit<Toast, 'id'>) => {
    seq.current += 1;
    setToast({ ...t, id: seq.current });
  }, []);

  const flushDrop = useCallback(
    (keepalive = false) => {
      const pending = drop.current;
      if (!pending) {
        return;
      }
      window.clearTimeout(pending.timer);
      drop.current = null;
      api(`/api/admin/subscriptions/${pending.subId}/actions/disconnect-device`, { method: 'POST', body: { hwid: pending.device.hwid }, keepalive }).then(
        () => void loadHistory(),
        (err: unknown) => {
          setDevices((list) => restore(list, pending));
          setError(failText(err, 'Устройство не отключилось'));
        },
      );
    },
    [loadHistory],
  );

  useEffect(() => {
    const onHide = () => flushDrop(true);
    window.addEventListener('pagehide', onHide);
    return () => {
      window.removeEventListener('pagehide', onHide);
      flushDrop();
    };
  }, [flushDrop]);

  const applyView = useCallback((view: View) => {
    setCard((prev) => (prev ? { ...prev, subscriptions: prev.subscriptions.map((s) => (s.id === view.subscription.id ? view.subscription : s)) } : prev));
    setDetails((prev) => {
      if (!prev) {
        return prev;
      }
      const next = { ...prev, subscription: view.subscription };
      if (view.subscription_url !== undefined) {
        next.subscription_url = view.subscription_url;
      }
      if (view.status) {
        next.status = view.status;
      }
      if (view.progress !== undefined) {
        next.progress = view.progress;
      }
      if (view.renewal !== undefined) {
        next.renewal = view.renewal;
      }
      return next;
    });
  }, []);

  const act = async (action: string, body: unknown, fallback: string): Promise<{ result: unknown } | null> => {
    setBusy(true);
    setError(null);
    try {
      return { result: await api<unknown>(`${base}/${action}`, { method: 'POST', body }) };
    } catch (err) {
      setError(failText(err, fallback));
      return null;
    } finally {
      setBusy(false);
    }
  };

  if (!card) {
    return (
      <div className="pc">
        <PhoneHeader title="Клиент" back={{ to: '/admin/clients', label: 'Назад' }} center />
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="pc-wait pc-muted">Загружаем карточку…</p>
        )}
      </div>
    );
  }

  const name = personName(card);
  const tg = telegramOf(card);
  const email = emailOf(card);
  const blocked = card.client.status === 'blocked';
  const notifies = card.bot_available && !!tg && !card.client.bot_unreachable;
  const tariff = tariffOf(card, sub);
  const mirror = sub?.mirror ?? null;
  const expireAt = mirror?.expire_at ?? null;
  const left = daysLeft(expireAt);
  const expired = !!sub && (mirror?.status === 'expired' || (left !== null && left <= 0));
  const leftDays = expired ? 0 : Math.max(left ?? 0, 0);
  const totalDays = Math.max(details?.progress?.days_total ?? 0, sub?.period_days ?? 0, leftDays);
  const share = expired || totalDays <= 0 ? 0 : Math.min(1, leftDays / totalDays);
  const status = sub ? statusOf(details, sub, blocked, expired) : null;
  const limitBytes = mirror?.traffic_limit_bytes && mirror.traffic_limit_bytes > 0 ? mirror.traffic_limit_bytes : null;
  const limited = mirror?.status === 'limited';
  const used = mirror?.traffic_used_bytes ?? 0;
  const reset = sub && limitBytes ? nextReset(sub, tariff) : '';
  const url = details?.subscription_url ?? null;
  const limit = mirror?.hwid_limit ?? null;
  const subline = (tg ? [tg.meta?.username ? `@${tg.meta.username}` : null, tg.value] : [email?.value ?? null, ...oauthLogins(card).map((l) => `вход через ${l.title}`)]).filter(Boolean).join(' · ');
  const seen = sub
    ? mirror?.online_at
      ? { text: `В сети ${seenText(mirror.online_at)}${details?.last_node ? ` через ${details.last_node}` : ''}`, warn: false }
      : { text: 'Подключений ещё не было', warn: true }
    : null;
  const until = sub
    ? expired
      ? expireAt
        ? `закончилась ${dateLong(expireAt)}`
        : 'закончилась'
      : [
          expireAt ? `до ${dateLong(expireAt)}` : null,
          sub.kind === 'trial'
            ? null
            : details?.renewal && (details.renewal.status === 'active' || details.renewal.status === 'cancelling')
              ? `автопродление через ${details.renewal.title}`
              : `автопродление ${sub.autorenew.enabled ? 'включено' : 'выключено'}`,
        ]
          .filter(Boolean)
          .join(', ')
    : '';
  const trafficMain = limitBytes
    ? trafficPair(used, limitBytes)
    : details?.traffic_30d_bytes !== null && details?.traffic_30d_bytes !== undefined
      ? `${traffic(details.traffic_30d_bytes)} за 30${NBSP}дней, без лимита`
      : `${traffic(used)}, без лимита`;
  const trafficTail = limitBytes && reset ? `, сброс ${reset}` : '';
  const lines = (history?.items ?? []).map((item, index) => historyLine(item, index, card.stats.referrer));
  const deviceCount = devices ? devices.length : (mirror?.devices_count ?? 0);

  const sheetItems: SheetItem[] = [];
  if (can('money') && card.balance) {
    sheetItems.push({ key: 'balance', icon: 'wallet', label: 'Пополнить или списать баланс' });
  }
  if (writable && sub) {
    sheetItems.push({ key: 'change', icon: 'tag', label: 'Сменить тариф' });
    sheetItems.push({ key: 'limits', icon: 'sliders', label: 'Изменить лимиты' });
  }
  if (writable && linked) {
    sheetItems.push({ key: 'reissue', icon: 'refresh', label: 'Выпустить новую ссылку' });
  }
  if (writable && !blocked && (!sub || card.multi_subscriptions)) {
    sheetItems.push({ key: 'remna-add', icon: 'plus', label: 'Добавить подписку из Remnawave' });
  }
  if (loginOk) {
    sheetItems.push({ key: 'email', icon: 'mail', label: email ? 'Изменить почту' : 'Указать почту' });
  }

  const undoExtend = async () => {
    if (!sub || steps.current === 0) {
      return;
    }
    const n = steps.current;
    setToast(null);
    const done = await act('undo-extend', { count: n }, 'Отменить не получилось');
    if (!done) {
      return;
    }
    applyView(done.result as View);
    added.current = 0;
    steps.current = 0;
    show({ kind: 'info', head: 'Добавленные дни отменены' });
    void loadHistory();
  };

  const extend = async (n: number) => {
    if (!sub) {
      return;
    }
    if (toastRef.current?.kind !== 'extend') {
      added.current = 0;
      steps.current = 0;
    }
    const done = await act('extend', { days: n }, 'Дни не добавились');
    if (!done) {
      return;
    }
    applyView(done.result as View);
    added.current += n;
    steps.current += 1;
    show({
      kind: 'extend',
      head: `Добавлено ${days(added.current)}${notifies ? ',' : ''}`,
      tail: notifies ? ' клиент получит уведомление в боте' : undefined,
      undo: () => void undoExtend(),
    });
    void loadHistory();
  };

  const resetTraffic = async () => {
    const done = await act('reset-traffic', undefined, 'Трафик не сбросился');
    if (!done) {
      return;
    }
    applyView(done.result as View);
    setAskReset(false);
    void loadHistory();
  };

  const dropDevice = (device: Device) => {
    if (!sub || !devices) {
      return;
    }
    flushDrop();
    const index = Math.max(
      devices.findIndex((d) => d.hwid === device.hwid),
      0,
    );
    setDevices(devices.filter((d) => d.hwid !== device.hwid));
    const pending: Drop = { device, index, subId: sub.id, timer: window.setTimeout(() => flushDrop(), TOAST_MS) };
    drop.current = pending;
    show({
      kind: 'device',
      head: `Устройство ${device.title} отключено`,
      undo: () => {
        if (drop.current === pending) {
          window.clearTimeout(pending.timer);
          drop.current = null;
          setDevices((list) => restore(list, pending));
        }
        setToast(null);
      },
    });
  };

  const moreHistory = async () => {
    if (!history?.next) {
      return;
    }
    try {
      const page = await api<HistoryPage>(`/api/admin/clients/${clientId}/history?limit=${MORE_PAGE}&after=${history.next}`);
      setHistory((prev) => (prev ? { items: [...prev.items, ...page.items], next: page.next, total: page.total } : page));
    } catch (err) {
      setError(failText(err, 'История не загрузилась'));
    }
  };

  const saveNote = async () => {
    if (note === (card.client.note ?? '')) {
      return;
    }
    const value = note.trim() ? note.trim() : null;
    try {
      await api(`/api/admin/clients/${clientId}/note`, { method: 'PUT', body: { note: value } });
      setCard((prev) => (prev ? { ...prev, client: { ...prev.client, note: value } } : prev));
    } catch (err) {
      setError(failText(err, 'Заметка не сохранилась'));
    }
  };

  const unblock = async () => {
    setBusy(true);
    setError(null);
    try {
      await api(`/api/admin/clients/${clientId}/unblock`, { method: 'POST' });
      await loadCard();
      show({ kind: 'info', head: 'Клиент разблокирован' });
      void loadHistory();
    } catch (err) {
      setError(failText(err, 'Разблокировать не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (!url) {
      return;
    }
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setFullLink(true);
      setError('Браузер не дал скопировать: ссылка показана целиком, выделите её вручную');
    }
  };

  const waitForGrant = async (before: string) => {
    show({ kind: 'info', head: 'Выдаём подписку…' });
    for (let attempt = 0; attempt < 12; attempt += 1) {
      await new Promise((resolve) => window.setTimeout(resolve, 1500));
      const data = await loadCard();
      if (data && fingerprint(data) !== before) {
        setDetailsKey((k) => k + 1);
        show({ kind: 'info', head: 'Готово:', tail: ' подписка обновлена' });
        void loadHistory();
        return;
      }
    }
    show({ kind: 'info', head: 'Заказ создан:', tail: ' подписка появится в течение пары минут' });
    void loadHistory();
  };

  const quick: Array<{ label: string; reset?: boolean; run: () => void }> = [];
  if (sub && writable) {
    const resetButton = limitBytes ? { label: 'Сбросить трафик', reset: true, run: () => setAskReset((v) => !v) } : null;
    if (limited && resetButton) {
      quick.push(resetButton);
    }
    quick.push({ label: `+7${NBSP}дней`, run: () => void extend(7) });
    quick.push({ label: `+30${NBSP}дней`, run: () => void extend(30) });
    if (!limited && resetButton) {
      quick.push(resetButton);
    }
  }

  const blockedReason = !card.bot_available
    ? 'Бот не подключён: сообщение некуда отправить. Подключите бота в настройках.'
    : !tg
      ? 'У клиента нет Telegram: написать можно только тем, кто пришёл через бота.'
      : card.client.bot_unreachable
        ? 'Клиент остановил бота, сообщение не дойдёт.'
        : null;

  return (
    <div className="pc">
      <PhoneHeader title="Клиент" back={{ to: '/admin/clients', label: 'Назад' }} center />

      <div className="pc-who">
        <Avatar id={card.client.id} name={name} className="pc-ava" retry />
        <span className="pc-who-text">
          <h1 className="pc-name">{name}</h1>
          {subline ? <span className="pc-muted">{subline}</span> : null}
          {seen ? <span className={seen.warn ? 'pc-warn' : 'pc-ink2'}>{seen.text}</span> : null}
        </span>
      </div>

      <div className="pc-actions">
        {writable && sub ? (
          <button type="button" className="pc-main press" onClick={() => setDialog('extend')} disabled={busy}>
            Продлить
          </button>
        ) : writable && !blocked ? (
          <button type="button" className="pc-main press" onClick={() => setDialog('grant')}>
            Выдать подписку
          </button>
        ) : null}
        {writable ? (
          <button type="button" className="pc-second press" onClick={() => setDialog('message')}>
            Написать
          </button>
        ) : null}
        {sheetItems.length ? (
          <button type="button" className="pc-circ press" aria-label="Ещё действия" aria-haspopup="dialog" aria-expanded={sheet} onClick={() => setSheet(true)}>
            <Icon name="more" size={22} />
          </button>
        ) : null}
      </div>

      {error ? (
        <p className="err pc-error" role="alert">
          {error}
        </p>
      ) : null}

      {sub && status ? (
        <section className="pc-group" aria-labelledby="pc-sub">
          <h2 id="pc-sub" className="hp-h2">
            Подписка
          </h2>
          <div className="pc-card pc-sub">
            <span className="pc-sub-row">
              <span className="pc-plan">{planTitle(sub, tariff)}</span>
              <span className={`pc-status ${status.tone}`}>{status.text}</span>
            </span>
            {details ? (
              <RemnaLinkNote
                phone
                sub={sub}
                missingSince={details.remna_missing_at}
                writable={writable}
                onLinked={(text) => {
                  show({ kind: 'info', head: text });
                  void loadCard();
                  setDetailsKey((k) => k + 1);
                  void loadHistory();
                }}
              />
            ) : null}
            <span className="pc-days">{days(leftDays)}</span>
            {until ? <span className="pc-small pc-muted">{until}</span> : null}
            <span
              className="pc-bar"
              role="progressbar"
              aria-label="Срок подписки"
              aria-valuemin={0}
              aria-valuemax={100}
              aria-valuenow={Math.round(share * 100)}
              aria-valuetext={expired ? `Подписка ${until}` : `Осталось ${days(leftDays)} из ${totalDays}`}
            >
              <span style={{ width: `${Math.round(share * 100)}%` }} />
            </span>
            {quick.length ? (
              <span className="pc-quick">
                {quick.map((q) => (
                  <button key={q.label} type="button" className="pc-q press" onClick={q.run} disabled={busy} aria-expanded={q.reset ? askReset : undefined}>
                    <span>{q.label}</span>
                  </button>
                ))}
              </span>
            ) : null}
            {blocked ? <span className="pc-small pc-muted">Продление не снимает блокировку</span> : null}
            {askReset && limitBytes ? (
              <div role="status" className="pc-ask">
                <span>
                  <b>Обнулить трафик?</b> Сейчас {trafficPair(used, limitBytes)}, станет 0. Отменить будет нельзя.
                </span>
                <span className="pc-ask-btns">
                  <button type="button" className="pc-ok press" onClick={() => void resetTraffic()} disabled={busy}>
                    Сбросить трафик
                  </button>
                  <button type="button" className="pc-cancel press" onClick={() => setAskReset(false)} disabled={busy}>
                    Отменить
                  </button>
                </span>
              </div>
            ) : null}
          </div>
        </section>
      ) : null}

      <dl className="pc-card pc-facts">
        {sub ? (
          <div className="pc-fact">
            <dt>Трафик</dt>
            <dd>
              <span className={limited ? 'pc-warn' : undefined}>{trafficMain}</span>
              {trafficTail}
            </dd>
          </div>
        ) : null}
        {details?.squads.length ? (
          <div className="pc-fact">
            <dt>Сквады</dt>
            <dd>{details.squads.join(', ')}</dd>
          </div>
        ) : null}
        {card.balance ? (
          <div className="pc-fact">
            <dt>Баланс</dt>
            <dd>{money(card.balance)}</dd>
          </div>
        ) : null}
        {card.stats.paid_total ? (
          <div className="pc-fact">
            <dt>Всего оплачено</dt>
            <dd>
              {money(card.stats.paid_total)}
              {card.stats.payments_count ? ` за ${count(card.stats.payments_count, 'платёж', 'платежа', 'платежей')}` : ''}
            </dd>
          </div>
        ) : null}
        <div className="pc-fact">
          <dt>Регистрация</dt>
          <dd>{registrationText(card.client.registration_source, card.client.created_at)}</dd>
        </div>
        {card.receipt_email ? (
          <div className="pc-fact">
            <dt>Почта для чеков</dt>
            <dd>{card.receipt_email}</dd>
          </div>
        ) : null}
        {utm ? (
          <div className="pc-fact">
            <dt>Метка</dt>
            <dd>
              <UtmValue found={utm} />
            </dd>
          </div>
        ) : null}
        {!sub ? (
          <div className="pc-fact">
            <dt>Подписка</dt>
            <dd className="pc-muted">{blocked ? 'Нет, клиент заблокирован' : 'Нет активной подписки'}</dd>
          </div>
        ) : null}
        {details?.renewal ? (
          <div className="pc-fact">
            <dt>Автопродление</dt>
            <dd>
              {renewalLine(details.renewal)}
              <span className="pc-small pc-muted" style={{ display: 'block' }}>
                {renewalNote(details.renewal)}
              </span>
              {writable && details.renewal.status === 'active' ? (
                <button type="button" className="pc-text press" onClick={() => setDialog('renewal')}>
                  Отменить автопродление
                </button>
              ) : null}
            </dd>
          </div>
        ) : null}
        {url ? (
          <div className="pc-link">
            <span className="pc-link-top">
              <dt>Ссылка подписки</dt>
              <button type="button" className="hp-pill press" onClick={() => void copy()}>
                <span>{copied ? 'Скопировано' : 'Скопировать'}</span>
              </button>
            </span>
            <dd className={fullLink ? 'pc-url mono full' : 'pc-url mono'}>{fullLink ? url : shortLink(url)}</dd>
          </div>
        ) : null}
      </dl>

      {sub ? (
        <section className="pc-group" aria-labelledby="pc-dev">
          <h2 id="pc-dev" className="hp-h2">
            Устройства {deviceCount}
            {limit ? `${NBSP}из${NBSP}${limit}` : ''}
          </h2>
          <div className="pc-card pc-list">
            {devices === null ? <p className="pc-empty">Загружаем устройства…</p> : null}
            {devices && devices.length === 0 ? (
              <p className="pc-empty">
                {devicesFailed ? 'Remnawave не ответил, устройства не загрузились' : linked ? 'Устройств нет: появятся, когда клиент добавит подписку в приложение' : 'Подписка не связана с Remnawave'}
              </p>
            ) : null}
            {devices?.map((d) => {
              const app = appShort(d.app);
              return (
                <div key={d.hwid} className="pc-dev">
                  <span className="pc-dev-icon" aria-hidden="true">
                    <Icon name={devicesWord(d.platform)} size={22} />
                  </span>
                  <span className="pc-dev-body">
                    <span className="pc-dev-text">
                      <span className="pc-dev-name">
                        <b>{d.title}</b>
                        {app ? <span className="pc-ink2">{` · ${app}`}</span> : null}
                      </span>
                      <span className="pc-small pc-muted">Добавлено {dateLong(d.added_at)}</span>
                    </span>
                    {writable ? (
                      <button type="button" className="pc-text press" aria-label={`Отключить ${d.title}`} onClick={() => dropDevice(d)}>
                        Отключить
                      </button>
                    ) : null}
                  </span>
                </div>
              );
            })}
          </div>
        </section>
      ) : null}

      {lines.length ? (
        <section className="pc-group" aria-labelledby="pc-hist">
          <h2 id="pc-hist" className="hp-h2">
            История
          </h2>
          <div className="pc-card pc-list">
            {lines.map((l) => (
              <div key={l.key} className="pc-hist">
                <span className="pc-hist-text">
                  <b>{l.what}</b>
                  <span className="pc-small pc-muted">{l.when}</span>
                  {refundsOk && l.refund ? (
                    <button type="button" className="pc-his-act press" onClick={() => setRefundFor(l.refund ?? null)}>
                      Вернуть деньги
                    </button>
                  ) : null}
                </span>
                {l.sum ? <b className="num pc-sum">{l.sum}</b> : null}
              </div>
            ))}
            {history?.next ? (
              <button type="button" className="pc-morehist press" onClick={() => void moreHistory()}>
                Показать ещё
              </button>
            ) : null}
          </div>
        </section>
      ) : null}

      <ClientProtect phone clientId={clientId} />

      <section className="pc-group">
        <label htmlFor="pc-note" className="hp-h2">
          Заметка
        </label>
        <textarea
          id="pc-note"
          className="pc-note"
          rows={2}
          value={note}
          placeholder="Видят только администраторы"
          onChange={(e) => setNote(e.target.value)}
          onBlur={() => void saveNote()}
          readOnly={!writable}
        />
      </section>

      {writable ? (
        <details className="pc-card pc-danger">
          <summary className="press">
            <span>Опасные действия</span>
            <Icon name="chevron-down" size={20} />
          </summary>
          <div className="pc-danger-grid">
            {blocked ? (
              <button type="button" className="pc-dbtn neutral press" onClick={() => void unblock()} disabled={busy}>
                Разблокировать
              </button>
            ) : (
              <button type="button" className="pc-dbtn press" onClick={() => setDialog('block')}>
                Заблокировать
              </button>
            )}
            {loginOk ? (
              <button type="button" className="pc-dbtn press" onClick={() => setDialog('reset-2fa')} disabled={!card.two_factor.enabled}>
                Сбросить 2FA
              </button>
            ) : null}
            {loginOk && emailOf(card) ? (
              <button type="button" className="pc-dbtn wide press" onClick={() => setDialog('password')}>
                Новый пароль для сайта
              </button>
            ) : null}
            {moneyOk && sub ? (
              <button type="button" className="pc-dbtn wide press" onClick={() => setDialog('delete-sub')}>
                Удалить подписку
              </button>
            ) : null}
            {moneyOk ? (
              <button type="button" className="pc-dbtn wide press" onClick={() => setDialog('delete')}>
                Удалить клиента
              </button>
            ) : null}
          </div>
          <p className="pc-small pc-muted pc-danger-note">
            {card.two_factor.enabled ? `2FA включена${card.two_factor.enabled_at ? ` ${dateLong(card.two_factor.enabled_at)}` : ''}` : '2FA у клиента не включена'}
          </p>
        </details>
      ) : null}

      <div role="status" className="pc-toast-zone">
        {toast ? (
          <div className="pc-toast">
            <span className="pc-toast-text">
              <b>{toast.head}</b>
              {toast.tail ?? ''}
            </span>
            {toast.undo ? (
              <button type="button" className="pc-undo press" onClick={toast.undo}>
                Отменить
              </button>
            ) : null}
          </div>
        ) : null}
      </div>

      {sheet ? (
        <div className="psheet-back" role="presentation" onClick={() => setSheet(false)}>
          <div className="psheet" role="dialog" aria-modal="true" aria-label="Ещё действия" onClick={(event) => event.stopPropagation()}>
            <div className="psheet-list">
              {sheetItems.map((item) => (
                <button
                  key={item.key}
                  type="button"
                  className="psheet-row press"
                  onClick={() => {
                    setSheet(false);
                    setDialog(item.key);
                  }}
                >
                  <Icon name={item.icon} size={21} />
                  <span className="psheet-name">{item.label}</span>
                </button>
              ))}
            </div>
          </div>
        </div>
      ) : null}

      {refundFor !== null ? (
        <RefundDialog
          paymentId={refundFor}
          onClose={() => setRefundFor(null)}
          onDone={(_, text) => {
            setRefundFor(null);
            show({ kind: 'info', head: text });
            void loadCard();
            setDetailsKey((k) => k + 1);
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'message' ? (
        <MessageDialog
          clientId={clientId}
          blockedReason={blockedReason}
          onClose={() => setDialog('')}
          onSent={() => {
            setDialog('');
            show({ kind: 'info', head: 'Сообщение отправлено,', tail: ' бот доставит его в течение минуты' });
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'balance' && card.balance ? (
        <BalanceDialog
          clientId={clientId}
          currency={me.currency}
          balance={card.balance}
          initial="add"
          onClose={() => setDialog('')}
          onDone={(next) => {
            setDialog('');
            setCard((prev) => (prev ? { ...prev, balance: next } : prev));
            show({ kind: 'info', head: 'Баланс изменён:', tail: ` теперь ${money(next)}` });
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'extend' && sub ? (
        <ExtendDialog until={sub.mirror.expire_at} onExtend={(n) => extend(n)} onClose={() => setDialog('')} />
      ) : null}
      {dialog === 'grant' || dialog === 'change' ? (
        <GrantDialog
          clientId={clientId}
          subscriptionId={dialog === 'change' && sub ? sub.id : null}
          currentTariffId={dialog === 'change' && sub ? sub.tariff_id : null}
          currentTrial={dialog === 'change' && sub?.kind === 'trial'}
          held={live.map((s) => ({ id: s.id, tariff_id: s.tariff_id }))}
          onClose={() => setDialog('')}
          onDone={() => {
            setDialog('');
            void waitForGrant(fingerprint(card));
          }}
        />
      ) : null}
      {dialog === 'limits' && sub ? (
        <LimitsDialog
          subscription={sub}
          tariff={tariff}
          trafficLimit={limitBytes}
          onClose={() => setDialog('')}
          onDone={(view) => {
            setDialog('');
            applyView(view as View);
            setDetailsKey((k) => k + 1);
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'reissue' && sub ? (
        <ConfirmDialog
          title="Выпустить новую ссылку"
          text="Старая ссылка перестанет работать, а с ней все устройства клиента. Новую ссылку клиент получит в боте и в кабинете."
          confirm="Выпустить ссылку"
          onClose={() => setDialog('')}
          run={async () => {
            const next = await api<unknown>(`${base}/reissue-link`, { method: 'POST' });
            applyView({ subscription: sub, subscription_url: typeof next === 'string' ? next : null });
            setDialog('');
            setFullLink(false);
            setDetailsKey((k) => k + 1);
            show({ kind: 'info', head: 'Новая ссылка выпущена,', tail: ' клиент получит её в боте и в кабинете' });
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'renewal' && sub && details?.renewal ? (
        <ConfirmDialog
          title="Отменить автопродление"
          text={`${details.renewal.title} больше не будет списывать деньги.${expireAt ? ` Подписка доработает до ${dateLong(expireAt)}.` : ''} Клиент получит сообщение.`}
          confirm="Отменить автопродление"
          onClose={() => setDialog('')}
          run={async () => {
            const next = await api<Renewal | null>(`${base}/renewal-cancel`, { method: 'POST' });
            applyView({ subscription: sub, renewal: next });
            setDialog('');
            show({ kind: 'info', head: next?.status === 'cancelling' ? 'Отмена отправлена,' : 'Автопродление отменено,', tail: next?.status === 'cancelling' ? ' ждём ответа платёжной системы' : ' клиент получит сообщение' });
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'remna-add' ? (
        <RemnaAddDialog
          clientId={clientId}
          phone
          onClose={() => setDialog('')}
          onDone={(text) => {
            setDialog('');
            show({ kind: 'info', head: text });
            void loadCard();
            setDetailsKey((k) => k + 1);
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'block' ? (
        <BlockDialog
          clientId={clientId}
          onClose={() => setDialog('')}
          onDone={() => {
            setDialog('');
            show({ kind: 'info', head: 'Клиент заблокирован' });
            void loadCard();
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'password' && emailOf(card) ? (
        <PasswordDialog clientId={clientId} email={emailOf(card)?.value ?? ''} onClose={() => setDialog('')} onIssued={() => undefined} />
      ) : null}
      {dialog === 'email' ? (
        <EmailDialog
          clientId={clientId}
          current={email}
          onClose={() => setDialog('')}
          onDone={(text) => {
            setDialog('');
            show({ kind: 'info', head: text });
            void loadCard();
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'reset-2fa' ? (
        <ConfirmDialog
          title="Сбросить 2FA"
          text="Клиент войдёт в кабинет на сайте без кода из приложения и сможет заново включить защиту в разделе «Безопасность». Все входы в кабинет на сайте закроются, мини-приложение в Telegram продолжит работать."
          confirm="Сбросить 2FA"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/clients/${clientId}/totp/reset`, { method: 'POST', body: {} });
            setDialog('');
            show({ kind: 'info', head: '2FA сброшена,', tail: ' при следующем входе код не понадобится' });
            await loadCard();
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'delete-sub' && sub ? (
        <ConfirmDialog
          title="Удалить подписку"
          text="Пользователь удалится в Remnawave, ссылка и все устройства перестанут работать. Вернуть подписку будет нельзя, только выдать новую. Оплаты останутся в истории."
          confirm="Удалить подписку"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`${base}/delete`, { method: 'POST', body: {} });
            setDialog('');
            show({ kind: 'info', head: 'Подписка удалена' });
            await loadCard();
            void loadHistory();
          }}
        />
      ) : null}
      {dialog === 'delete' ? (
        <ConfirmDialog
          title="Удалить клиента"
          text="Подписки клиента удалятся в Remnawave, вход в кабинет закроется, профиль исчезнет из списка. Если человек снова напишет боту, он придёт как новый клиент. Оплаты останутся в отчётах."
          confirm="Удалить клиента"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/clients/${clientId}/delete`, { method: 'POST', body: {} });
            navigate('/admin/clients', { replace: true });
          }}
        />
      ) : null}
    </div>
  );
}
