import { Fragment, useCallback, useEffect, useRef, useState, type FormEvent } from 'react';
import { Link, useNavigate } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, timeHM } from '../lib/format';
import { msg, t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { supportTickets } from '../lib/support';
import { Card, useCabinet } from './CabinetApp';
import { aboutEmail, closedText, doneText, sixDigits, unconfirmedText, useEmailChange, withoutCodeText } from './email';
import type { TwoFactor } from './model';
import { TelegramLink } from './TelegramLink';
import { finishOauth, leaveFor, PROVIDER_TITLE, ProviderMark, returnedProvider, type OauthProvider } from './oauth';

type Setup = { secret: string; uri: string };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function grouped(secret: string): string {
  return secret.replace(/(.{4})/g, '$1 ').trim();
}

function digits(value: string): string {
  return value.replace(/\D/g, '').slice(0, 6);
}

export function SecurityPage() {
  const { me, site, reload } = useCabinet();
  const [state, setState] = useState<TwoFactor>(me.two_factor);
  const [setup, setSetup] = useState<Setup | null>(null);
  const [turningOff, setTurningOff] = useState(false);
  const [code, setCode] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [copied, setCopied] = useState(false);

  useEffect(() => {
    let alive = true;
    api<TwoFactor>('/api/client/security').then(
      (next) => {
        if (alive) {
          setState(next);
        }
      },
      () => undefined,
    );
    return () => {
      alive = false;
    };
  }, []);

  const begin = async () => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setSetup(await api<Setup>('/api/client/security/totp/setup', { method: 'POST', body: {} }));
      setCode('');
    } catch (err) {
      setError(failText(err, t('Настройка не началась, попробуйте ещё раз')));
    } finally {
      setBusy(false);
    }
  };

  const enable = async (event: FormEvent) => {
    event.preventDefault();
    if (!setup) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await api<TwoFactor>('/api/client/security/totp/enable', { method: 'POST', body: { secret: setup.secret, code } });
      setState(next);
      setSetup(null);
      setCode('');
      setNotice(t('Вход по коду включён. При следующем входе на сайт понадобится код из приложения.'));
      void reload();
    } catch (err) {
      setError(failText(err, t('Код не подошёл')));
    } finally {
      setBusy(false);
    }
  };

  const disable = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const next = await api<TwoFactor>('/api/client/security/totp/disable', { method: 'POST', body: { code } });
      setState(next);
      setTurningOff(false);
      setCode('');
      setNotice(t('Вход по коду выключен.'));
      void reload();
    } catch (err) {
      setError(failText(err, t('Код не подошёл')));
    } finally {
      setBusy(false);
    }
  };

  const copy = async () => {
    if (!setup) {
      return;
    }
    try {
      await navigator.clipboard.writeText(setup.secret);
      setCopied(true);
      window.setTimeout(() => setCopied(false), 2000);
    } catch {
      setError(t('Браузер не дал скопировать: выделите ключ вручную'));
    }
  };

  const stop = () => {
    setSetup(null);
    setTurningOff(false);
    setCode('');
    setError(null);
  };

  const codeField = (
    <label className="cb-field">
      <span>{t('Код из приложения')}</span>
      <input value={code} onChange={(e) => setCode(digits(e.target.value))} inputMode="numeric" autoComplete="one-time-code" placeholder={t('6 цифр')} />
    </label>
  );

  const [lostBefore, lostAfter] = t('Если телефон с приложением потерян, {link}: вход по коду сбросят, и его можно будет включить заново.').split('{link}');

  return (
    <div className="cb-secure">
      <h1 className="cb-title">{t('Безопасность')}</h1>
      <LoginsCard />
      <EmailCard />
      <PasswordCard />
      <Card label={t('Вход по коду из приложения')}>
        <div className="cb-row wrap">
          <h2 className="cb-h2">{t('Вход по коду из приложения')}</h2>
          <span className={state.enabled ? 'cb-status ink2' : 'cb-status muted'}>{state.enabled ? t('Включён') : t('Выключен')}</span>
        </div>
        <p className="cb-note">
          {t('После подтверждения в Telegram или кода из письма сайт спросит ещё шестизначный код из приложения-аутентификатора, например Google Authenticator или Яндекс Ключ. В мини-приложении внутри Telegram код не нужен.')}
        </p>
        {state.enabled && state.enabled_at ? <span className="cb-small cb-muted">{t('Включён {date}', { date: dateLong(state.enabled_at) })}</span> : null}
        {notice ? (
          <p className="cb-result" role="status">
            {notice}
          </p>
        ) : null}
        {!state.enabled && !setup ? (
          <span className="cb-btns">
            <button type="button" className="cb-main" onClick={() => void begin()} disabled={busy}>
              {t('Включить')}
            </button>
          </span>
        ) : null}
        {setup ? (
          <form className="cb-secure-form" onSubmit={enable}>
            <ol className="cb-steps">
              <li>
                <span className="cb-num">1</span>
                <span className="cb-step">
                  <span className="cb-step-text">
                    <strong>{t('Добавьте {brand} в приложение', { brand: site.brand_name })}</strong>
                    <span className="cb-small cb-muted">{t('Отсканируйте QR-код камерой приложения или введите ключ вручную')}</span>
                  </span>
                  <QrCode text={setup.uri} size={176} label={t('QR-код для приложения-аутентификатора')} />
                  <span className="cb-linkrow">
                    <span className="cb-link mono">{grouped(setup.secret)}</span>
                    <button type="button" className="cb-second" onClick={() => void copy()}>
                      {copied ? t('Скопировано') : t('Скопировать ключ')}
                    </button>
                  </span>
                </span>
              </li>
              <li>
                <span className="cb-num">2</span>
                <span className="cb-step">
                  <span className="cb-step-text">
                    <strong>{t('Введите код, который покажет приложение')}</strong>
                    <span className="cb-small cb-muted">{t('Код меняется каждые 30 секунд')}</span>
                  </span>
                  {codeField}
                </span>
              </li>
            </ol>
            <span className="cb-btns">
              <button type="submit" className="cb-main" disabled={busy || code.length !== 6}>
                {t('Включить вход по коду')}
              </button>
              <button type="button" className="cb-cancel" onClick={stop} disabled={busy}>
                {t('Отменить')}
              </button>
            </span>
          </form>
        ) : null}
        {state.enabled && !turningOff ? (
          <span className="cb-btns">
            <button
              type="button"
              className="cb-second"
              onClick={() => {
                setTurningOff(true);
                setCode('');
                setNotice(null);
              }}
            >
              {t('Выключить')}
            </button>
          </span>
        ) : null}
        {state.enabled && turningOff ? (
          <form className="cb-secure-form" onSubmit={disable}>
            <p className="cb-ink2">{t('Чтобы выключить вход по коду, введите код из приложения.')}</p>
            {codeField}
            <span className="cb-btns">
              <button type="submit" className="cb-main" disabled={busy || code.length !== 6}>
                {t('Выключить вход по коду')}
              </button>
              <button type="button" className="cb-cancel" onClick={stop} disabled={busy}>
                {t('Отменить')}
              </button>
            </span>
          </form>
        ) : null}
        {error ? (
          <p className="cb-error" role="alert">
            {error}
          </p>
        ) : null}
        <p className="cb-small cb-muted">
          {lostBefore}
          {supportTickets(site) ? (
            <Link className="cb-accent-link" to="/cabinet/help">
              {t('напишите в поддержку')}
            </Link>
          ) : site.support_url ? (
            <a className="cb-accent-link" href={site.support_url} target="_blank" rel="noreferrer">
              {t('напишите в поддержку')}
            </a>
          ) : (
            t('напишите в поддержку')
          )}
          {lostAfter}
        </p>
      </Card>
    </div>
  );
}

function focusPassword() {
  const field = document.getElementById('cb-new-password');
  field?.scrollIntoView({ block: 'center', behavior: 'smooth' });
  field?.focus({ preventScroll: true });
}

function EmailCard() {
  const { me, site, reload } = useCabinet();
  const flow = useEmailChange(reload);
  const [open, setOpen] = useState(false);
  const { info, sent } = flow;
  if (!info) {
    return flow.failed ? (
      <Card label={t('Почта')}>
        <h2 className="cb-h2">{t('Почта')}</h2>
        <p className="cb-error" role="alert">
          {flow.failed}
        </p>
      </Card>
    ) : null;
  }
  const editing = (open && !flow.done) || sent !== null;
  const siteLogin = site.client_access?.site_login !== false;
  const askPassword = !!flow.done && siteLogin && site.email_auth === 'password' && me.has_password !== true;
  const about = aboutEmail(info, site);
  const note = info.without_code
    ? withoutCodeText()
    : info.proof === 'totp'
      ? t('Для смены почты нужен код из приложения-аутентификатора, затем код из письма на новый адрес.')
      : info.proof === 'password'
        ? t('Для смены почты нужен пароль для сайта, затем код из письма на новый адрес.')
        : flow.byCurrent
          ? t('Для смены почты нужен код из письма на текущую почту, затем код из письма на новый адрес.')
          : t('На новый адрес придёт код. Почта сменится, когда вы его введёте.');
  const errorLine = flow.error ? (
    <p className="cb-error" role="alert">
      {flow.error}
    </p>
  ) : null;

  return (
    <Card label={t('Почта')}>
      <div className="cb-row wrap">
        <h2 className="cb-h2">{t('Почта')}</h2>
        <span className="cb-status muted">{info.email ?? t('не указана')}</span>
      </div>
      {about ? <p className="cb-note">{about}</p> : null}
      {info.available && info.unconfirmed && !sent ? (
        <>
          <p className="cb-note">
            <strong>{t('Почта не подтверждена')}.</strong> {unconfirmedText(site)}
          </p>
          {flow.confirmError ? (
            <p className="cb-error" role="alert">
              {flow.confirmError}
            </p>
          ) : null}
          <span className="cb-btns">
            <button type="button" className="cb-main" onClick={() => void flow.confirmCurrent()} disabled={flow.busy}>
              {flow.busy ? t('Отправляем…') : t('Прислать код')}
            </button>
          </span>
        </>
      ) : null}
      {flow.done ? (
        <p className="cb-result" role="status">
          {doneText(flow.done)}
        </p>
      ) : null}
      {askPassword ? <p className="cb-note">{t('Теперь задайте пароль: с почтой и паролем можно войти в кабинет, если Telegram не работает.')}</p> : null}
      {flow.done && siteLogin && site.email_auth === 'code' ? (
        <p className="cb-note">{t('Теперь на сайт можно входить по этой почте: код для входа придёт в письме.')}</p>
      ) : null}
      {!info.available && !info.without_code ? (
        <p className="cb-note">{closedText(info)}</p>
      ) : !editing ? (
        <span className="cb-btns">
          {askPassword ? (
            <button type="button" className="cb-main" onClick={focusPassword}>
              {t('Задать пароль')}
            </button>
          ) : null}
          <button
            type="button"
            className="cb-second"
            onClick={() => {
              flow.reset();
              setOpen(true);
            }}
          >
            {info.email ? t('Сменить почту') : t('Добавить почту')}
          </button>
        </span>
      ) : sent ? (
        <form
          className="cb-secure-form"
          onSubmit={(event) => {
            event.preventDefault();
            void flow.confirm();
          }}
        >
          <p className="cb-ink2">{t('Код отправлен на {email}. Он действует до {time}.', { email: sent.email, time: timeHM(sent.expires_at) })}</p>
          <label className="cb-field">
            <span>{t('Код из письма')}</span>
            <input value={flow.code} onChange={(e) => flow.setCode(sixDigits(e.target.value))} inputMode="numeric" autoComplete="one-time-code" placeholder={t('6 цифр')} />
          </label>
          {errorLine}
          <span className="cb-btns">
            <button type="submit" className="cb-main" disabled={flow.busy || flow.code.length !== 6}>
              {flow.busy ? t('Проверяем…') : t('Подтвердить')}
            </button>
            <button type="button" className="cb-cancel" onClick={flow.restart} disabled={flow.busy}>
              {t('Указать другой адрес')}
            </button>
          </span>
        </form>
      ) : (
        <form
          className="cb-secure-form"
          onSubmit={(event) => {
            event.preventDefault();
            void flow.start();
          }}
        >
          <label className="cb-field">
            <span>{t('Новая почта')}</span>
            <input type="email" value={flow.address} onChange={(e) => flow.setAddress(e.target.value)} autoComplete="email" maxLength={254} />
          </label>
          {info.proof === 'password' ? (
            <label className="cb-field">
              <span>{t('Пароль для сайта')}</span>
              <input type="password" value={flow.proof} onChange={(e) => flow.setProof(e.target.value)} autoComplete="current-password" />
            </label>
          ) : null}
          {info.proof === 'totp' ? (
            <label className="cb-field">
              <span>{t('Код из приложения')}</span>
              <input value={flow.proof} onChange={(e) => flow.setProof(sixDigits(e.target.value))} inputMode="numeric" autoComplete="one-time-code" placeholder={t('6 цифр')} />
            </label>
          ) : null}
          {flow.byCurrent ? (
            <>
              <label className="cb-field">
                <span>{t('Код из письма на текущую почту')}</span>
                <input value={flow.proof} onChange={(e) => flow.setProof(sixDigits(e.target.value))} inputMode="numeric" autoComplete="one-time-code" placeholder={t('6 цифр')} />
              </label>
              <span className="cb-btns">
                <button type="button" className="cb-cancel" onClick={() => void flow.sendProof()} disabled={flow.busy}>
                  {flow.proofSent ? t('Прислать код ещё раз') : t('Прислать код на {email}', { email: info.email ?? '' })}
                </button>
              </span>
            </>
          ) : null}
          <p className="cb-small cb-muted">{note}</p>
          {errorLine}
          <span className="cb-btns">
            <button type="submit" className="cb-main" disabled={flow.busy || !flow.ready}>
              {info.without_code ? (flow.busy ? t('Сохраняем…') : t('Сохранить почту')) : flow.busy ? t('Отправляем…') : t('Получить код')}
            </button>
            <button
              type="button"
              className="cb-cancel"
              onClick={() => {
                setOpen(false);
                flow.reset();
              }}
              disabled={flow.busy}
            >
              {t('Отменить')}
            </button>
          </span>
        </form>
      )}
    </Card>
  );
}

function PasswordCard() {
  const { me, site, reload } = useCabinet();
  const [current, setCurrent] = useState('');
  const [fresh, setFresh] = useState('');
  const [busy, setBusy] = useState(false);
  const [result, setResult] = useState<{ text: string; bad: boolean } | null>(null);
  if (!me.email || site.email_auth !== 'password') {
    return null;
  }
  const has = me.has_password === true;

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setResult(null);
    try {
      await api('/api/client/security/password', { method: 'POST', body: { current: has ? current : undefined, password: fresh } });
      setCurrent('');
      setFresh('');
      setResult({ text: has ? t('Пароль изменён. На других устройствах кабинет попросит войти заново.') : t('Пароль задан. Теперь можно входить по почте и паролю.'), bad: false });
      await reload();
    } catch (err) {
      setResult({ text: err instanceof ApiError ? err.message : t('Пароль не сохранился'), bad: true });
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card label={t('Пароль')}>
      <div className="cb-row wrap">
        <h2 className="cb-h2">{t('Пароль')}</h2>
        <span className="cb-status muted">{me.email}</span>
      </div>
      <p className="cb-note">{has ? t('Пароль для входа на сайт по почте.') : t('Пароля пока нет: вы входили через Telegram или по коду из письма. Задайте его, чтобы входить по почте и паролю.')}</p>
      <form className="cb-secure-form" onSubmit={submit}>
        {has ? (
          <label className="cb-field">
            <span>{t('Текущий пароль')}</span>
            <input type="password" value={current} onChange={(e) => setCurrent(e.target.value)} autoComplete="current-password" />
          </label>
        ) : null}
        <label className="cb-field">
          <span>{t('Новый пароль')}</span>
          <input id="cb-new-password" type="password" value={fresh} onChange={(e) => setFresh(e.target.value)} autoComplete="new-password" placeholder={t('Не короче 10 символов')} />
        </label>
        {result ? (
          <p className={result.bad ? 'cb-error' : 'cb-result'} role={result.bad ? 'alert' : 'status'}>
            {result.text}
          </p>
        ) : null}
        <span className="cb-btns">
          <button type="submit" className="cb-main" disabled={busy || fresh.length < 10 || (has && !current)}>
            {busy ? t('Сохраняем…') : has ? t('Сменить пароль') : t('Задать пароль')}
          </button>
        </span>
      </form>
    </Card>
  );
}

type LoginKind = 'telegram' | 'email' | 'google' | 'apple';
type LoginMethod = { kind: LoginKind; account: string; linked_at: string; removable: boolean };
type Logins = { methods: LoginMethod[]; can_link: OauthProvider[]; can_link_telegram?: boolean };

const KIND_TITLE: Record<LoginKind, string> = { telegram: 'Telegram', email: msg('Почта'), google: 'Google', apple: 'Apple' };

function isProvider(kind: LoginKind): kind is OauthProvider {
  return kind === 'google' || kind === 'apple';
}

function KindMark({ kind }: { kind: LoginKind }) {
  if (isProvider(kind)) {
    return <ProviderMark provider={kind} />;
  }
  return <Icon name={kind === 'telegram' ? 'send' : 'mail'} size={20} />;
}

function LoginsCard() {
  const { phone, reload } = useCabinet();
  const navigate = useNavigate();
  const [data, setData] = useState<Logins | null>(null);
  const [ask, setAsk] = useState<OauthProvider | null>(null);
  const [busy, setBusy] = useState(false);
  const [notice, setNotice] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const finished = useRef(false);

  const load = useCallback(async () => {
    try {
      setData(await api<Logins>('/api/client/security/logins'));
    } catch (err) {
      setError(failText(err, t('Способы входа не загрузились')));
    }
  }, []);

  useEffect(() => {
    const back = returnedProvider();
    if (!back || finished.current) {
      void load();
      return;
    }
    finished.current = true;
    finishOauth()
      .then(
        (res) => {
          if (res.status === 'linked') {
            setNotice(t('Вход через {provider} привязан. Теперь им можно входить в кабинет.', { provider: PROVIDER_TITLE[res.provider] }));
          }
        },
        (err) => setError(failText(err, t('Привязка не завершилась, попробуйте ещё раз'))),
      )
      .finally(() => {
        navigate('/cabinet/security', { replace: true });
        void load();
      });
  }, [load, navigate]);

  const link = async (provider: OauthProvider) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      await leaveFor(`/api/client/security/logins/${provider}`);
    } catch (err) {
      setError(failText(err, t('Привязка не началась')));
      setBusy(false);
    }
  };

  const unlink = async (provider: OauthProvider) => {
    setBusy(true);
    setError(null);
    setNotice(null);
    try {
      setData(await api<Logins>(`/api/client/security/logins/${provider}`, { method: 'DELETE' }));
      setAsk(null);
      setNotice(t('Вход через {provider} отвязан.', { provider: PROVIDER_TITLE[provider] }));
    } catch (err) {
      setError(failText(err, t('Не получилось отвязать')));
    } finally {
      setBusy(false);
    }
  };

  const linked = (account: string) => {
    setNotice(t('Telegram привязан: {account}. Теперь им можно входить в кабинет.', { account }));
    void load();
    void reload();
  };

  const removable = data !== null && data.methods.some((m) => isProvider(m.kind));
  const shown = data !== null && (data.can_link.length > 0 || data.can_link_telegram === true || removable || data.methods.length > 1);
  if (!shown && !error) {
    return null;
  }

  return (
    <Card label={t('Способы входа')} className="cb-logins">
      <h2 className="cb-h2">{t('Способы входа')}</h2>
      {removable ? (
        <p className="cb-note">{t('Войти в кабинет можно любым из них. Последний способ отвязать нельзя.')}</p>
      ) : data && data.methods.length > 1 ? (
        <p className="cb-note">{t('Войти в кабинет можно любым из них.')}</p>
      ) : null}
      {data?.methods.map((m) => (
        <Fragment key={m.kind}>
          <div className="cb-device">
            <KindMark kind={m.kind} />
            <span className="cb-device-text">
              <strong>{t(KIND_TITLE[m.kind])}</strong>
              <span className="cb-small cb-muted">{m.account}</span>
            </span>
            {isProvider(m.kind) && m.removable ? (
              <button
                type="button"
                className={phone ? 'cb-text' : 'cb-second sm'}
                aria-label={t('Отвязать {kind}', { kind: t(KIND_TITLE[m.kind]) })}
                aria-expanded={ask === m.kind}
                onClick={() => setAsk(ask === m.kind ? null : (m.kind as OauthProvider))}
                disabled={busy}
              >
                {t('Отвязать')}
              </button>
            ) : null}
          </div>
          {ask === m.kind ? (
            <div className="cb-ask">
              <p>
                <strong>{t('Отвязать вход через {provider}?', { provider: PROVIDER_TITLE[ask] })}</strong>{' '}
                {t('Входить через этот аккаунт {provider} больше не получится. Привязать его можно снова.', { provider: PROVIDER_TITLE[ask] })}
              </p>
              <span className="cb-ask-btns">
                <button type="button" className="cb-main sm" onClick={() => void unlink(ask)} disabled={busy}>
                  {t('Отвязать')}
                </button>
                <button type="button" className="cb-cancel" onClick={() => setAsk(null)} disabled={busy}>
                  {t('Отменить')}
                </button>
              </span>
            </div>
          ) : null}
        </Fragment>
      ))}
      {data && data.can_link.length > 0 ? (
        <span className="cb-btns">
          {data.can_link.map((p) => (
            <button key={p} type="button" className="cb-second cb-oauth-btn" onClick={() => void link(p)} disabled={busy}>
              <ProviderMark provider={p} />
              {t('Привязать {provider}', { provider: PROVIDER_TITLE[p] })}
            </button>
          ))}
        </span>
      ) : null}
      {data?.can_link_telegram ? (
        <TelegramLink
          onStart={() => {
            setNotice(null);
            setError(null);
          }}
          onLinked={linked}
        />
      ) : null}
      {notice ? (
        <p className="cb-result" role="status">
          {notice}
        </p>
      ) : null}
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
    </Card>
  );
}
