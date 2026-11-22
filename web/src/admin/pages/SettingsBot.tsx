import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { NBSP, plural, timeHM } from '../../lib/format';
import '../form.css';
import { BotFlood, floodChanged, floodForm, floodInvalid, floodSettings, type FloodForm, type FloodSettings, type FloodStats } from './BotFlood';
import { BotTabs } from './BotTabs';
import { BotUpdates, type Updates, type UpdatesMode } from './BotUpdates';
import { SettingsLayout } from './SettingsLayout';

type Problem = 'bad_token' | 'unreachable' | 'conflict' | 'duplicate' | 'webhook' | 'telegram' | 'internal';

type BotStatus =
  | { state: 'off' }
  | { state: 'no_token' }
  | { state: 'starting' }
  | { state: 'running'; username: string | null; since: string; last_ok_at: string }
  | { state: 'failed'; problem: Problem; detail: string | null; at: string; retry_at: string | null };

type Bot = {
  token_set: boolean;
  token_from_env: boolean;
  username: string | null;
  proxy: string | null;
  status: BotStatus;
  updates: Updates;
  flood: { settings: FloodSettings; stats: FloodStats | null };
};

type Tone = 'ok' | 'quiet' | 'bad' | 'warn';

type Shown = {
  head: string;
  tone: Tone;
  meta?: string;
  hint?: string;
  retry?: boolean;
  takeOver?: boolean;
  link?: string;
};

const FAST_MS = 2000;
const SLOW_MS = 15000;
const FAST_FOR_MS = 20000;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function contact(iso: string): string {
  const seconds = Math.max(0, Math.round((Date.now() - Date.parse(iso)) / 1000));
  if (seconds < 5) {
    return 'только что';
  }
  if (seconds < 60) {
    return `${seconds}${NBSP}${plural(seconds, 'секунду', 'секунды', 'секунд')} назад`;
  }
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) {
    return `${minutes}${NBSP}${plural(minutes, 'минуту', 'минуты', 'минут')} назад`;
  }
  return `в ${timeHM(iso)}`;
}

function retryText(retryAt: string | null): string {
  return retryAt ? `Повторим сами в ${timeHM(retryAt)}` : 'Повторим сами';
}

function describe(status: BotStatus): Shown {
  switch (status.state) {
    case 'running':
      return {
        head: 'Работает',
        tone: 'ok',
        meta: [status.username ? `@${status.username}` : null, `на связи ${contact(status.last_ok_at)}`].filter(Boolean).join(' · '),
        link: status.username ? `https://t.me/${status.username}` : undefined,
      };
    case 'starting':
      return { head: 'Подключаемся к Telegram…', tone: 'quiet' };
    case 'no_token':
      return { head: 'Не запущен: нужен токен', tone: 'quiet', hint: 'Вставьте токен ниже: бот запустится сразу после сохранения' };
    case 'off':
      return {
        head: 'Выключен на этом сервере',
        tone: 'quiet',
        hint: 'В файле .env стоит SN_RUN_BOT=false: бот здесь не запускается. Если так не задумано, уберите эту строку и перезапустите систему',
      };
    case 'failed':
      switch (status.problem) {
        case 'bad_token':
          return {
            head: 'Telegram не принял токен',
            tone: 'bad',
            hint: 'Скопируйте токен в @BotFather заново и замените его ниже. Пока токен прежний, проверяем раз в 10 минут',
          };
        case 'unreachable':
          return {
            head: 'Нет связи с Telegram',
            tone: 'bad',
            hint: `Сервер не достучался до api.telegram.org. ${retryText(status.retry_at)}. Если это надолго, проверьте интернет на сервере: у некоторых хостингов Telegram закрыт`,
            retry: true,
          };
        case 'conflict':
          return {
            head: 'Этот бот уже работает в другой программе',
            tone: 'bad',
            hint: 'Telegram отдаёт сообщения только одной программе. Остановите старого бота или выпустите в @BotFather новый токен и замените его ниже',
            retry: true,
          };
        case 'duplicate':
          return {
            head: 'Бот уже запущен в другом процессе панели',
            tone: 'warn',
            hint: 'Обновления обрабатывает один процесс. Этот ждёт и подхватит бота, если тот остановится. Если так не задумано, оставьте SN_RUN_BOT=true только в одном процессе',
            retry: true,
          };
        case 'webhook':
          return {
            head: status.detail ? `Бот подключён к другому сайту: ${status.detail}` : 'Бот подключён к другому сайту',
            tone: 'warn',
            hint: 'Скорее всего, там работает старый бот. Если забрать бота сюда, тот перестанет получать сообщения',
            takeOver: true,
          };
        case 'telegram':
          return {
            head: 'Telegram ответил ошибкой',
            tone: 'bad',
            hint: [status.detail ? `«${status.detail}».` : null, `${retryText(status.retry_at)}.`].filter(Boolean).join(' '),
            retry: true,
          };
        case 'internal':
          return {
            head: 'Бот остановился из-за внутренней ошибки',
            tone: 'bad',
            hint: `${retryText(status.retry_at)}.`,
            retry: true,
          };
      }
  }
}

export function SettingsBot() {
  const [bot, setBot] = useState<Bot | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [token, setToken] = useState('');
  const [replacing, setReplacing] = useState(false);
  const [username, setUsername] = useState('');
  const [proxy, setProxy] = useState('');
  const [mode, setMode] = useState<UpdatesMode>('polling');
  const [flood, setFlood] = useState<FloodForm | null>(null);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);
  const [busy, setBusy] = useState(false);
  const [askTakeOver, setAskTakeOver] = useState(false);
  const [actionError, setActionError] = useState<string | null>(null);
  const [fastUntil, setFastUntil] = useState(0);
  const touched = useRef(false);
  const proxyTouched = useRef(false);
  const modeTouched = useRef(false);
  const floodTouched = useRef(false);

  const apply = useCallback((data: Bot) => {
    setBot(data);
    if (!touched.current) {
      setUsername(data.username ?? '');
    }
    if (!proxyTouched.current) {
      setProxy(data.proxy ?? '');
    }
    if (!modeTouched.current) {
      setMode(data.updates.mode);
    }
    if (!floodTouched.current) {
      setFlood(floodForm(data.flood.settings));
    }
  }, []);

  const load = useCallback(async () => {
    try {
      const data = await api<Bot>('/api/admin/settings/bot');
      touched.current = false;
      proxyTouched.current = false;
      modeTouched.current = false;
      floodTouched.current = false;
      apply(data);
      setToken('');
      setReplacing(false);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Настройки бота не загрузились'));
    }
  }, [apply]);

  useEffect(() => {
    void load();
  }, [load]);

  const state = bot?.status.state;
  useEffect(() => {
    if (!state) {
      return;
    }
    const fast = state === 'starting' || Date.now() < fastUntil;
    const timer = window.setTimeout(
      async () => {
        try {
          apply(await api<Bot>('/api/admin/settings/bot'));
        } catch {
          setBot((prev) => (prev ? { ...prev } : prev));
        }
      },
      fast ? FAST_MS : SLOW_MS,
    );
    return () => window.clearTimeout(timer);
  }, [bot, state, fastUntil, apply]);

  if (!bot) {
    return (
      <SettingsLayout>
        <BotTabs />
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем настройки бота…</p>
        )}
      </SettingsLayout>
    );
  }

  const clean = username.trim().replace(/^@/, '');
  const nameBad = clean !== '' && !/^[A-Za-z0-9_]{5,32}$/.test(clean);
  const tokenBad = token.trim() !== '' && !/^\d+:[A-Za-z0-9_-]{20,}$/.test(token.trim());
  const proxyBad = proxy.trim() !== '' && !/^(https?|socks5h?):\/\//i.test(proxy.trim());
  const modeChanged = mode !== bot.updates.mode;
  const floodDirty = flood !== null && floodChanged(flood, bot.flood.settings);
  const floodBad = flood !== null && floodInvalid(flood);
  const dirty = token.trim() !== '' || clean !== (bot.username ?? '') || proxy.trim() !== (bot.proxy ?? '') || modeChanged || floodDirty;
  const editingToken = !bot.token_set || replacing;
  const shown = describe(bot.status);

  const save = async () => {
    if (!dirty || busy) {
      return;
    }
    if (nameBad || tokenBad || proxyBad || floodBad) {
      setStatus({ text: 'Исправьте поля, отмеченные красным', bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      const body: Record<string, unknown> = {};
      if (token.trim()) {
        body.token = token.trim();
      }
      if (clean !== (bot.username ?? '')) {
        body.username = clean;
      }
      if (proxy.trim() !== (bot.proxy ?? '')) {
        body.proxy = proxy.trim();
      }
      if (modeChanged) {
        body.updates = mode;
      }
      if (flood && floodDirty) {
        body.flood = floodSettings(flood);
      }
      await api('/api/admin/settings/bot', { method: 'PUT', body });
      setFastUntil(Date.now() + FAST_FOR_MS);
      await load();
      const done = modeChanged
        ? mode === 'webhook'
          ? 'Сохранено, бот переходит на вебхук'
          : 'Сохранено, бот переходит на опрос'
        : body.token
          ? 'Сохранено, бот переподключается'
          : 'Сохранено';
      setStatus({ text: done, bad: false });
    } catch (err) {
      setStatus({ text: failText(err, 'Настройки бота не сохранились'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const dismiss = async () => {
    setBusy(true);
    setActionError(null);
    try {
      apply(await api<Bot>('/api/admin/settings/bot', { method: 'PUT', body: { updates: bot.updates.mode } }));
    } catch (err) {
      setActionError(failText(err, 'Не получилось убрать сообщение'));
    } finally {
      setBusy(false);
    }
  };

  const restart = async (takeOver: boolean) => {
    setBusy(true);
    setActionError(null);
    try {
      apply(await api<Bot>('/api/admin/settings/bot/restart', { method: 'POST', body: { take_over: takeOver } }));
      setAskTakeOver(false);
      setFastUntil(Date.now() + FAST_FOR_MS);
    } catch (err) {
      setActionError(failText(err, takeOver ? 'Забрать бота не получилось' : 'Повторить не получилось'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <SettingsLayout>
      <BotTabs />
      <section className="sec" aria-labelledby="bot-h">
        <div className="sec-head">
          <h2 id="bot-h" className="section-title">
            Telegram-бот
          </h2>
          <span className="muted">Через бота клиенты покупают и продлевают подписку, платят звёздами и пишут в поддержку</span>
        </div>
        <div className="row">
          <span className="rl">Состояние</span>
          <div className="rc">
            <span className="bot-state">
              <strong className={`bot-${shown.tone}`} aria-live="polite">
                {shown.head}
              </strong>
              {shown.meta ? <span className="muted">{shown.meta}</span> : null}
              {shown.link ? (
                <a className="b34 hov bot-open" href={shown.link} target="_blank" rel="noreferrer">
                  Открыть в Telegram
                </a>
              ) : null}
              {shown.retry ? (
                <button type="button" className="b34 hov" onClick={() => void restart(false)} disabled={busy}>
                  Повторить сейчас
                </button>
              ) : null}
              {shown.takeOver && !askTakeOver ? (
                <button type="button" className="b34 hov" onClick={() => setAskTakeOver(true)} disabled={busy}>
                  Забрать бота сюда
                </button>
              ) : null}
            </span>
            {shown.hint ? <span className="cap">{shown.hint}</span> : null}
            {shown.takeOver && askTakeOver ? (
              <div role="status" className="bot-ask">
                <span>
                  <strong>Забрать бота{bot.status.state === 'failed' && bot.status.detail ? ` с ${bot.status.detail}` : ''}?</strong> Старый бот перестанет получать сообщения, а этот начнёт отвечать клиентам.
                </span>
                <span className="bot-ask-btns">
                  <button type="button" className="b34 on-surface" onClick={() => void restart(true)} disabled={busy}>
                    {busy ? 'Забираем…' : 'Забрать бота'}
                  </button>
                  <button type="button" className="b34 on-surface" onClick={() => setAskTakeOver(false)} disabled={busy}>
                    Отменить
                  </button>
                </span>
              </div>
            ) : null}
            {actionError ? (
              <span className="err" role="alert">
                {actionError}
              </span>
            ) : null}
          </div>
        </div>
        <div className="row">
          <label htmlFor="bot-token" className="rl">
            Токен бота
          </label>
          <div className="rc">
            {editingToken ? (
              <input
                id="bot-token"
                className="field mono"
                type="password"
                autoComplete="off"
                spellCheck={false}
                value={token}
                placeholder={bot.token_set ? 'Новый токен' : '123456789:AA…'}
                aria-invalid={tokenBad}
                aria-describedby="bot-token-hint"
                onChange={(e) => {
                  setToken(e.target.value);
                  setStatus(null);
                }}
                disabled={busy}
              />
            ) : (
              <span className="bot-state">
                <strong>{bot.token_from_env ? 'Задан в файле .env' : 'Сохранён'}</strong>
                <button type="button" className="b34 hov" onClick={() => setReplacing(true)} disabled={busy}>
                  Заменить
                </button>
              </span>
            )}
            {editingToken ? (
              <span id="bot-token-hint" className={tokenBad ? 'err' : 'cap'}>
                {tokenBad
                  ? 'Токен выглядит как 123456789:AAH… — скопируйте его из @BotFather целиком'
                  : bot.token_from_env
                    ? 'Токен из админки важнее файла .env: после сохранения бот переключится на него'
                    : 'Выдаёт @BotFather командой /newbot или /token. Бот переподключится сразу после сохранения'}
              </span>
            ) : bot.token_from_env ? (
              <span className="cap">Можно заменить здесь: токен из админки важнее файла</span>
            ) : null}
          </div>
        </div>
        <div className="row">
          <label htmlFor="bot-name" className="rl">
            Имя бота
          </label>
          <div className="rc">
            <input
              id="bot-name"
              className="field"
              value={username}
              placeholder="@stealthnet_bot"
              maxLength={33}
              aria-invalid={nameBad}
              aria-describedby="bot-name-hint"
              onChange={(e) => {
                touched.current = true;
                setUsername(e.target.value);
                setStatus(null);
              }}
              disabled={busy}
              style={{ maxWidth: 320 }}
            />
            <span id="bot-name-hint" className={nameBad ? 'err' : 'cap'}>
              {nameBad ? 'Латиница, цифры и _, от 5 до 32 символов' : 'Подставится само, когда бот подключится. Нужно для ссылок на бота в кабинете, на сайте и в приглашениях'}
            </span>
          </div>
        </div>
        <div className="row">
          <label htmlFor="bot-proxy" className="rl">
            Прокси для Telegram
          </label>
          <div className="rc">
            <input
              id="bot-proxy"
              className="field"
              value={proxy}
              placeholder="socks5://127.0.0.1:1080"
              aria-invalid={proxyBad}
              aria-describedby="bot-proxy-hint"
              onChange={(e) => {
                proxyTouched.current = true;
                setProxy(e.target.value);
                setStatus(null);
              }}
              disabled={busy}
              style={{ maxWidth: 420 }}
            />
            <span id="bot-proxy-hint" className={proxyBad ? 'err' : 'cap'}>
              {proxyBad
                ? 'Адрес начинается с http://, https://, socks5:// или socks5h://'
                : 'Необязательно. socks5://127.0.0.1:1080, socks5h://user:pass@host:1080 или http://user:pass@host:port. Пусто — напрямую или через прокси из .env'}
            </span>
          </div>
        </div>
      </section>

      <BotUpdates
        updates={bot.updates}
        mode={mode}
        busy={busy}
        onMode={(next) => {
          modeTouched.current = true;
          setMode(next);
          setStatus(null);
        }}
        onDismiss={() => void dismiss()}
      />

      {flood ? (
        <BotFlood
          form={flood}
          stats={bot.flood.stats}
          busy={busy}
          onChange={(patch) => {
            floodTouched.current = true;
            setFlood((prev) => (prev ? { ...prev, ...patch } : prev));
            setStatus(null);
          }}
        />
      ) : null}

      <div className="save-row">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button
          type="button"
          className="btn-line hov"
          disabled={busy}
          onClick={() => {
            touched.current = false;
            proxyTouched.current = false;
            modeTouched.current = false;
            floodTouched.current = false;
            setToken('');
            setReplacing(false);
            setUsername(bot.username ?? '');
            setProxy(bot.proxy ?? '');
            setMode(bot.updates.mode);
            setFlood(floodForm(bot.flood.settings));
            setStatus(null);
          }}
        >
          Отменить
        </button>
        <span role="status" className={status?.bad ? 'bad' : undefined}>
          {status?.text ?? (dirty ? 'Есть несохранённые изменения' : '')}
        </span>
      </div>
    </SettingsLayout>
  );
}
