import { useCallback, useEffect, useRef, useState } from 'react';
import { Link, useLocation, useNavigate, useParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { pathPart } from '../../lib/apiPath';
import { dateLong, dateTime, timeHM } from '../../lib/format';
import { changedConfig, initialValues, missingRequired, SchemaFields, type FieldValues, type SecretState } from '../SchemaFields';
import { useSession } from '../session';
import { Switch } from '../ui';
import { CredentialsBlock } from './ExtCredentials';
import { ManagedManage, RuntimeSection, useRuntimeWatch } from './ExtRuntime';
import {
  KEY_TEXT,
  extPageLink,
  savedNote,
  stateNote,
  type Attempt,
  type Credentials,
  type ExtCard,
  type ExtConfig,
  type ExtConfigSaved,
  type Journal,
  type PingResult,
  type Reissued,
  type RuntimeView,
} from './ext';
import { signatureNote } from './package';
import '../form.css';
import '../pages/admins.css';
import '../pages/updates.css';
import './modules.css';
import './ext.css';

const NO_SECRETS: SecretState = { clear: [], replacing: [] };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function attemptLine(a: Attempt): { text: string; tone: 'bad' | 'muted' } {
  const took = a.duration_ms ? ` за ${a.duration_ms} мс` : '';
  if (!a.error) {
    return { text: `Принято, код ${a.http_status ?? '—'}${took}`, tone: 'muted' };
  }
  if (a.status === 'pending' && a.next_attempt_at) {
    return { text: `${a.error}. Повторим в ${timeHM(a.next_attempt_at)}`, tone: 'bad' };
  }
  if (a.status === 'failed') {
    return { text: `${a.error}. Больше не повторяем`, tone: 'bad' };
  }
  return { text: a.error, tone: 'bad' };
}

function Settings({ id, onSaved }: { id: string; onSaved: () => void }) {
  const [cfg, setCfg] = useState<ExtConfig | null>(null);
  const [values, setValues] = useState<FieldValues>({});
  const [secrets, setSecrets] = useState<SecretState>(NO_SECRETS);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);

  const apply = (next: ExtConfig) => {
    setCfg(next);
    setValues(initialValues(next.schema, next.config));
    setSecrets(NO_SECRETS);
  };

  useEffect(() => {
    api<ExtConfig>(`/api/admin/ext-modules/${pathPart(id)}/config`)
      .then(apply)
      .catch((err) => setError(failText(err, 'Настройки не загрузились')));
  }, [id]);

  const save = async () => {
    if (!cfg) {
      return;
    }
    const lacking = missingRequired(cfg.schema, cfg.config, values, secrets);
    if (lacking.length) {
      setError(`Заполните: ${lacking.map((f) => f.label).join(', ')}`);
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const saved = await api<ExtConfigSaved>(`/api/admin/ext-modules/${pathPart(id)}/config`, {
        method: 'PUT',
        body: { config: changedConfig(cfg.schema, cfg.config, values), clear: secrets.clear },
      });
      apply(saved);
      setNote(savedNote(saved.event_queued));
      onSaved();
    } catch (err) {
      setError(failText(err, 'Настройки не сохранились'));
    } finally {
      setBusy(false);
    }
  };

  if (!cfg) {
    return error ? (
      <p className="err" role="alert">
        {error}
      </p>
    ) : null;
  }

  return (
    <section className="card md-form" aria-labelledby="ext-form-h">
      <h2 id="ext-form-h" className="section-title">
        Настройки
      </h2>
      <SchemaFields
        schema={cfg.schema}
        config={cfg.config}
        values={values}
        onChange={setValues}
        secrets={secrets}
        onSecrets={setSecrets}
        busy={busy}
        idPrefix={`ext-${id}`}
      />
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="md-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        {note ? (
          <span role="status" className="muted">
            {note}
          </span>
        ) : null}
      </div>
    </section>
  );
}

function DeliveryLog({ id, version }: { id: string; version: number }) {
  const [journal, setJournal] = useState<Journal | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | 'load' | 'all' | null>(null);
  const [open, setOpen] = useState<number | null>(null);

  const load = useCallback(async () => {
    setBusy('load');
    try {
      setJournal(await api<Journal>(`/api/admin/ext-modules/${pathPart(id)}/deliveries`));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Журнал не загрузился'));
    } finally {
      setBusy(null);
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load, version]);

  const retryAll = async () => {
    setBusy('all');
    try {
      await api(`/api/admin/ext-modules/${pathPart(id)}/deliveries/retry-failed`, { method: 'POST', body: {} });
      await load();
    } catch (err) {
      setError(failText(err, 'События не поставились в очередь'));
    } finally {
      setBusy(null);
    }
  };

  const retry = async (a: Attempt) => {
    setBusy(a.id);
    try {
      await api(`/api/admin/ext-modules/${pathPart(id)}/deliveries/${a.delivery_id}/retry`, { method: 'POST', body: {} });
      await load();
    } catch (err) {
      setError(failText(err, 'Событие не поставилось в очередь'));
    } finally {
      setBusy(null);
    }
  };

  const stats = journal?.stats;
  const summary = stats
    ? [stats.pending ? `в очереди ${stats.pending}` : null, stats.failed ? `не доставлено ${stats.failed}` : null].filter(Boolean).join(' · ')
    : '';

  return (
    <section className="sec" aria-labelledby="ext-log-h">
      <div className="sec-head ext-log-head">
        <span className="ext-log-title">
          <h2 id="ext-log-h" className="section-title">
            Журнал доставки
          </h2>
          <span className="cap">
            Последние попытки отправить события модулю. Неудачные панель повторяет: через 30 секунд, 2 и 10 минут, полчаса, час, 3 и 6 часов.
            {summary ? ` Сейчас: ${summary}.` : ''}
          </span>
        </span>
        <span className="adm-acts">
          {stats && stats.failed > 0 ? (
            <button type="button" className="b34 hov" onClick={() => void retryAll()} disabled={busy !== null}>
              {busy === 'all' ? 'Ставим…' : `Отправить снова все (${stats.failed})`}
            </button>
          ) : null}
          <button type="button" className="b34 hov" onClick={() => void load()} disabled={busy !== null}>
            {busy === 'load' ? 'Обновляем…' : 'Обновить'}
          </button>
        </span>
      </div>
      {error ? (
        <div className="row">
          <span className="err" role="alert">
            {error}
          </span>
        </div>
      ) : null}
      {journal && journal.attempts.length === 0 ? (
        <div className="row">
          <span className="cap">Событий модулю ещё не отправляли.</span>
        </div>
      ) : null}
      {(journal?.attempts ?? []).map((a) => {
        const line = attemptLine(a);
        return (
          <div key={a.id} className="adm-item">
            <div className="row adm-row">
              <span className="adm-who">
                <span className="adm-email">
                  {a.title} <span className="muted">{a.event_id ? `№${a.event_id}` : ''}</span>
                </span>
                <span className={`adm-status ${line.tone}`}>{line.text}</span>
                <span className="adm-status">
                  {dateTime(a.at)} · попытка {a.attempt}
                </span>
              </span>
              <span className="adm-acts">
                {a.response ? (
                  <button type="button" className="b34 hov" aria-expanded={open === a.id} onClick={() => setOpen(open === a.id ? null : a.id)}>
                    Ответ
                  </button>
                ) : null}
                {a.can_retry ? (
                  <button type="button" className="b34 hov" onClick={() => void retry(a)} disabled={busy !== null}>
                    {busy === a.id ? 'Ставим…' : 'Отправить снова'}
                  </button>
                ) : null}
              </span>
            </div>
            {open === a.id && a.response ? (
              <div className="row adm-ask">
                <pre className="mono ext-response">{a.response}</pre>
              </div>
            ) : null}
          </div>
        );
      })}
    </section>
  );
}

export function ExtModule() {
  const { id = '' } = useParams();
  const { can, me, reload } = useSession();
  const navigate = useNavigate();
  const location = useLocation();
  const [card, setCard] = useState<ExtCard | null>(null);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>((location.state as { note?: string } | null)?.note ?? null);
  const [busy, setBusy] = useState<string | null>(null);
  const [asking, setAsking] = useState<'reissue' | 'remove' | null>(null);
  const [issued, setIssued] = useState<Credentials | null>(null);
  const [ping, setPing] = useState<PingResult | null>(null);
  const [version, setVersion] = useState(0);
  const [runtime, setRuntime] = useState<RuntimeView | null>(null);
  const [runtimeError, setRuntimeError] = useState<string | null>(null);
  const canInstall = can('modules') && can('settings');
  const managed = card?.managed ?? false;
  const owner = me.admin.role === 'owner';

  const load = useCallback(async () => {
    try {
      setCard(await api<ExtCard>(`/api/admin/ext-modules/${pathPart(id)}`));
      setError(null);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        setMissing(true);
        return;
      }
      setError(failText(err, 'Модуль не загрузился'));
    }
  }, [id]);

  const loadRuntime = useCallback(async () => {
    try {
      setRuntime(await api<RuntimeView>(`/api/admin/ext-modules/${pathPart(id)}/runtime`));
      setRuntimeError(null);
    } catch (err) {
      setRuntimeError(failText(err, 'Состояние модуля на сервере не загрузилось'));
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  useEffect(() => {
    if (managed) {
      void loadRuntime();
    }
  }, [managed, loadRuntime]);

  const running = useRuntimeWatch(managed ? id : null, runtime, setRuntime);
  const wasRunning = useRef(false);

  useEffect(() => {
    if (wasRunning.current && !running) {
      void load();
    }
    wasRunning.current = running;
  }, [running, load]);

  const toggle = async () => {
    if (!card || busy) {
      return;
    }
    setBusy('toggle');
    setNote(null);
    try {
      setCard(await api<ExtCard>(`/api/admin/ext-modules/${pathPart(id)}`, { method: 'PATCH', body: { enabled: !card.enabled } }));
      if (card.managed) {
        await loadRuntime();
      }
      await reload();
    } catch (err) {
      setError(failText(err, 'Модуль не переключился'));
    } finally {
      setBusy(null);
    }
  };

  const check = async () => {
    setBusy('ping');
    setPing(null);
    setError(null);
    try {
      setPing(await api<PingResult>(`/api/admin/ext-modules/${pathPart(id)}/ping`, { method: 'POST', body: {} }));
      setVersion((n) => n + 1);
      await load();
    } catch (err) {
      setError(failText(err, 'Связь не проверилась'));
    } finally {
      setBusy(null);
    }
  };

  const reissue = async () => {
    setBusy('reissue');
    setError(null);
    try {
      const next = await api<Reissued>(`/api/admin/ext-modules/${pathPart(id)}/credentials`, { method: 'POST', body: {} });
      if (next.credentials) {
        setIssued(next.credentials);
      }
      if (next.runtime) {
        setRuntime(next.runtime);
        setNote('Ключ и секрет обновлены, модуль перезапускается с новыми');
      }
      setAsking(null);
      await load();
    } catch (err) {
      setError(failText(err, 'Новый ключ не выпустился'));
    } finally {
      setBusy(null);
    }
  };

  const remove = async () => {
    setBusy('remove');
    setError(null);
    try {
      await api(`/api/admin/ext-modules/${pathPart(id)}`, { method: 'DELETE' });
      await reload();
      navigate('/admin/modules');
    } catch (err) {
      setError(failText(err, 'Модуль не удалился'));
      setBusy(null);
    }
  };

  if (missing) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        <h1 className="page-title tight">Модуль не найден</h1>
        <p className="muted">Такого внешнего модуля нет: возможно, его удалили.</p>
      </div>
    );
  }

  if (!card) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем модуль…</p>
        )}
      </div>
    );
  }

  const keyBad = card.key.state !== 'active';
  const signed = card.managed && runtime?.package ? signatureNote(runtime.package.signature, card.author) : null;
  const eventsPath = runtime?.run?.events_path ?? null;
  const canPing = card.enabled && card.takes_events;

  return (
    <div className="md-page">
      <Link to="/admin/modules" className="md-back">
        Все модули
      </Link>
      <div className="md-title">
        <h1 className="page-title tight">{card.name}</h1>
        {card.description ? <p>{card.description}</p> : null}
        <span className="md-where">
          {card.managed ? `Модуль из архива · версия ${card.version} · ${card.author}` : `Внешний модуль · версия ${card.version} · автор ${card.author}`}
          {signed ? `, ${signed.text}` : null}
          {card.site ? (
            <>
              {' · '}
              <a href={card.site} target="_blank" rel="noreferrer noopener" className="md-link-inline">
                сайт модуля
              </a>
            </>
          ) : null}
        </span>
      </div>
      {note ? (
        <p role="status" className="sec-notice">
          {note}
        </p>
      ) : null}
      <section className="card md-state" aria-label="Состояние модуля">
        <div className="md-text">
          <span className="md-name">{card.enabled ? 'Модуль включён' : 'Модуль выключен'}</span>
          <span className="md-where">
            {stateNote(card)} Установлен {dateLong(card.installed_at)},{' '}
            {card.installed_by}
          </span>
          {card.warning ? <span className="md-warn">{card.warning}</span> : null}
        </div>
        <Switch on={card.enabled} onToggle={() => void toggle()} label={card.name} disabled={busy !== null || running} />
      </section>
      {card.page && card.enabled && can(card.page.permission) ? (
        <Link to={extPageLink(card.id)} className="md-back">
          Открыть страницу «{card.page.title}»
        </Link>
      ) : null}
      {card.managed && card.inbound ? (
        <section className="card card-pad md-inbound" aria-label="Входящий адрес модуля">
          <span className="md-name">Входящий адрес</span>
          <span className="mono ext-url">{`${window.location.origin}${card.inbound}`}</span>
          <span className="cap">Сюда сторонний сервис шлёт запросы модулю. Панель пересылает их модулю как есть, подпись проверяет сам модуль.</span>
        </section>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {issued ? <CredentialsBlock credentials={issued} onDone={() => setIssued(null)} /> : null}
      {card.managed && runtime ? <RuntimeSection id={card.id} runtime={runtime} onRuntime={setRuntime} /> : null}
      {card.managed && !runtime && runtimeError ? (
        <p className="err" role="alert">
          {runtimeError}
        </p>
      ) : null}
      {card.has_settings ? <Settings id={card.id} onSaved={() => setVersion((n) => n + 1)} /> : null}
      <section className="sec ext-facts" aria-labelledby="ext-access-h">
        <div className="sec-head">
          <h2 id="ext-access-h" className="section-title">
            Доступ
          </h2>
          <span className="cap">
            Что вы подтвердили при установке. Чтобы изменить — загрузите {card.managed ? 'архив новой версии' : 'новый файл описания'}.
          </span>
        </div>
        <div className="row">
          <span className="rl">События</span>
          <span className="rc">
            {card.events.length ? (
              <ul className="ext-list">
                {card.events.map((e) => (
                  <li key={e.kind}>
                    <span className="ext-item">{e.title}</span>
                    <span className="cap">{e.about}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <span className="cap">Не подписан.</span>
            )}
          </span>
        </div>
        <div className="row">
          <span className="rl">Права ключа API</span>
          <span className="rc">
            {card.permissions.length ? (
              <ul className="ext-list">
                {card.permissions.map((p) => (
                  <li key={p.key}>
                    <span className="ext-item">{p.title}</span>
                    <span className="cap">{p.about}</span>
                  </li>
                ))}
              </ul>
            ) : (
              <span className="cap">Только свои настройки.</span>
            )}
          </span>
        </div>
        <div className="row adm-row">
          <span className="adm-who">
            <span className="ext-cred-label">Адрес для событий</span>
            {card.managed ? (
              eventsPath ? (
                <>
                  <span className="mono ext-url">{eventsPath}</span>
                  <span className="cap">Путь внутри модуля: панель шлёт ему события по внутренней сети сервера.</span>
                </>
              ) : runtime ? (
                <span className="cap">Нет: модуль не принимает события.</span>
              ) : null
            ) : card.webhook ? (
              <span className="mono ext-url">{card.webhook}</span>
            ) : (
              <span className="cap">Не указан.</span>
            )}
            {ping ? (
              <span className={`adm-status ${ping.ok ? '' : 'bad'}`} role="status">
                {ping.ok ? `Модуль ответил: код ${ping.http_status} за ${ping.duration_ms} мс` : ping.error}
              </span>
            ) : null}
          </span>
          {canPing ? (
            <span className="adm-acts">
              <button type="button" className="b34 hov" onClick={() => void check()} disabled={busy !== null}>
                {busy === 'ping' ? 'Проверяем…' : 'Проверить связь'}
              </button>
            </span>
          ) : null}
        </div>
        <div className="row adm-row">
          <span className="adm-who">
            <span className="ext-cred-label">Ключ API</span>
            <span className={`adm-status ${keyBad ? 'bad' : ''}`}>
              {card.key.prefix ? <span className="mono">{card.key.prefix}…</span> : null} {KEY_TEXT[card.key.state]}
              {card.key.owner ? ` · от имени ${card.key.owner}` : ''}
            </span>
            <span className="adm-status">
              {card.key.last_used_at ? `Последний запрос: ${dateTime(card.key.last_used_at)}` : 'Модуль ещё не обращался к API'}
            </span>
          </span>
          {canInstall ? (
            <span className="adm-acts">
              <button
                type="button"
                className="b34 hov"
                aria-expanded={asking === 'reissue'}
                onClick={() => setAsking(asking === 'reissue' ? null : 'reissue')}
                disabled={busy !== null}
              >
                Выпустить новые
              </button>
            </span>
          ) : null}
        </div>
        {asking === 'reissue' ? (
          <div className="row adm-ask">
            <span className="cap">
              {card.managed
                ? 'Ключ и секрет обновятся, модуль перезапустится.'
                : 'Панель отзовёт старый ключ API и сменит секрет подписи. Модуль перестанет работать, пока вы не передадите ему новые значения.'}
            </span>
            <span className="adm-acts">
              <button type="button" className="btn-main btn" onClick={() => void reissue()} disabled={busy !== null}>
                {busy === 'reissue' ? 'Выпускаем…' : 'Выпустить ключ и секрет'}
              </button>
              <button type="button" className="btn-line hov" onClick={() => setAsking(null)} disabled={busy !== null}>
                Отменить
              </button>
            </span>
          </div>
        ) : null}
      </section>
      <DeliveryLog id={card.id} version={version} />
      {canInstall && card.managed ? <ManagedManage card={card} runtime={runtime} owner={owner} /> : null}
      {canInstall && !card.managed ? (
        <section className="sec" aria-labelledby="ext-manage-h">
          <div className="sec-head">
            <h2 id="ext-manage-h" className="section-title">
              Обновить или удалить
            </h2>
          </div>
          <div className="row adm-row">
            <span className="adm-who">
              <span className="ext-cred-label">Новая версия</span>
              <span className="adm-status">Загрузите новый файл описания: панель покажет, что изменится, и попросит подтвердить.</span>
            </span>
            <span className="adm-acts">
              <Link to={`/admin/modules/install?update=${encodeURIComponent(card.id)}`} className="b34 hov">
                Обновить из файла
              </Link>
            </span>
          </div>
          <div className="row adm-row">
            <span className="adm-who">
              <span className="ext-cred-label">Удаление</span>
              <span className="adm-status">Ключ API отзывается, секреты и журнал стираются.</span>
            </span>
            <span className="adm-acts">
              <button
                type="button"
                className="b34 bad hov"
                aria-expanded={asking === 'remove'}
                onClick={() => setAsking(asking === 'remove' ? null : 'remove')}
                disabled={busy !== null}
              >
                Удалить модуль
              </button>
            </span>
          </div>
          {asking === 'remove' ? (
            <div className="row adm-ask">
              <span className="cap">Модуль сразу потеряет доступ к панели. Вернуть его можно только новой установкой с новым ключом.</span>
              <span className="adm-acts">
                <button type="button" className="btn-bad" onClick={() => void remove()} disabled={busy !== null}>
                  {busy === 'remove' ? 'Удаляем…' : 'Удалить'}
                </button>
                <button type="button" className="btn-line hov" onClick={() => setAsking(null)} disabled={busy !== null}>
                  Отменить
                </button>
              </span>
            </div>
          ) : null}
        </section>
      ) : null}
    </div>
  );
}
