import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { dateLong, days, daysLeft, money, NBSP, periodLabel, timeHM, traffic, trafficPair } from '../../lib/format';
import { Icon } from '../../lib/icons';
import type { Renewal, Subscription } from '../clients/types';
import { LimitsDialog, QrDialog } from './dialogs';
import { historyWhen } from './history';
import { devicesWord, nextReset, platformName, responseText } from './model';
import { RemnaLinkNote } from './RemnaLink';
import { RemoveDaysDialog } from './RemoveDaysDialog';
import type { Connections, Device, SubRequest, SubscriptionDetails, TariffBrief } from './types';

type View = { subscription: Subscription; subscription_url?: string | null } & Partial<SubscriptionDetails>;

type Props = {
  sub: Subscription;
  details: SubscriptionDetails | null;
  tariff: TariffBrief | null;
  clientBlocked: boolean;
  writable: boolean;
  notifies: boolean;
  onView: (view: View) => void;
  onHistory: () => void;
  onExtendHeader: (run: (days: number) => Promise<void>) => void;
  onChangeTariff: () => void;
  onLinked: (text: string) => void;
};

type Ask = '' | 'reset' | 'link' | 'devices' | 'renewal';

const CANCEL_SOURCES: Record<string, string> = {
  client: 'клиентом',
  operator: 'оператором',
  refund: 'возвратом',
  tariff: 'при смене тарифа',
  block: 'при блокировке клиента',
  chargeback: 'после чарджбэка',
  gone: 'панелью: подписки уже нет',
};

export function renewalLine(renewal: Renewal): string {
  const price = renewal.amount ? `${money(renewal.amount)} ${renewal.every}` : renewal.every;
  switch (renewal.status) {
    case 'active':
      return `${renewal.title}, ${price}`;
    case 'cancelling':
      return `${renewal.title}: отключается`;
    case 'failed':
      return `${renewal.title}: не удалось списать`;
    default:
      return `${renewal.title}: отменено`;
  }
}

export function renewalNote(renewal: Renewal): string {
  const since = renewal.legacy
    ? `из старой панели, оформлено ${dateLong(renewal.started_at)}`
    : `оформлено ${dateLong(renewal.started_at)}, списаний ${renewal.charges}`;
  switch (renewal.status) {
    case 'active':
      return [
        renewal.next_charge_at ? `следующее списание около ${dateLong(renewal.next_charge_at)}` : null,
        since,
        renewal.email ? `почта в ${renewal.title} ${renewal.email}` : renewal.problem,
      ]
        .filter(Boolean)
        .join(' · ');
    case 'cancelling':
      return renewal.problem ? `ждём ответа ${renewal.title}: ${renewal.problem}` : `ждём ответа ${renewal.title}`;
    case 'failed':
      return [renewal.ended_at ? `${renewal.title} отменила подписку ${dateLong(renewal.ended_at)}` : null, since].filter(Boolean).join(' · ');
    default: {
      const who = renewal.cancel_source === 'provider' ? `в ${renewal.title}` : (CANCEL_SOURCES[renewal.cancel_source ?? ''] ?? '');
      return [renewal.ended_at ? `отменено ${dateLong(renewal.ended_at)}${who ? ` ${who}` : ''}` : null, since].filter(Boolean).join(' · ');
    }
  }
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function planTitle(sub: Subscription, tariff: TariffBrief | null): string {
  const span = periodLabel(sub.period_days);
  if (sub.kind === 'trial') {
    return span ? `Пробный период${NBSP}· ${span}` : 'Пробный период';
  }
  const name = tariff?.name ?? 'Подписка';
  return span ? `${name}${NBSP}· ${span}` : name;
}

export function SubscriptionBlock({ sub, details, tariff, clientBlocked, writable, notifies, onView, onHistory, onExtendHeader, onChangeTariff, onLinked }: Props) {
  const [added, setAdded] = useState(0);
  const [steps, setSteps] = useState(0);
  const [ask, setAsk] = useState<Ask>('');
  const [busy, setBusy] = useState(false);
  const [message, setMessage] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);
  const [qr, setQr] = useState(false);
  const [limits, setLimits] = useState(false);
  const [removing, setRemoving] = useState(false);
  const [devicesKey, setDevicesKey] = useState(0);

  useEffect(() => {
    setAdded(0);
    setSteps(0);
    setAsk('');
    setMessage(null);
    setError(null);
  }, [sub.id]);

  const base = `/api/admin/subscriptions/${sub.id}/actions`;
  const run = useCallback(
    async (action: string, body: unknown, after: (result: unknown) => void, fallback: string) => {
      setBusy(true);
      setError(null);
      setMessage(null);
      try {
        const result = await api<unknown>(`${base}/${action}`, { method: 'POST', body });
        after(result);
        onHistory();
      } catch (err) {
        setError(failText(err, fallback));
      } finally {
        setBusy(false);
      }
    },
    [base, onHistory],
  );

  const extend = useCallback(
    async (n: number) => {
      await run(
        'extend',
        { days: n },
        (view) => {
          onView(view as View);
          setAdded((value) => value + n);
          setSteps((value) => value + 1);
        },
        'Дни не добавились',
      );
    },
    [onView, run],
  );

  useEffect(() => {
    onExtendHeader(extend);
  }, [extend, onExtendHeader]);

  const removeDays = (n: number) =>
    run(
      'remove-days',
      { days: n },
      (view) => {
        const next = view as View;
        onView(next);
        setAdded(0);
        setSteps(0);
        const until = next.subscription.mirror.expire_at;
        setMessage(until ? `Убрано ${days(n)}, подписка до ${dateLong(until)}` : `Убрано ${days(n)}`);
      },
      'Дни не убрались',
    );

  const undo = () =>
    void run(
      'undo-extend',
      { count: steps },
      (view) => {
        onView(view as View);
        setAdded(0);
        setSteps(0);
        setMessage('Добавленные дни отменены');
      },
      'Отменить не получилось',
    );

  const resetTraffic = () =>
    void run(
      'reset-traffic',
      undefined,
      (view) => {
        onView(view as View);
        setAsk('');
        setMessage('Трафик обнулён');
      },
      'Трафик не сбросился',
    );

  const reissue = () =>
    void run(
      'reissue-link',
      undefined,
      (url) => {
        onView({ subscription: sub, subscription_url: typeof url === 'string' ? url : null });
        setAsk('');
        setDevicesKey((k) => k + 1);
        setMessage('Новая ссылка выпущена, клиент получит её в боте и в кабинете');
      },
      'Новая ссылка не выпустилась',
    );

  const renewal = details?.renewal ?? null;
  const renewalCancel = () =>
    void run(
      'renewal-cancel',
      undefined,
      (next) => {
        onView({ subscription: sub, renewal: (next as Renewal | null) ?? null });
        setAsk('');
        setMessage((next as Renewal | null)?.status === 'cancelling' ? 'Отмена отправлена, ждём ответа платёжной системы' : 'Автопродление отменено, клиент получит сообщение');
      },
      'Автопродление не отменилось',
    );
  const renewalCheck = () =>
    void run(
      'renewal-check',
      undefined,
      (next) => {
        onView({ subscription: sub, renewal: (next as Renewal | null) ?? null });
        setMessage('Сверено с платёжной системой');
      },
      'Сверка не прошла',
    );

  const dropDevices = () =>
    void run(
      'disconnect-devices',
      undefined,
      () => {
        setAsk('');
        setDevicesKey((k) => k + 1);
        setMessage('Все устройства отключены');
      },
      'Устройства не отключились',
    );

  const url = details?.subscription_url ?? null;
  const copy = async () => {
    if (!url) {
      return;
    }
    try {
      await navigator.clipboard.writeText(url);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setError('Браузер не дал скопировать: выделите ссылку вручную');
    }
  };

  const mirror = sub.mirror;
  const now = Date.now();
  const expireAt = mirror.expire_at;
  const expired = mirror.status === 'expired' || (expireAt !== null && Date.parse(expireAt) <= now);
  const left = Math.max(daysLeft(expireAt) ?? 0, 0);
  const limitBytes = mirror.traffic_limit_bytes && mirror.traffic_limit_bytes > 0 ? mirror.traffic_limit_bytes : null;
  const exhausted = mirror.status === 'limited';
  const reset = nextReset(sub, tariff);
  const liveRenewal = renewal && (renewal.status === 'active' || renewal.status === 'cancelling') ? renewal : null;
  const autorenew = liveRenewal
    ? `автопродление через ${liveRenewal.title}`
    : sub.autorenew.enabled
      ? 'автопродление включено'
      : 'автопродление выключено';
  const untilText = expired
    ? expireAt
      ? `закончилась ${dateLong(expireAt)}`
      : 'закончилась'
    : expireAt
      ? `осталось, до ${dateLong(expireAt, true)}${sub.kind === 'trial' ? '' : ` · ${autorenew}`}`
      : '';
  const used = mirror.traffic_used_bytes ?? 0;
  const usedText = limitBytes ? trafficPair(used, limitBytes) : '';
  const trafficValue = limitBytes
    ? reset
      ? `, сброс ${reset}`
      : `${NBSP}на весь срок`
    : details?.traffic_30d_bytes !== null && details?.traffic_30d_bytes !== undefined
      ? `${traffic(details.traffic_30d_bytes)} за 30${NBSP}дней, без лимита`
      : `${traffic(used)} использовано, без лимита`;
  const devicesText =
    mirror.hwid_limit !== null && mirror.hwid_limit !== undefined
      ? `${mirror.devices_count ?? 0}${NBSP}из${NBSP}${mirror.hwid_limit}`
      : `${mirror.devices_count ?? 0}, без ограничения`;
  const squads = details?.squads ?? [];
  const live = !sub.ended_at;
  const canAct = writable && live && !busy;

  return (
    <>
      <section aria-labelledby="sub-h" className="cc-sec cc-subsec">
        <div className="cc-subhead">
          <div className="cc-col">
            <span className="cc-cap">Подписка</span>
            <h2 id="sub-h" className="cc-plan">
              {planTitle(sub, tariff)}
            </h2>
          </div>
          {writable && live ? (
            <button type="button" className="b36 hov" onClick={onChangeTariff} disabled={busy}>
              Сменить тариф
            </button>
          ) : null}
        </div>
        {details ? (
          <RemnaLinkNote
            sub={sub}
            missingSince={details.remna_missing_at}
            writable={writable}
            onLinked={(text) => {
              setDevicesKey((k) => k + 1);
              onLinked(text);
            }}
          />
        ) : null}
        <div className="cc-col" style={{ gap: 12 }}>
          <div className="cc-days">
            <span className="cc-days-n">{days(expired ? 0 : left)}</span>
            <span className="muted">{untilText}</span>
          </div>
          {added > 0 ? (
            <div role="status" className="cc-note">
              <span className="cc-note-text">
                <strong>Добавлено {days(added)}</strong>
                <span>{notifies ? ', клиент получит уведомление в боте через минуту' : ''}</span>
              </span>
              <button type="button" className="b34 on-surface" onClick={undo} disabled={busy}>
                Отменить
              </button>
            </div>
          ) : null}
        </div>
        <div className="cc-col">
          <div className="cc-fact">
            <span className="muted">Трафик</span>
            <span className="cc-fact-v">
              <span style={{ color: exhausted ? 'var(--warn)' : 'inherit' }}>{usedText}</span>
              <span>{trafficValue}</span>
            </span>
          </div>
          <div className="cc-fact">
            <span className="muted">Устройства</span>
            <span className="cc-fact-v">{devicesText}</span>
          </div>
          {renewal ? (
            <div className="cc-fact">
              <span className="muted">Автопродление</span>
              <span className="cc-fact-v cc-renewal">
                <span>{renewalLine(renewal)}</span>
                <span className="muted cc-renewal-note">{renewalNote(renewal)}</span>
                {writable && liveRenewal ? (
                  <span className="cc-btns">
                    <button type="button" className="b34 hov" onClick={renewalCheck} disabled={busy}>
                      Сверить с {renewal.title}
                    </button>
                    {renewal.status === 'active' ? (
                      <button type="button" className="b34 hov" aria-expanded={ask === 'renewal'} onClick={() => setAsk(ask === 'renewal' ? '' : 'renewal')} disabled={busy}>
                        Отменить автопродление
                      </button>
                    ) : null}
                  </span>
                ) : null}
              </span>
            </div>
          ) : null}
          {ask === 'renewal' && renewal ? (
            <div role="status" className="cc-note" style={{ marginTop: 10 }}>
              <span className="cc-note-text wide">
                <strong>{renewal.title} больше не будет списывать деньги.</strong>
                <span>{expireAt ? ` Подписка доработает до ${dateLong(expireAt)}.` : ''} Клиент получит сообщение.</span>
              </span>
              <span className="cc-btns">
                <button type="button" className="b34 on-surface" onClick={renewalCancel} disabled={busy}>
                  Отменить автопродление
                </button>
                <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
                  Оставить
                </button>
              </span>
            </div>
          ) : null}
          {squads.length > 0 ? (
            <div className="cc-fact">
              <span className="muted">Сквады</span>
              <span className="cc-fact-v">{squads.join(', ')}</span>
            </div>
          ) : null}
          <div className="cc-link">
            <span className="muted">Ссылка подписки</span>
            <span className="mono cc-url" title={url ?? undefined}>
              {url ?? (details ? 'нет ссылки' : 'загружаем…')}
            </span>
            <span className="cc-btns">
              <button type="button" className="b34 hov" onClick={() => void copy()} disabled={!url}>
                {copied ? 'Скопировано' : 'Скопировать'}
              </button>
              <button type="button" className="b34 hov" onClick={() => setQr(true)} disabled={!url}>
                QR-код
              </button>
              {writable ? (
                <button type="button" className="b34 hov" aria-expanded={ask === 'link'} onClick={() => setAsk(ask === 'link' ? '' : 'link')} disabled={!canAct}>
                  Выпустить новую ссылку
                </button>
              ) : null}
            </span>
          </div>
          {ask === 'link' ? (
            <div role="status" className="cc-note" style={{ marginTop: 10 }}>
              <span className="cc-note-text wide">
                <strong>Старая ссылка и все устройства клиента перестанут работать.</strong>
                <span> Новую ссылку клиент получит в боте и в кабинете.</span>
              </span>
              <span className="cc-btns">
                <button type="button" className="b34 on-surface" onClick={reissue} disabled={busy}>
                  Выпустить новую ссылку
                </button>
                <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
                  Отменить
                </button>
              </span>
            </div>
          ) : null}
        </div>
        {writable && live ? (
          <div className="cc-col" style={{ gap: 12 }}>
            <div className="cc-btns" style={{ gap: 8 }}>
              {limitBytes && exhausted ? (
                <button type="button" className="b38 hov" aria-expanded={ask === 'reset'} onClick={() => setAsk(ask === 'reset' ? '' : 'reset')} disabled={!canAct}>
                  Сбросить трафик
                </button>
              ) : null}
              <button type="button" className="b38 hov" onClick={() => void extend(7)} disabled={!canAct}>
                +7 дней
              </button>
              <button type="button" className="b38 hov" onClick={() => void extend(30)} disabled={!canAct}>
                +30 дней
              </button>
              <button type="button" className="b38 hov" onClick={() => setRemoving(true)} disabled={!canAct}>
                Убрать дни
              </button>
              {limitBytes && !exhausted ? (
                <button type="button" className="b38 hov" aria-expanded={ask === 'reset'} onClick={() => setAsk(ask === 'reset' ? '' : 'reset')} disabled={!canAct}>
                  Сбросить трафик
                </button>
              ) : null}
              <button type="button" className="b38 hov" onClick={() => setLimits(true)} disabled={!canAct}>
                Изменить лимиты
              </button>
            </div>
            {clientBlocked ? <span className="cap">Продление не снимает блокировку</span> : null}
            {ask === 'reset' ? (
              <div role="status" className="cc-note">
                <span className="cc-note-text">
                  <strong>Обнулить трафик?</strong>
                  <span>
                    {' '}
                    Сейчас {usedText}, станет 0. Отменить будет нельзя.
                  </span>
                </span>
                <span className="cc-btns">
                  <button type="button" className="b34 on-surface" onClick={resetTraffic} disabled={busy}>
                    Сбросить трафик
                  </button>
                  <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
                    Отменить
                  </button>
                </span>
              </div>
            ) : null}
          </div>
        ) : null}
        {message ? (
          <p role="status" className="cap" style={{ color: 'var(--ink-2)' }}>
            {message}
          </p>
        ) : null}
        {error ? (
          <p role="alert" className="err">
            {error}
          </p>
        ) : null}
      </section>

      <DevicesBlock
        sub={sub}
        writable={writable && live}
        reload={devicesKey}
        ask={ask}
        setAsk={setAsk}
        onDropAll={dropDevices}
        busy={busy}
        onHistory={onHistory}
      />

      {qr && url ? <QrDialog url={url} onClose={() => setQr(false)} /> : null}
      {removing ? <RemoveDaysDialog until={sub.mirror.expire_at} onRemove={removeDays} onClose={() => setRemoving(false)} /> : null}
      {limits ? (
        <LimitsDialog
          subscription={sub}
          tariff={tariff}
          trafficLimit={limitBytes}
          onClose={() => setLimits(false)}
          onDone={(view) => {
            setLimits(false);
            onView(view as View);
            setMessage('Лимиты изменены');
            onHistory();
          }}
        />
      ) : null}
    </>
  );
}

type DevicesProps = {
  sub: Subscription;
  writable: boolean;
  reload: number;
  ask: Ask;
  setAsk: (ask: Ask) => void;
  onDropAll: () => void;
  busy: boolean;
  onHistory: () => void;
};

function connectionTime(raw: string): string {
  const parsed = Date.parse(raw);
  return Number.isNaN(parsed) ? raw : timeHM(new Date(parsed).toISOString());
}

function DevicesBlock({ sub, writable, reload, ask, setAsk, onDropAll, busy, onHistory }: DevicesProps) {
  const [devices, setDevices] = useState<Device[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [conns, setConns] = useState<Connections | null>(null);
  const [connsOpen, setConnsOpen] = useState(false);
  const [requests, setRequests] = useState<SubRequest[] | null>(null);
  const [requestsOpen, setRequestsOpen] = useState(false);
  const [requestsError, setRequestsError] = useState<string | null>(null);
  const [dropping, setDropping] = useState<string | null>(null);
  const alive = useRef(true);
  const base = `/api/admin/subscriptions/${sub.id}/actions`;
  const linked = sub.remna !== null;

  useEffect(() => {
    alive.current = true;
    return () => {
      alive.current = false;
    };
  }, []);

  const load = useCallback(async () => {
    if (!linked) {
      setDevices([]);
      return;
    }
    setError(null);
    try {
      const list = await api<Device[]>(`${base}/devices`, { method: 'POST' });
      if (alive.current) {
        setDevices(list);
      }
    } catch (err) {
      if (alive.current) {
        setError(failText(err, 'Устройства не загрузились'));
        setDevices([]);
      }
    }
  }, [base, linked]);

  useEffect(() => {
    setConnsOpen(false);
    setConns(null);
    setRequestsOpen(false);
    setRequests(null);
    void load();
  }, [load, reload]);

  const pollConnections = async () => {
    setConns({ state: 'pending', job_id: '' });
    try {
      let result = await api<Connections>(`${base}/connections`, { method: 'POST', body: {} });
      for (let i = 0; i < 8 && result.state === 'pending' && alive.current; i += 1) {
        await new Promise((resolve) => window.setTimeout(resolve, 1500));
        result = await api<Connections>(`${base}/connections`, { method: 'POST', body: { job_id: result.job_id } });
      }
      if (alive.current) {
        setConns(result);
      }
    } catch (err) {
      if (alive.current) {
        setConns({ state: 'failed' });
        setError(failText(err, 'Подключения не загрузились'));
      }
    }
  };

  const toggleConnections = () => {
    if (connsOpen) {
      setConnsOpen(false);
      return;
    }
    setConnsOpen(true);
    void pollConnections();
  };

  const toggleRequests = async () => {
    if (requestsOpen) {
      setRequestsOpen(false);
      return;
    }
    setRequestsOpen(true);
    setRequestsError(null);
    try {
      const list = await api<SubRequest[]>(`${base}/requests`, { method: 'POST' });
      if (alive.current) {
        setRequests(list);
      }
    } catch (err) {
      if (alive.current) {
        setRequestsError(failText(err, 'Запросы не загрузились'));
      }
    }
  };

  const dropOne = async (device: Device) => {
    setDropping(device.hwid);
    setError(null);
    try {
      await api(`${base}/disconnect-device`, { method: 'POST', body: { hwid: device.hwid } });
      await load();
      onHistory();
    } catch (err) {
      setError(failText(err, 'Устройство не отключилось'));
    } finally {
      setDropping(null);
    }
  };

  const limit = sub.mirror.hwid_limit;
  const countText = devices
    ? limit !== null && limit !== undefined
      ? `${devices.length}${NBSP}из${NBSP}${limit}`
      : String(devices.length)
    : '';
  const connections = conns?.state === 'done' ? conns.connections : [];

  return (
    <section aria-labelledby="dev-h" className="cc-sec cc-list">
      <div className="cc-listhead">
        <h2 id="dev-h" className="cc-h2">
          Устройства <span className="cc-count">{countText}</span>
        </h2>
        {linked ? (
          <span className="cc-btns">
            <button type="button" className="b34 hov" aria-expanded={connsOpen} onClick={toggleConnections} style={connsOpen ? { background: 'var(--surface-2)' } : undefined}>
              Подключения сейчас
            </button>
            {writable ? (
              <button type="button" className="b34 hov" aria-expanded={ask === 'devices'} onClick={() => setAsk(ask === 'devices' ? '' : 'devices')} disabled={busy}>
                Отключить все устройства
              </button>
            ) : null}
          </span>
        ) : null}
      </div>
      {ask === 'devices' ? (
        <div role="status" className="cc-note" style={{ marginBottom: 12 }}>
          <span className="cc-note-text wide">
            <strong>Все устройства клиента отключатся.</strong>
            <span> Подключатся снова, когда клиент обновит подписку в приложении, ссылка останется прежней.</span>
          </span>
          <span className="cc-btns">
            <button type="button" className="b34 on-surface" onClick={onDropAll} disabled={busy}>
              Отключить все устройства
            </button>
            <button type="button" className="b34 on-surface" onClick={() => setAsk('')} disabled={busy}>
              Отменить
            </button>
          </span>
        </div>
      ) : null}
      {connsOpen ? (
        <div className="cc-conns">
          {conns?.state === 'pending' ? <span className="muted">Спрашиваем ноды…</span> : null}
          {conns?.state === 'failed' ? <span className="muted">Ноды не ответили, попробуйте ещё раз</span> : null}
          {conns?.state === 'done' && connections.length === 0 ? <span className="muted">Сейчас не подключён</span> : null}
          {connections.map((c) => (
            <span key={`${c.node}-${c.ip}`}>
              <span>{c.node} · </span>
              <span className="mono">{c.ip}</span>
              <span> · {connectionTime(c.last_seen)}</span>
            </span>
          ))}
        </div>
      ) : null}
      {devices === null ? <p className="cc-empty muted">Загружаем устройства…</p> : null}
      {devices?.map((device) => {
        const meta = [[platformName(device.platform), device.os_version].filter(Boolean).join(' '), device.app].filter(Boolean).join(' · ');
        return (
          <div key={device.hwid} className="cc-dev">
            <span className="cc-dev-icon" aria-hidden="true">
              <Icon name={devicesWord(device.platform)} size={22} stroke={1.8} />
            </span>
            <span className="cc-dev-main">
              <span className="cc-dev-name">{device.title}</span>
              {meta ? <span className="cc-small muted">{meta}</span> : null}
            </span>
            <span className="cc-small muted">Добавлено {dateLong(device.added_at)}</span>
            {writable ? (
              <button type="button" className="b34 hov" aria-label={`Отключить ${device.title}`} onClick={() => void dropOne(device)} disabled={dropping !== null}>
                {dropping === device.hwid ? 'Отключаем…' : 'Отключить'}
              </button>
            ) : null}
          </div>
        );
      })}
      {devices && devices.length === 0 && !error ? (
        <p className="cc-empty muted">{linked ? 'Устройств нет: появятся, когда клиент добавит подписку в приложение' : 'Подписка не связана с Remnawave'}</p>
      ) : null}
      {error ? (
        <p className="cc-empty err" role="alert">
          {error}
        </p>
      ) : null}
      {linked ? (
        <button type="button" className="cc-more" aria-expanded={requestsOpen} onClick={() => void toggleRequests()}>
          Запросы подписки
        </button>
      ) : null}
      {requestsOpen ? (
        <div style={{ marginTop: 8 }}>
          {requests === null && !requestsError ? <p className="cc-req muted">Загружаем…</p> : null}
          {requestsError ? (
            <p className="cc-req err" role="alert">
              {requestsError}
            </p>
          ) : null}
          {requests && requests.length === 0 ? <p className="cc-req muted">Запросов пока не было</p> : null}
          {requests?.map((r, index) => (
            <div key={`${r.at}-${index}`} className="cc-req">
              <span>
                {historyWhen(r.at)}
                {r.app ? ` · ${r.app}` : ''}
                {r.ip ? ' · ' : ''}
              </span>
              {r.ip ? <span className="mono">{r.ip}</span> : null}
              <span> · {responseText(r.response)}</span>
            </div>
          ))}
        </div>
      ) : null}
    </section>
  );
}
