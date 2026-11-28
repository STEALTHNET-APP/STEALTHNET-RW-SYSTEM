import { useCallback, useEffect, useRef, useState, type ChangeEvent, type ReactNode } from 'react';
import { Link, useNavigate, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { pathPart } from '../../lib/apiPath';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { useSession } from '../session';
import { CredentialsBlock } from './ExtCredentials';
import { JobSteps, NO_CONFIRM, OwnerFields, confirmError, useRuntimeWatch, type Confirm, type ConfirmError } from './ExtRuntime';
import {
  extLink,
  installedNote,
  MANIFEST_LIMIT,
  noEventsNote,
  type ExtCard,
  type ExtEvent,
  type ExtPermission,
  type InstallInfo,
  type InstallResult,
  type Moved,
  type PackageCheck,
  type PackageInstalled,
  type PackagePreview,
  type Preview,
  type PreviewResult,
  type Problem,
  type RuntimeView,
  type Uploaded,
} from './ext';
import {
  UPLOAD_PART,
  capital,
  fileKind,
  jobFailure,
  kept,
  plainSummary,
  platformsText,
  runChanges,
  signatureLine,
  signatureNote,
  sizeText,
  volumesText,
  zipBlocked,
  type Blocked,
  type FileKind,
} from './package';
import '../form.css';
import '../pages/admins.css';
import '../pages/updates.css';
import './modules.css';
import './ext.css';

const INTRO =
  'Модуль — отдельная программа. Архив от автора модуля панель проверит и запустит на этом сервере, а модуль, который работает на другом сервере, подключается файлом описания JSON. Перед установкой панель покажет, что модуль получит. Как собрать модуль — в docs/MODULES.md.';

type Picked = { kind: FileKind | null; name: string; size: number };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function readText(file: File): Promise<string> {
  return new Promise((resolve, reject) => {
    const reader = new FileReader();
    reader.onload = () => resolve(typeof reader.result === 'string' ? reader.result : '');
    reader.onerror = () => reject(reader.error);
    reader.readAsText(file);
  });
}

function problemText(p: Problem): string {
  return p.path ? `${p.path}: ${p.message}` : p.message;
}

function discardUpload(id: string) {
  void api(`/api/admin/ext-modules/packages/${pathPart(id)}`, { method: 'DELETE', keepalive: true }).catch(() => undefined);
}

function sendPart(holder: { current: XMLHttpRequest | null }, url: string, part: Blob, progress: (loaded: number) => void): Promise<Uploaded> {
  return new Promise((resolve, reject) => {
    const request = new XMLHttpRequest();
    holder.current = request;
    request.open('POST', url);
    request.setRequestHeader('accept', 'application/json');
    request.setRequestHeader('content-type', 'application/zip');
    request.upload.onprogress = (e) => progress(e.loaded);
    request.onload = () => {
      holder.current = null;
      let data: unknown = null;
      try {
        data = JSON.parse(request.responseText);
      } catch {
        data = null;
      }
      if (request.status >= 200 && request.status < 300 && data && typeof data === 'object' && 'id' in data) {
        resolve(data as Uploaded);
        return;
      }
      const err = (data as { error?: { code?: string; message?: string } } | null)?.error;
      reject(new ApiError(request.status, err?.code ?? `http.${request.status}`, err?.message ?? `Архив не загрузился (ошибка ${request.status})`));
    };
    request.onerror = () => {
      holder.current = null;
      reject(new ApiError(0, 'network', 'Загрузка оборвалась. Проверьте интернет и попробуйте ещё раз'));
    };
    request.onabort = () => {
      holder.current = null;
      reject(new ApiError(0, 'aborted', 'Загрузка отменена'));
    };
    request.send(part);
  });
}

function ChangeMark({ change }: { change: ExtEvent['change'] }) {
  if (change === 'added') {
    return <span className="ext-new">новое</span>;
  }
  return null;
}

function EventList({ events, empty }: { events: ExtEvent[]; empty: string }) {
  const shown = kept(events);
  const gone = events.filter((e) => e.change === 'removed');
  return (
    <div className="rc">
      {shown.length === 0 ? <span className="cap">{empty}</span> : null}
      {shown.length ? (
        <ul className="ext-list">
          {shown.map((e) => (
            <li key={e.kind}>
              <span className="ext-item">
                {e.title} <ChangeMark change={e.change} />
              </span>
              <span className="cap">{e.about}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {gone.length ? <span className="cap">Больше не будет приходить: {gone.map((e) => e.title).join(', ')}.</span> : null}
    </div>
  );
}

function PermissionList({ permissions, empty }: { permissions: ExtPermission[]; empty: string }) {
  const shown = kept(permissions);
  const gone = permissions.filter((p) => p.change === 'removed');
  return (
    <div className="rc">
      {shown.length === 0 ? <span className="cap">{empty}</span> : null}
      {shown.length ? (
        <ul className="ext-list">
          {shown.map((p) => (
            <li key={p.key}>
              <span className="ext-item">
                {p.title} <ChangeMark change={p.change} />
              </span>
              <span className="cap">{p.about}</span>
            </li>
          ))}
        </ul>
      ) : null}
      {gone.length ? <span className="cap">Модуль теряет права: {gone.map((p) => p.title).join(', ')}.</span> : null}
    </div>
  );
}

function MovedNote({ moved, address }: { moved: Moved[]; address: Moved['address'] }) {
  const change = moved.find((m) => m.address === address);
  if (!change) {
    return null;
  }
  const now =
    address === 'webhook'
      ? change.before
        ? `Сейчас события уходят на ${change.before}.`
        : 'Сейчас события не уходят.'
      : change.before
        ? `Сейчас страница открывается с ${change.before}.`
        : 'Сейчас страницы нет.';
  return <span className="md-warn">Адрес меняется. {now} Проверьте, что новый адрес принадлежит автору модуля.</span>;
}

function PreviewCard({ preview }: { preview: Preview }) {
  return (
    <section className="sec ext-facts" aria-labelledby="ext-what-h">
      <div className="sec-head">
        <h2 id="ext-what-h" className="section-title">
          Что получит модуль
        </h2>
        {preview.sensitive ? (
          <span className="md-warn">
            Модуль сможет менять данные и работать с деньгами через API, как администратор с этими правами. Ставьте модули только от тех, кому доверяете.
          </span>
        ) : (
          <span className="cap">Права ключа API модуля не шире ваших. Изменить их можно только новым файлом описания и повторным подтверждением.</span>
        )}
      </div>
      <div className="row">
        <span className="rl">События</span>
        <EventList events={preview.events} empty="Нет: модуль не подписан на события." />
      </div>
      <div className="row">
        <span className="rl">Права ключа API</span>
        <PermissionList permissions={preview.permissions} empty="Нет: ключ модуля сможет только читать свои настройки." />
      </div>
      <div className="row">
        <span className="rl">Адрес для событий</span>
        <span className="rc">
          {preview.webhook ? <span className="mono ext-url">{preview.webhook}</span> : <span className="cap">Не указан: события не приходят.</span>}
          <MovedNote moved={preview.moved} address="webhook" />
        </span>
      </div>
      <div className="row">
        <span className="rl">Страница в админке</span>
        <span className="rc">
          {preview.page ? (
            <>
              <span>«{preview.page.title}»</span>
              <span className="mono ext-url">{preview.page.url}</span>
              <span className="cap">Открывается во встроенном окне у тех, у кого есть право «{preview.page.permission_title}».</span>
            </>
          ) : (
            <span className="cap">Нет.</span>
          )}
          <MovedNote moved={preview.moved} address="page" />
        </span>
      </div>
      <div className="row">
        <span className="rl">Настройки</span>
        <span className="rc">
          {preview.settings.length ? <span>{preview.settings.join(', ')}</span> : <span className="cap">Нет.</span>}
        </span>
      </div>
    </section>
  );
}

function CopyCommand({ text }: { text: string }) {
  const [copied, setCopied] = useState(false);
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 1600);
    } catch {
      setCopied(false);
    }
  };
  return (
    <span className="mz-cmd">
      <code className="mono">{text}</code>
      <button type="button" className="b34 hov" onClick={() => void copy()}>
        {copied ? 'Скопировано' : 'Скопировать'}
      </button>
    </span>
  );
}

function BlockedSection({ blocked, children }: { blocked: Blocked; children: ReactNode }) {
  return (
    <section className="sec" aria-labelledby="mz-blocked-h">
      <div className="sec-head">
        <h2 id="mz-blocked-h" className="section-title">
          {blocked.title}
        </h2>
        <span className="cap">{blocked.about}</span>
      </div>
      {blocked.steps.length ? (
        <div className="row">
          <div className="rc">
            <span className="strong">Что можно сделать</span>
            <ol className="mz-ol">
              {blocked.steps.map((step) => (
                <li key={step.text}>
                  {step.text}
                  {step.command ? <CopyCommand text={step.command} /> : null}
                </li>
              ))}
            </ol>
          </div>
        </div>
      ) : null}
      {children}
    </section>
  );
}

function ZipSummary({ preview, updating }: { preview: PackagePreview; updating: boolean }) {
  const note = signatureNote(preview.package.signature, preview.author);
  const changes = runChanges(preview);
  return (
    <section className="card mz-summary" aria-labelledby="mz-name">
      <h2 id="mz-name" className="mz-title">
        {preview.name}
      </h2>
      <p className="mz-meta">
        Версия {preview.version} · {note.ok ? preview.author : `автор, как указано в архиве: ${preview.author}`} ·{' '}
        <span className={note.ok ? 'ok' : undefined}>{note.text}</span>
        {preview.site ? (
          <>
            {' · '}
            <a href={preview.site} target="_blank" rel="noreferrer noopener" className="md-link-inline">
              сайт модуля
            </a>
          </>
        ) : null}
      </p>
      {preview.description ? <p className="mz-desc">{preview.description}</p> : null}
      <p className="mz-plain">{plainSummary(preview)}</p>
      {preview.sensitive ? (
        <p className="md-warn">
          Модуль сможет менять данные и работать с деньгами через API, как администратор с этими правами. Ставьте модули только от тех, кому доверяете.
        </p>
      ) : null}
      {updating && preview.installed ? (
        <div className="mz-changes">
          <span className="strong">Что меняется</span>
          {changes.length ? (
            <ul className="ext-list">
              {changes.map((c) => (
                <li key={c.label}>
                  <span>
                    <span className="ext-item">{c.label}:</span> было {c.before}, станет {c.after}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <span className="cap">Версия {preview.version} уже установлена, запуск не меняется.</span>
          )}
          <span className="cap">Данные модуля останутся. Если новая версия не заработает, вернётся {preview.installed.version}.</span>
        </div>
      ) : null}
    </section>
  );
}

function DetailRows({ preview }: { preview: PackagePreview }) {
  const { run } = preview;
  const pkg = preview.package;
  const folders = volumesText(run.volumes);
  const many = run.volumes.length > 1;
  return (
    <>
      <div className="row">
        <span className="rl">Данные</span>
        <span className="rc">
          {folders ? (
            <>
              <span>{capital(folders)}</span>
              <span className="cap">
                {many
                  ? 'Остаются при обновлении. При удалении модуля стираются, только если вы это отметите.'
                  : 'Остаётся при обновлении. При удалении модуля стирается, только если вы это отметите.'}
              </span>
            </>
          ) : (
            <span>Нет: модуль ничего не хранит</span>
          )}
        </span>
      </div>
      <div className="row">
        <span className="rl">Доступ снаружи</span>
        <span className="rc">
          {preview.inbound ? (
            <>
              <span>Только через панель</span>
              <span className="mono ext-url">{`${window.location.origin}${preview.inbound}`}</span>
              <span className="cap">Портов модуль не открывает. Сторонние сервисы шлют ему запросы на этот адрес, панель пересылает их модулю, а подпись проверяет сам модуль.</span>
            </>
          ) : (
            <>
              <span>Нет</span>
              <span className="cap">{preview.page ? 'Портов модуль не открывает, страница работает через панель.' : 'Портов модуль не открывает.'}</span>
            </>
          )}
        </span>
      </div>
      {preview.client_card ? (
        <div className="row">
          <span className="rl">Карточка клиента</span>
          <span className="rc">
            <span>Раздел «{preview.client_card.title}»</span>
            <span className="cap">Видят администраторы с правом «{preview.client_card.permission_title}».</span>
          </span>
        </div>
      ) : null}
      {preview.bot ? (
        <div className="row">
          <span className="rl">Кнопка в боте</span>
          <span className="rc">
            <span>«{preview.bot.button}»</span>
            <span className="cap">В главном меню бота. Экран за кнопкой показывает модуль, ему уходят номер клиента и Telegram ID.</span>
          </span>
        </div>
      ) : null}
      <div className="row">
        <span className="rl">События</span>
        <EventList events={preview.events} empty={noEventsNote(run)} />
      </div>
      <div className="row">
        <span className="rl">Права ключа API</span>
        <PermissionList permissions={preview.permissions} empty="Нет: ключ читает только настройки модуля." />
      </div>
      <div className="row">
        <span className="rl">Настройки</span>
        <span className="rc">
          {preview.settings.length ? <span>{preview.settings.join(', ')}</span> : <span className="cap">Нет.</span>}
        </span>
      </div>
      <div className="row">
        <span className="rl">Архив</span>
        <span className="rc">
          <span>
            {platformsText(pkg.platforms)}, образ {sizeText(pkg.image_bytes)}
          </span>
          <span className="cap">{signatureLine(pkg.signature)}</span>
        </span>
      </div>
    </>
  );
}

function Decide({
  preview,
  updating,
  totp,
  confirm,
  onConfirm,
  trusted,
  onTrusted,
  busy,
  error,
  onInstall,
}: {
  preview: PackagePreview;
  updating: boolean;
  totp: boolean;
  confirm: Confirm;
  onConfirm: (next: Confirm) => void;
  trusted: boolean;
  onTrusted: (next: boolean) => void;
  busy: boolean;
  error: ConfirmError | null;
  onInstall: () => void;
}) {
  const signature = preview.package.signature;
  const unverified = signature.state !== 'verified';
  return (
    <section className="sec mz-decide" aria-labelledby="mz-decide-h">
      <div className="sec-head">
        <h2 id="mz-decide-h" className="section-title">
          {updating ? 'Обновить модуль?' : 'Установить модуль?'}
        </h2>
        <span className="cap">
          {updating ? 'Новая версия будет выполнять свой код на этом сервере.' : 'Модуль будет выполнять свой код на этом сервере.'} Ставит и удаляет модули
          только владелец.
        </span>
      </div>
      <div className="mz-body">
        {unverified ? (
          <>
            <span className="adm-status warn">
              {signature.state === 'unknown'
                ? 'Подписан неизвестным ключом: панель не может проверить, кто собрал этот архив'
                : 'Подписи нет: панель не может проверить, кто собрал этот архив'}
            </span>
            <label className="mz-check">
              <input type="checkbox" checked={trusted} onChange={(e) => onTrusted(e.target.checked)} disabled={busy} />
              <span>Я получил архив от автора напрямую и доверяю ему</span>
            </label>
          </>
        ) : null}
        <OwnerFields value={confirm} onChange={onConfirm} totp={totp} invalid={error?.field ?? null} busy={busy} onSubmit={onInstall} />
        {error ? (
          <p className="err" role="alert">
            {error.text}
          </p>
        ) : null}
        <button type="button" className="btn-main btn mz-wide" onClick={onInstall} disabled={busy || (unverified && !trusted)}>
          {busy ? 'Запускаем…' : updating ? 'Обновить' : 'Установить'}
        </button>
        <span className="cap">Около минуты. Панель и бот в это время работают.</span>
      </div>
    </section>
  );
}

function Progress({ module, runtime, updating, canPage }: { module: ExtCard; runtime: RuntimeView; updating: boolean; canPage: boolean }) {
  const job = runtime.job;
  const state = job?.state ?? 'waiting';
  const active = state === 'waiting' || state === 'running';
  const title = active ? (updating ? 'Обновляем' : 'Устанавливаем') : state === 'done' ? 'Модуль работает' : updating ? 'Модуль не обновлён' : 'Модуль не установлен';
  return (
    <section className="sec mz-decide" aria-labelledby="mz-run-h">
      <div className="sec-head">
        <h2 id="mz-run-h" className="section-title">
          {title}
        </h2>
        {active ? (
          <span className="cap">Страницу можно закрыть: {updating ? 'обновление' : 'установка'} продолжится на сервере, итог появится в карточке модуля.</span>
        ) : null}
      </div>
      <div className="mz-body" aria-live="polite">
        {job ? <JobSteps job={job} /> : null}
        {state === 'waiting' ? <span className="cap">Ждём, когда сервер возьмёт заявку.</span> : null}
        {state === 'failed' || state === 'rolled_back' ? (
          <p className="err" role="alert">
            {jobFailure(job) ?? (updating ? 'Обновление не прошло' : 'Установка не прошла')}
          </p>
        ) : null}
        {state === 'done' && module.has_settings ? <span className="cap">Осталось заполнить настройки модуля: они в его карточке.</span> : null}
        {active ? null : (
          <Link to={extLink(module.id)} className={state === 'done' ? 'btn-main btn mz-wide' : 'btn-line hov mz-wide'}>
            Открыть карточку модуля
          </Link>
        )}
        {state === 'done' && module.page && canPage ? (
          <Link to={`${extLink(module.id)}/page`} className="btn-line hov mz-wide">
            Открыть страницу «{module.page.title}»
          </Link>
        ) : null}
      </div>
    </section>
  );
}

export function ExtInstall() {
  const [params] = useSearchParams();
  const update = params.get('update');
  const navigate = useNavigate();
  const { can, reload } = useSession();
  const phone = usePhone();
  const input = useRef<HTMLInputElement>(null);
  const xhr = useRef<XMLHttpRequest | null>(null);
  const pending = useRef<string | null>(null);
  const round = useRef(0);
  const [info, setInfo] = useState<InstallInfo | null>(null);
  const [infoError, setInfoError] = useState<string | null>(null);
  const [picked, setPicked] = useState<Picked | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [source, setSource] = useState<string | null>(null);
  const [result, setResult] = useState<PreviewResult | null>(null);
  const [done, setDone] = useState<InstallResult | null>(null);
  const [sent, setSent] = useState<number | null>(null);
  const [checking, setChecking] = useState(false);
  const [upload, setUpload] = useState<string | null>(null);
  const [check, setCheck] = useState<PackageCheck | null>(null);
  const [confirm, setConfirm] = useState<Confirm>(NO_CONFIRM);
  const [trusted, setTrusted] = useState(false);
  const [formError, setFormError] = useState<ConfirmError | null>(null);
  const [installed, setInstalled] = useState<ExtCard | null>(null);
  const [runtime, setRuntime] = useState<RuntimeView | null>(null);
  const [more, setMore] = useState(false);

  useRuntimeWatch(installed?.id ?? null, runtime, setRuntime);

  useEffect(() => {
    let alive = true;
    api<InstallInfo>('/api/admin/ext-modules/install-info')
      .then((next) => {
        if (alive) {
          setInfo(next);
        }
      })
      .catch((err) => {
        if (alive) {
          setInfoError(failText(err, 'Не удалось узнать, можно ли ставить модули из архива'));
        }
      });
    return () => {
      alive = false;
    };
  }, []);

  const drop = useCallback(() => {
    const id = pending.current;
    pending.current = null;
    if (id) {
      discardUpload(id);
    }
  }, []);

  useEffect(() => {
    window.addEventListener('pagehide', drop);
    return () => {
      window.removeEventListener('pagehide', drop);
      round.current += 1;
      xhr.current?.abort();
      drop();
    };
  }, [drop]);

  const blocked: Blocked | null = info
    ? zipBlocked(info)
    : infoError
      ? { title: 'Архив модуля сейчас не поставить', about: `${infoError}. Модуль, который работает на другом сервере, можно подключить файлом описания JSON.`, steps: [] }
      : null;
  const zipOpen = info !== null && blocked === null;
  const zipMode = picked?.kind === 'zip' && zipOpen;
  const preview = zipMode ? (check?.preview ?? null) : null;
  const jsonPreview = picked?.kind === 'json' ? (result?.preview ?? null) : null;
  const updating = Boolean(update || jsonPreview?.installed || preview?.installed);

  const clear = () => {
    round.current += 1;
    xhr.current?.abort();
    drop();
    setPicked(null);
    setBusy(false);
    setError(null);
    setSource(null);
    setResult(null);
    setDone(null);
    setSent(null);
    setChecking(false);
    setUpload(null);
    setCheck(null);
    setConfirm(NO_CONFIRM);
    setTrusted(false);
    setFormError(null);
    setMore(false);
  };

  const readJson = async (file: File) => {
    if (file.size > MANIFEST_LIMIT) {
      setError('Файл больше 64 КБ — это не похоже на описание модуля');
      return;
    }
    const mine = round.current;
    setBusy(true);
    try {
      const text = await readText(file);
      if (mine !== round.current) {
        return;
      }
      setSource(text);
      const next = await api<PreviewResult>('/api/admin/ext-modules/preview', { method: 'POST', body: { manifest: text, update } });
      if (mine === round.current) {
        setResult(next);
      }
    } catch (err) {
      if (mine === round.current) {
        setError(failText(err, 'Файл не проверился'));
      }
    } finally {
      if (mine === round.current) {
        setBusy(false);
      }
    }
  };

  const sendZip = async (file: File, max: number) => {
    if (file.size === 0) {
      setError('Файл пустой');
      return;
    }
    if (file.size > max) {
      setError(`Архив больше ${sizeText(max)}`);
      return;
    }
    const mine = round.current;
    const live = () => mine === round.current;
    let id: string | null = null;
    setSent(0);
    try {
      for (let offset = 0; offset < file.size; offset += UPLOAD_PART) {
        const part = file.slice(offset, Math.min(file.size, offset + UPLOAD_PART));
        const url: string = id ? `/api/admin/ext-modules/packages/${pathPart(id)}` : '/api/admin/ext-modules/packages';
        const saved = await sendPart(xhr, url, part, (loaded) => {
          if (live()) {
            setSent(offset + loaded);
          }
        });
        id = saved.id;
        if (!live()) {
          discardUpload(saved.id);
          return;
        }
        pending.current = saved.id;
      }
    } catch (err) {
      if (!live()) {
        return;
      }
      setSent(null);
      drop();
      setError(failText(err, 'Архив не загрузился'));
      return;
    }
    if (!id) {
      return;
    }
    setSent(null);
    setUpload(id);
    setChecking(true);
    try {
      const next = await api<PackageCheck>(`/api/admin/ext-modules/packages/${pathPart(id)}/check`, { method: 'POST', body: update ? { update } : {} });
      if (!live()) {
        return;
      }
      setCheck(next);
      if (!next.preview) {
        drop();
        setUpload(null);
      }
    } catch (err) {
      if (live()) {
        setError(failText(err, 'Архив не проверился'));
      }
    } finally {
      if (live()) {
        setChecking(false);
      }
    }
  };

  const choose = async (file: File) => {
    clear();
    const kind = fileKind(file.name, file.type);
    setPicked({ kind, name: file.name, size: file.size });
    if (kind === 'json') {
      await readJson(file);
      return;
    }
    if (kind === null) {
      setError(zipOpen ? 'Выберите архив модуля .zip или файл описания .json' : 'Выберите файл описания модуля .json');
      return;
    }
    if (!info || !zipOpen) {
      setError('Архив модуля здесь не поставить: подойдёт только файл описания модуля .json');
      return;
    }
    await sendZip(file, info.max_bytes);
  };

  const pick = (event: ChangeEvent<HTMLInputElement>) => {
    const file = event.target.files?.[0];
    event.target.value = '';
    if (file) {
      void choose(file);
    }
  };

  const confirmJson = async () => {
    if (!jsonPreview || source === null) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await api<InstallResult>('/api/admin/ext-modules', {
        method: 'POST',
        body: {
          manifest: source,
          update,
          permissions: kept(jsonPreview.permissions).map((p) => p.key),
          events: kept(jsonPreview.events).map((e) => e.kind),
          webhook: jsonPreview.webhook,
          page: jsonPreview.page?.url ?? null,
        },
      });
      await reload();
      if (next.credentials) {
        setDone(next);
      } else {
        navigate(extLink(next.module.id), { state: { note: `Модуль обновлён до версии ${next.module.version}` } });
      }
    } catch (err) {
      setError(failText(err, 'Модуль не установился'));
    } finally {
      setBusy(false);
    }
  };

  const install = async () => {
    if (!preview || !upload || busy) {
      return;
    }
    const unverified = preview.package.signature.state !== 'verified';
    if (unverified && !trusted) {
      setFormError({ text: 'Отметьте, что получили архив от автора напрямую и доверяете ему', field: null });
      return;
    }
    if (!confirm.password) {
      setFormError({ text: 'Введите пароль', field: 'password' });
      return;
    }
    const mine = round.current;
    setBusy(true);
    setFormError(null);
    pending.current = null;
    try {
      const next = await api<PackageInstalled>(`/api/admin/ext-modules/packages/${pathPart(upload)}/install`, {
        method: 'POST',
        body: {
          password: confirm.password,
          code: info?.totp_required ? confirm.code : undefined,
          permissions: kept(preview.permissions).map((p) => p.key),
          events: kept(preview.events).map((e) => e.kind),
          trust_unsigned: unverified && trusted,
          update: update ?? undefined,
        },
      });
      if (mine !== round.current) {
        return;
      }
      setConfirm(NO_CONFIRM);
      setInstalled(next.module);
      setRuntime(next.runtime);
      void reload();
    } catch (err) {
      if (mine !== round.current) {
        return;
      }
      if (err instanceof ApiError && err.status >= 400) {
        pending.current = upload;
      }
      setFormError(confirmError(err, updating ? 'Обновление не запустилось' : 'Установка не запустилась'));
    } finally {
      if (mine === round.current) {
        setBusy(false);
      }
    }
  };

  if (done?.credentials) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        <div className="md-title">
          <h1 className="page-title tight">Модуль «{done.module.name}» установлен</h1>
          <p>{installedNote(done.module)}</p>
        </div>
        <CredentialsBlock credentials={done.credentials} onDone={() => navigate(extLink(done.module.id))} />
        <Link to={extLink(done.module.id)} className="md-back">
          Открыть карточку модуля
        </Link>
      </div>
    );
  }

  const problems = (zipMode ? check?.errors : picked?.kind === 'json' ? result?.errors : undefined) ?? [];
  const percent = sent !== null && picked ? Math.min(100, Math.floor((sent / Math.max(1, picked.size)) * 100)) : 0;
  let status: string | null = null;
  if (!picked) {
    status = zipOpen && info ? `Архив модуля .zip до ${sizeText(info.max_bytes)} или файл описания .json до 64 КБ` : 'JSON до 64 КБ';
  } else if (zipMode) {
    if (sent !== null) {
      status = `Загружаем: ${percent} % из ${sizeText(picked.size)}`;
    } else if (checking) {
      status = `${sizeText(picked.size)} · проверяем архив…`;
    } else {
      status = preview ? `${sizeText(picked.size)} · архив проверен` : sizeText(picked.size);
    }
  } else if (busy && !jsonPreview) {
    status = 'Проверяем файл…';
  }
  const chooseLabel = !picked ? (zipOpen ? 'Выбрать файл' : 'Выбрать файл описания') : zipMode ? 'Другой архив' : 'Выбрать другой';

  const fileRow = (
    <>
      <span className="adm-who">
        <span className="adm-email">{picked?.name ?? 'Файл не выбран'}</span>
        {status ? <span className="adm-status">{status}</span> : null}
        {sent !== null ? (
          <span className="mz-bar" role="progressbar" aria-label="Загрузка архива" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}>
            <span style={{ width: `${percent}%` }} />
          </span>
        ) : null}
      </span>
      {installed ? null : (
        <span className="adm-acts">
          {sent !== null ? (
            <button type="button" className="b34 hov" onClick={clear}>
              Отменить
            </button>
          ) : (
            <button type="button" className="b34 hov" onClick={() => input.current?.click()} disabled={busy || checking || (!info && !infoError)}>
              {chooseLabel}
            </button>
          )}
        </span>
      )}
    </>
  );

  const problemList = problems.length ? (
    <>
      <span className="err" role="alert">
        {zipMode ? 'В архиве' : 'В файле'} {problems.length === 1 ? 'ошибка' : 'ошибки'}, модуль не установить:
      </span>
      <ul className="ext-problems">
        {problems.map((p, i) => (
          <li key={`${p.path}-${i}`}>{problemText(p)}</li>
        ))}
      </ul>
    </>
  ) : null;

  const canPage = Boolean(installed?.page && can(installed.page.permission));

  return (
    <div className={zipMode ? 'mz-page' : 'md-page'}>
      <input
        ref={input}
        type="file"
        accept={zipOpen ? '.zip,.json,application/zip,application/json' : '.json,application/json'}
        hidden
        onChange={pick}
      />
      <Link to={update ? extLink(update) : '/admin/modules'} className="md-back">
        {update ? 'Карточка модуля' : 'Все модули'}
      </Link>
      <div className="md-title">
        <h1 className="page-title tight">{updating ? 'Обновление модуля' : 'Установка модуля'}</h1>
        {zipMode ? null : <p>{INTRO}</p>}
      </div>
      {!info && !infoError ? <p className="muted">Загружаем…</p> : null}
      {blocked ? (
        <BlockedSection blocked={blocked}>
          <div className="row adm-row">{fileRow}</div>
          {problemList ? (
            <div className="row">
              <span className="rc">{problemList}</span>
            </div>
          ) : null}
        </BlockedSection>
      ) : info ? (
        <>
          <section className="card mz-file" aria-label="Файл модуля">
            {fileRow}
          </section>
          {problemList ? <section className="card mz-problems">{problemList}</section> : null}
        </>
      ) : null}
      {error && !jsonPreview ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {jsonPreview ? (
        <>
          <section className="card md-state" aria-label="Модуль из файла">
            <div className="md-text">
              <span className="md-name">
                {jsonPreview.name} <span className="muted">{jsonPreview.version}</span>
              </span>
              {jsonPreview.description ? <span className="md-desc">{jsonPreview.description}</span> : null}
              <span className="md-where">
                Автор: {jsonPreview.author}
                {jsonPreview.site ? (
                  <>
                    {' · '}
                    <a href={jsonPreview.site} target="_blank" rel="noreferrer noopener" className="md-link-inline">
                      сайт модуля
                    </a>
                  </>
                ) : null}
              </span>
              {jsonPreview.installed ? (
                <span className="md-note">
                  Уже установлена версия {jsonPreview.installed.version}: файл обновит модуль, ключ и секрет останутся прежними.
                </span>
              ) : null}
            </div>
          </section>
          <PreviewCard preview={jsonPreview} />
          {error ? (
            <p className="err" role="alert">
              {error}
            </p>
          ) : null}
          <div className="md-actions">
            <button type="button" className="btn-main btn" onClick={() => void confirmJson()} disabled={busy}>
              {busy ? 'Сохраняем…' : updating ? 'Подтвердить и обновить' : 'Подтвердить и установить'}
            </button>
            <button type="button" className="btn-line hov" onClick={clear} disabled={busy}>
              Отменить
            </button>
          </div>
        </>
      ) : null}
      {preview ? (
        <div className="mz-grid">
          <div className="mz-main">
            <ZipSummary preview={preview} updating={updating} />
            {phone ? null : (
              <section className="sec ext-facts" aria-labelledby="mz-more-h">
                <div className="sec-head">
                  <h2 id="mz-more-h" className="section-title">
                    Подробно
                  </h2>
                </div>
                <DetailRows preview={preview} />
              </section>
            )}
          </div>
          <aside className="mz-side">
            {installed && runtime ? (
              <Progress module={installed} runtime={runtime} updating={updating} canPage={canPage} />
            ) : (
              <Decide
                preview={preview}
                updating={updating}
                totp={info?.totp_required ?? false}
                confirm={confirm}
                onConfirm={setConfirm}
                trusted={trusted}
                onTrusted={setTrusted}
                busy={busy}
                error={formError}
                onInstall={() => void install()}
              />
            )}
          </aside>
          {phone ? (
            <section className="sec ext-facts">
              <button type="button" className="mz-more" aria-expanded={more} aria-controls="mz-more-rows" onClick={() => setMore(!more)}>
                <span className="adm-who">
                  <span>Подробно</span>
                  <span className="cap">данные, события, права, настройки, архив</span>
                </span>
                <Icon name="chevron-down" size={18} stroke={2} />
              </button>
              {more ? (
                <div id="mz-more-rows">
                  <DetailRows preview={preview} />
                </div>
              ) : null}
            </section>
          ) : null}
        </div>
      ) : null}
    </div>
  );
}
