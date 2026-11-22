import { useCallback, useEffect, useId, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { BrandMark } from '../../lib/brand';
import { Icon } from '../../lib/icons';
import { QrCode } from '../../lib/qr';
import { useSession } from '../session';
import '../form.css';
import '../login.css';
import './admins.css';

type SecurityInfo = { email: string; totp_enabled: boolean; must_change_password: boolean };
type Setup = { secret: string; uri: string };

const MIN_PASSWORD = 10;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function groups(secret: string): string {
  return secret.replace(/(.{4})/g, '$1 ').trim();
}

export function PasswordForm({ forced, onDone }: { forced: boolean; onDone: () => void }) {
  const [current, setCurrent] = useState('');
  const [next, setNext] = useState('');
  const [repeat, setRepeat] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ids = { current: useId(), next: useId(), repeat: useId() };
  const problem =
    next && next.length < MIN_PASSWORD
      ? `Не короче ${MIN_PASSWORD} символов`
      : repeat && repeat !== next
        ? 'Пароли не совпадают'
        : next && next === current
          ? 'Новый пароль совпадает с текущим'
          : null;
  const ready = Boolean(current && next && repeat) && !problem;

  const submit = async () => {
    if (!ready || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api('/api/admin/auth/password', { method: 'POST', body: { current, new: next } });
      setCurrent('');
      setNext('');
      setRepeat('');
      onDone();
    } catch (err) {
      setError(failText(err, 'Пароль не сменился'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <form
      className="sec-form"
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <div className="row">
        <label className="rl" htmlFor={ids.current}>
          {forced ? 'Временный пароль' : 'Текущий пароль'}
        </label>
        <span className="rc">
          <input
            id={ids.current}
            className="field"
            type="password"
            autoComplete="current-password"
            value={current}
            onChange={(e) => setCurrent(e.target.value)}
            disabled={busy}
            autoFocus={forced}
          />
        </span>
      </div>
      <div className="row">
        <label className="rl" htmlFor={ids.next}>
          Новый пароль
        </label>
        <span className="rc">
          <input id={ids.next} className="field" type="password" autoComplete="new-password" value={next} onChange={(e) => setNext(e.target.value)} disabled={busy} />
          <span className="cap">Не короче {MIN_PASSWORD} символов. После смены остальные входы в админку закроются.</span>
        </span>
      </div>
      <div className="row">
        <label className="rl" htmlFor={ids.repeat}>
          Ещё раз
        </label>
        <span className="rc">
          <input
            id={ids.repeat}
            className="field"
            type="password"
            autoComplete="new-password"
            value={repeat}
            aria-invalid={problem !== null}
            onChange={(e) => setRepeat(e.target.value)}
            disabled={busy}
          />
          {problem ? <span className="err">{problem}</span> : null}
        </span>
      </div>
      <div className="row">
        <span className="rc">
          {error ? (
            <span className="err" role="alert">
              {error}
            </span>
          ) : null}
          <span>
            <button type="submit" className="btn-main btn" disabled={!ready || busy}>
              {busy ? 'Меняем…' : forced ? 'Сохранить и продолжить' : 'Сменить пароль'}
            </button>
          </span>
        </span>
      </div>
    </form>
  );
}

function TwoFactor({ info, onChange }: { info: SecurityInfo; onChange: () => void }) {
  const [setup, setSetup] = useState<Setup | null>(null);
  const [off, setOff] = useState(false);
  const [code, setCode] = useState('');
  const [password, setPassword] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const codeId = useId();
  const passId = useId();

  const reset = () => {
    setSetup(null);
    setOff(false);
    setCode('');
    setPassword('');
    setError(null);
  };

  const start = async () => {
    setBusy(true);
    setError(null);
    try {
      setSetup(await api<Setup>('/api/admin/auth/2fa/setup', { method: 'POST', body: {} }));
    } catch (err) {
      setError(failText(err, 'Настройка не началась'));
    } finally {
      setBusy(false);
    }
  };

  const finish = async () => {
    setBusy(true);
    setError(null);
    try {
      if (setup) {
        await api('/api/admin/auth/2fa/enable', { method: 'POST', body: { secret: setup.secret, code: code.trim(), password } });
      } else {
        await api('/api/admin/auth/2fa/disable', { method: 'POST', body: { code: code.trim(), password } });
      }
      reset();
      onChange();
    } catch (err) {
      setError(failText(err, 'Не получилось'));
    } finally {
      setBusy(false);
    }
  };

  const asking = setup !== null || off;
  return (
    <>
      <div className="row adm-row">
        <span className="adm-who">
          <span className="adm-email">{info.totp_enabled ? 'Вход по коду включён' : 'Вход по коду выключен'}</span>
          <span className="adm-status">
            {info.totp_enabled
              ? 'При входе админка спрашивает код из приложения-аутентификатора.'
              : 'Код из приложения на телефоне защитит вход, даже если пароль узнают.'}
          </span>
        </span>
        {asking ? null : (
          <span className="adm-acts">
            {info.totp_enabled ? (
              <button type="button" className="b34 bad hov" onClick={() => setOff(true)}>
                Выключить
              </button>
            ) : (
              <button type="button" className="b34 hov" disabled={busy} onClick={() => void start()}>
                {busy ? 'Готовим…' : 'Включить'}
              </button>
            )}
          </span>
        )}
      </div>
      {setup ? (
        <div className="row tf-setup">
          <QrCode text={setup.uri} size={168} label="QR-код для приложения-аутентификатора" />
          <span className="rc">
            <span>Отсканируйте код в Google Authenticator, Aegis, 1Password или другом приложении. Если камеры нет, введите ключ вручную:</span>
            <span className="mono tf-secret">{groups(setup.secret)}</span>
          </span>
        </div>
      ) : null}
      {asking ? (
        <form
          className="row tf-confirm"
          onSubmit={(e) => {
            e.preventDefault();
            void finish();
          }}
        >
          <span className="rc">
            <span className="adm-new-fields">
              <input
                id={codeId}
                className="field tf-code mono"
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder="Код из приложения"
                aria-label="Код из приложения"
                maxLength={8}
                value={code}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, ''))}
                disabled={busy}
                autoFocus
              />
              <input
                id={passId}
                className="field"
                type="password"
                autoComplete="current-password"
                placeholder="Пароль от админки"
                aria-label="Пароль от админки"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                disabled={busy}
              />
              <button type="submit" className={setup ? 'btn-main btn' : 'btn-bad'} disabled={busy || code.length < 6 || !password}>
                {busy ? 'Проверяем…' : setup ? 'Включить вход по коду' : 'Выключить вход по коду'}
              </button>
              <button type="button" className="btn-line hov" onClick={reset} disabled={busy}>
                Отменить
              </button>
            </span>
          </span>
        </form>
      ) : null}
      {error ? (
        <div className="row">
          <span className="err" role="alert">
            {error}
          </span>
        </div>
      ) : null}
    </>
  );
}

export function Security() {
  const { me, logout } = useSession();
  const [info, setInfo] = useState<SecurityInfo | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setInfo(await api<SecurityInfo>('/api/admin/auth/security'));
      setError(null);
    } catch (err) {
      setError(failText(err, 'Не загрузилось'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  return (
    <>
      <h1 className="page-title tight">Безопасность</h1>
      <p className="muted sec-lead">{me.email ?? info?.email ?? ''}</p>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {notice ? (
        <p className="sec-notice" role="status">
          {notice}
        </p>
      ) : null}
      <div className="sec-stack">
        <section className="sec" aria-labelledby="pw-h">
          <div className="sec-head">
            <h2 id="pw-h" className="section-title">
              Пароль
            </h2>
          </div>
          <PasswordForm forced={false} onDone={() => setNotice('Пароль изменён. Остальные входы в админку закрыты.')} />
        </section>
        <section className="sec" aria-labelledby="tf-h">
          <div className="sec-head">
            <h2 id="tf-h" className="section-title">
              Вход по коду (2FA)
            </h2>
          </div>
          {info ? (
            <TwoFactor
              info={info}
              onChange={() => {
                setNotice(info.totp_enabled ? 'Вход по коду выключен' : 'Вход по коду включён');
                void load();
              }}
            />
          ) : (
            <div className="row">
              <span className="cap">Загружаем…</span>
            </div>
          )}
        </section>
        <section className="sec" aria-labelledby="session-h">
          <div className="sec-head">
            <h2 id="session-h" className="section-title">
              Сеанс
            </h2>
            <span className="muted">Завершить текущий вход в админку. Для возврата понадобится снова ввести пароль.</span>
          </div>
          <div className="row">
            <button type="button" className="btn-line sec-out hov" onClick={() => void logout()}>
              <Icon name="logout" size={18} />
              Выйти
            </button>
          </div>
        </section>
      </div>
    </>
  );
}

export function ForcedPassword({ brand, logo, onDone, onLogout }: { brand: string; logo: string | null; onDone: () => void; onLogout: () => void }) {
  return (
    <div className="login">
      <div className="login-box forced">
        <div className="login-brand">
          <BrandMark name={brand} logo={logo} className="mark" />
          <span>{brand.toUpperCase()}</span>
        </div>
        <section className="card sec" aria-labelledby="forced-h">
          <div className="sec-head">
            <h1 id="forced-h" className="section-title">
              Задайте свой пароль
            </h1>
            <span className="muted">Вход выполнен по временному паролю. Придумайте свой, чтобы продолжить работу в админке.</span>
          </div>
          <PasswordForm forced onDone={onDone} />
        </section>
        <button type="button" className="btn-line hov" onClick={onLogout}>
          Выйти
        </button>
      </div>
    </div>
  );
}
