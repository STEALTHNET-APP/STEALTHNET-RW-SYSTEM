import { useEffect, useRef, useState, type FormEvent } from 'react';
import { useNavigate } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { Captcha, type CaptchaWidget } from '../lib/captcha';
import { NATIVE, t, type Lang } from '../lib/i18n';
import { QrCode } from '../lib/qr';
import { useSurface, useSystemTheme } from '../lib/theme';
import type { Site } from '../lib/types';
import { botLink, storedReferral } from '../lib/referral';
import { reportVisit, storedUtm } from '../lib/utm';
import { startParam } from '../lib/utmTags';
import { finishOauth, leaveFor, PROVIDER_TITLE, ProviderMark, returnedProvider, type OauthProvider } from './oauth';

type Tg = { poll_token: string; link: string; expires_at: string };
type Status = { status: 'pending' | 'confirmed' | 'rejected' | 'expired' | 'totp_required'; challenge?: string };
type Verified = { status?: 'totp_required'; challenge?: string };
type Registered = { status?: 'totp_required' | 'code_sent'; challenge?: string; email?: string; signup_token?: string };
type Signup = { email: string; token: string; again: boolean };

const RESEND_SECONDS = 60;
const HUMAN_CODES = ['antibot.captcha', 'antibot.failed', 'antibot.unavailable'];

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function Login({ site, onDone, langSwitch }: { site: Site; onDone: () => void; langSwitch?: { lang: Lang; onPick: () => void } | null }) {
  const system = useSystemTheme();
  const choice = site.appearance.client_theme;
  const theme = choice === 'dark' || choice === 'light' ? choice : system;
  useSurface('cabinet', theme, site.appearance.client === 'mono' ? 'b' : 'a');
  const [tg, setTg] = useState<Tg | null>(null);
  const [tgState, setTgState] = useState<'idle' | 'waiting' | 'rejected' | 'expired'>('idle');
  const [mode, setMode] = useState<'telegram' | 'email'>('telegram');
  const [email, setEmail] = useState('');
  const [code, setCode] = useState('');
  const [sent, setSent] = useState<{ email: string; again: boolean } | null>(null);
  const [wait, setWait] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [challenge, setChallenge] = useState<string | null>(null);
  const [totp, setTotp] = useState('');
  const [password, setPassword] = useState('');
  const [joining, setJoining] = useState(false);
  const [signup, setSignup] = useState<Signup | null>(null);
  const [reset, setReset] = useState<'off' | 'email' | 'code'>('off');
  const [fresh, setFresh] = useState('');
  const emailAuth = site.email_auth === undefined ? 'code' : site.email_auth;
  const confirmJoin = site.email_confirm === true;
  const [human, setHuman] = useState<string | null>(null);
  const [humanRound, setHumanRound] = useState(0);
  const humanCheck: CaptchaWidget | null = site.captcha ? { provider: site.captcha.provider, site_key: site.captcha.site_key, action: emailAuth === 'password' ? 'signup' : 'login' } : null;
  const [asked, setAsked] = useState(false);
  const spare: CaptchaWidget | null = asked && site.captcha_spare ? { provider: site.captcha_spare.provider, site_key: site.captcha_spare.site_key, action: 'signup' } : null;
  const freshHuman = () => {
    setHuman(null);
    setHumanRound((n) => n + 1);
  };
  const askHuman = (err: unknown): boolean => {
    if (!(err instanceof ApiError) || !site.captcha_spare || !HUMAN_CODES.includes(err.code)) {
      return false;
    }
    freshHuman();
    if (err.code !== 'antibot.captcha' || spare) {
      return false;
    }
    setAsked(true);
    return true;
  };
  const siteOpen = site.client_access?.site_login !== false;
  const timer = useRef<number | null>(null);
  const navigate = useNavigate();
  const [back] = useState<OauthProvider | null>(returnedProvider);
  const [finishing, setFinishing] = useState(back !== null);
  const [leaving, setLeaving] = useState<OauthProvider | null>(null);
  const finished = useRef(false);
  const providers = (['google', 'apple'] as const).filter((p) => site.oauth?.[p]);

  useEffect(() => {
    reportVisit();
  }, []);

  const completeOauth = async (captcha?: string) => {
    setError(null);
    try {
      const res = await finishOauth(captcha);
      if (res.status === 'totp_required') {
        setChallenge(res.challenge);
        setTotp('');
      } else {
        navigate('/cabinet', { replace: true });
        onDone();
        return;
      }
    } catch (err) {
      if (askHuman(err)) {
        return;
      }
      setError(failText(err, t('Вход не завершён. Нажмите кнопку входа ещё раз.')));
      if (captcha && err instanceof ApiError && HUMAN_CODES.includes(err.code)) {
        return;
      }
    }
    setAsked(false);
    setFinishing(false);
    navigate('/cabinet', { replace: true });
  };

  useEffect(() => {
    if (!back || finished.current) {
      return;
    }
    finished.current = true;
    void completeOauth();
  }, [back]);

  const startOauth = async (provider: OauthProvider) => {
    setLeaving(provider);
    setError(null);
    try {
      const referral = storedReferral();
      const utm = storedUtm();
      await leaveFor(`/api/client/auth/oauth/${provider}`, { ...(referral ? { ref: referral } : {}), ...(utm ? { utm } : {}) });
    } catch (err) {
      setError(failText(err, t('Вход не начался')));
      setLeaving(null);
    }
  };

  useEffect(
    () => () => {
      if (timer.current) {
        window.clearTimeout(timer.current);
      }
    },
    [],
  );

  useEffect(() => {
    if (wait <= 0) {
      return;
    }
    const tick = window.setTimeout(() => setWait((left) => Math.max(0, left - 1)), 1000);
    return () => window.clearTimeout(tick);
  }, [wait]);

  const poll = (token: string, expires: number) => {
    timer.current = window.setTimeout(async () => {
      if (Date.now() > expires + 5000) {
        setTgState('expired');
        return;
      }
      try {
        const res = await api<Status>('/api/client/auth/telegram/status', { method: 'POST', body: { poll_token: token } });
        if (res.status === 'confirmed') {
          onDone();
          return;
        }
        if (res.status === 'totp_required' && res.challenge) {
          setChallenge(res.challenge);
          setTotp('');
          setTgState('idle');
          return;
        }
        if (res.status === 'rejected' || res.status === 'expired') {
          setTgState(res.status);
          return;
        }
        poll(token, expires);
      } catch (err) {
        if (err instanceof ApiError && err.status === 429) {
          poll(token, expires);
          return;
        }
        setError(failText(err, t('Вход не подтвердился')));
        setTgState('idle');
      }
    }, 2500);
  };

  const startTelegram = async () => {
    setBusy(true);
    setError(null);
    try {
      const referral = storedReferral();
      const res = await api<Tg>('/api/client/auth/telegram', { method: 'POST', body: referral ? { ref: referral } : {} });
      setTg(res);
      setTgState('waiting');
      poll(res.poll_token, Date.parse(res.expires_at));
    } catch (err) {
      setError(err instanceof ApiError && err.code === 'bot.not_configured' ? t('Вход через Telegram пока не настроен. Попробуйте позже или войдите по почте.') : failText(err, t('Вход не начался')));
    } finally {
      setBusy(false);
    }
  };

  const requestCode = async (again: boolean) => {
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ email?: string }>('/api/client/auth/email', { method: 'POST', body: { email: email.trim(), ...(human ? { captcha: human } : {}) } });
      setSent({ email: res.email ?? email.trim(), again });
      setWait(RESEND_SECONDS);
      if (again) {
        setCode('');
      }
    } catch (err) {
      setError(err instanceof ApiError && err.status === 501 ? t('Вход по почте пока недоступен. Войдите через Telegram.') : failText(err, t('Код не отправился')));
    } finally {
      if (humanCheck) {
        freshHuman();
      }
      setBusy(false);
    }
  };

  const sendCode = (event: FormEvent) => {
    event.preventDefault();
    void requestCode(false);
  };

  const changeEmail = () => {
    setSent(null);
    setCode('');
    setError(null);
  };

  const verify = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const res = await api<Verified>('/api/client/auth/email/verify', {
        method: 'POST',
        body: { email: email.trim(), code: code.trim(), utm: storedUtm() ?? undefined, ref: storedReferral() ?? undefined, ...(spare && human ? { captcha: human } : {}) },
      });
      if (res.status === 'totp_required' && res.challenge) {
        setChallenge(res.challenge);
        setTotp('');
        return;
      }
      onDone();
    } catch (err) {
      if (!askHuman(err)) {
        setError(failText(err, t('Код не подошёл')));
      }
    } finally {
      setBusy(false);
    }
  };

  const byPassword = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const res = await api<Registered>(joining ? '/api/client/auth/email/register' : '/api/client/auth/email/password', {
        method: 'POST',
        body: joining
          ? { email: email.trim(), password, utm: storedUtm() ?? undefined, ref: storedReferral() ?? undefined, ...(human ? { captcha: human } : {}) }
          : { email: email.trim(), password },
      });
      if (res.status === 'code_sent' && res.signup_token) {
        setSignup({ email: res.email ?? email.trim(), token: res.signup_token, again: false });
        setCode('');
        setWait(RESEND_SECONDS);
        if (humanCheck) {
          freshHuman();
        }
        return;
      }
      if (res.status === 'totp_required' && res.challenge) {
        setChallenge(res.challenge);
        setTotp('');
        return;
      }
      onDone();
    } catch (err) {
      if (err instanceof ApiError && err.code === 'auth.email_taken') {
        setJoining(false);
      }
      if (!(joining && askHuman(err))) {
        setError(failText(err, joining ? t('Аккаунт не создался') : t('Войти не получилось')));
      }
      if (joining && (humanCheck || spare)) {
        freshHuman();
      }
    } finally {
      setBusy(false);
    }
  };

  const finishSignup = async (event: FormEvent) => {
    event.preventDefault();
    if (!signup) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<Verified>('/api/client/auth/email/register/confirm', { method: 'POST', body: { email: signup.email, signup_token: signup.token, code, ...(spare && human ? { captcha: human } : {}) } });
      if (res.status === 'totp_required' && res.challenge) {
        setSignup(null);
        setChallenge(res.challenge);
        setTotp('');
        return;
      }
      onDone();
    } catch (err) {
      if (err instanceof ApiError && (err.code === 'auth.signup_expired' || err.code === 'auth.email_taken')) {
        setSignup(null);
        setCode('');
        if (err.code === 'auth.email_taken') {
          setJoining(false);
        }
      }
      if (!askHuman(err)) {
        setError(failText(err, t('Код не подошёл')));
      }
    } finally {
      setBusy(false);
    }
  };

  const resendSignup = async () => {
    if (!signup) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<{ email: string; signup_token: string }>('/api/client/auth/email/register/resend', { method: 'POST', body: { email: signup.email, signup_token: signup.token } });
      setSignup({ email: res.email, token: res.signup_token, again: true });
      setCode('');
      setWait(RESEND_SECONDS);
    } catch (err) {
      if (err instanceof ApiError && err.code === 'auth.signup_expired') {
        setSignup(null);
        setCode('');
      }
      setError(failText(err, t('Код не отправился')));
    } finally {
      setBusy(false);
    }
  };

  const leaveSignup = () => {
    setSignup(null);
    setCode('');
    setError(null);
  };

  const resetStart = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api('/api/client/auth/email/reset', { method: 'POST', body: { email: email.trim() } });
      setCode('');
      setFresh('');
      setReset('code');
    } catch (err) {
      setError(failText(err, t('Код не отправился')));
    } finally {
      setBusy(false);
    }
  };

  const resetConfirm = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      const res = await api<Verified>('/api/client/auth/email/reset/confirm', {
        method: 'POST',
        body: { email: email.trim(), code: code.trim(), password: fresh },
      });
      setReset('off');
      if (res.status === 'totp_required' && res.challenge) {
        setChallenge(res.challenge);
        setTotp('');
        return;
      }
      onDone();
    } catch (err) {
      setError(failText(err, t('Пароль не сменился')));
    } finally {
      setBusy(false);
    }
  };

  const restart = () => {
    setChallenge(null);
    setTotp('');
    setTg(null);
    setTgState('idle');
    setSent(null);
    setCode('');
    setError(null);
  };

  const sendTotp = async (event: FormEvent) => {
    event.preventDefault();
    if (!challenge) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await api('/api/client/auth/totp', { method: 'POST', body: { challenge, code: totp } });
      onDone();
    } catch (err) {
      if (err instanceof ApiError && (err.code === 'auth.login_expired' || err.code === 'auth.code_attempts')) {
        restart();
      }
      setTotp('');
      setError(failText(err, t('Код не подошёл')));
    } finally {
      setBusy(false);
    }
  };

  const [noAppBefore, noAppAfter] = t('Нет доступа к приложению — {link}, вход по коду сбросят.').split('{link}');
  const [forgotBefore, forgotAfter] = t('Забыли пароль — {link}.').split('{link}');

  const help = (
    <span className="cb-login-help cb-muted">
      {noAppBefore}
      {site.support_url ? (
        <a className="cb-accent-link" href={site.support_url}>
          {t('напишите в поддержку')}
        </a>
      ) : (
        t('напишите в поддержку')
      )}
      {noAppAfter}
    </span>
  );

  const humanAsk = (onToken: (token: string) => void) =>
    spare ? (
      <>
        <p className="cb-login-note" role="status">
          {t('Сейчас создаётся много новых аккаунтов, поэтому нужна проверка.')}
        </p>
        <Captcha widget={spare} round={humanRound} onToken={onToken} onExpire={() => setHuman(null)} />
      </>
    ) : null;

  return (
    <div className="cb-solo" data-part="login">
      <div className="cb-solo-box cb-login">
        <div className="cb-brand" aria-hidden="true">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="cb-mark" />
          <span className="cb-brand-name">{site.brand_name.toUpperCase()}</span>
        </div>
        <h1>{t('Вход в кабинет')}</h1>
        {!siteOpen ? (
          <div className="cb-login-wait">
            <p>{t('Вход на сайт сейчас выключен. Подписка, оплата и поддержка доступны в Telegram-боте.')}</p>
            {site.bot_username ? (
              <a className="cb-main" href={botLink(site.bot_username, startParam(storedUtm(), storedReferral()))} target="_blank" rel="noreferrer">
                {t('Открыть бота')}
              </a>
            ) : null}
          </div>
        ) : null}
        {siteOpen && finishing && back ? (
          <div className="cb-login-wait" aria-busy={spare ? undefined : true}>
            {spare ? humanAsk((token) => void completeOauth(token)) : <p>{t('Завершаем вход через {provider}…', { provider: PROVIDER_TITLE[back] })}</p>}
          </div>
        ) : null}
        {siteOpen && challenge ? (
          <form className="cb-login-form" onSubmit={sendTotp}>
            <p>{t('Вход защищён кодом. Откройте приложение-аутентификатор и введите код {brand}.', { brand: site.brand_name })}</p>
            <label className="cb-field">
              <span>{t('Код из приложения')}</span>
              <input
                value={totp}
                onChange={(e) => setTotp(e.target.value.replace(/\D/g, '').slice(0, 6))}
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder={t('6 цифр')}
                autoFocus
              />
            </label>
            {error ? (
              <p className="cb-error" role="alert">
                {error}
              </p>
            ) : null}
            <button type="submit" className="cb-main" disabled={busy || totp.length !== 6}>
              {busy ? t('Проверяем…') : t('Войти')}
            </button>
            <button type="button" className="cb-text" onClick={restart} disabled={busy}>
              {t('Начать вход заново')}
            </button>
            {help}
          </form>
        ) : null}
        {!siteOpen || challenge || finishing || emailAuth === null ? null : (
        <div role="group" aria-label={t('Способ входа')} className="cb-seg">
          <button type="button" aria-pressed={mode === 'telegram'} onClick={() => setMode('telegram')}>
            Telegram
          </button>
          <button type="button" aria-pressed={mode === 'email'} onClick={() => setMode('email')}>
            {t('Почта')}
          </button>
        </div>
        )}
        {!siteOpen || challenge || finishing ? null : mode === 'telegram' ? (
          tg && tgState === 'waiting' ? (
            <div className="cb-login-wait">
              <p>{t('Откройте бота и подтвердите вход. Страница обновится сама.')}</p>
              <a className="cb-main" href={tg.link} target="_blank" rel="noreferrer">
                {t('Открыть Telegram')}
              </a>
              <QrCode text={tg.link} size={180} label={t('QR-код для входа через Telegram')} />
              <span className="cb-muted">{t('На компьютере отсканируйте код телефоном')}</span>
            </div>
          ) : (
            <div className="cb-login-wait">
              {tgState === 'rejected' ? <p className="cb-warn">{t('Вход отклонён в боте.')}</p> : null}
              {tgState === 'expired' ? <p className="cb-muted">{t('Время на подтверждение вышло, начните заново.')}</p> : null}
              <p>{t('Бот пришлёт запрос на вход — подтвердите его, и кабинет откроется.')}</p>
              <button type="button" className="cb-main" onClick={() => void startTelegram()} disabled={busy}>
                {busy ? t('Готовим вход…') : t('Войти через Telegram')}
              </button>
            </div>
          )
        ) : emailAuth === 'password' && reset === 'email' ? (
          <form className="cb-login-form" onSubmit={resetStart}>
            <p className="cb-login-note">{t('Пришлём на почту код — с ним зададите новый пароль.')}</p>
            <label className="cb-field">
              <span>{t('Почта')}</span>
              <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} autoComplete="email" placeholder="you@example.com" />
            </label>
            <button type="submit" className="cb-main" disabled={busy || !email.includes('@')}>
              {busy ? t('Отправляем…') : t('Прислать код')}
            </button>
            <button type="button" className="cb-text" onClick={() => setReset('off')} disabled={busy}>
              {t('Вернуться ко входу')}
            </button>
          </form>
        ) : emailAuth === 'password' && reset === 'code' ? (
          <form className="cb-login-form" onSubmit={resetConfirm}>
            <p className="cb-login-note">{t('Если почта зарегистрирована, на {email} пришёл код. Он действует 15 минут.', { email: email.trim() })}</p>
            <label className="cb-field">
              <span>{t('Код из письма')}</span>
              <input value={code} onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))} inputMode="numeric" autoComplete="one-time-code" placeholder={t('6 цифр')} />
            </label>
            <label className="cb-field">
              <span>{t('Новый пароль')}</span>
              <input type="password" value={fresh} onChange={(e) => setFresh(e.target.value)} autoComplete="new-password" placeholder={t('Не короче 10 символов')} />
            </label>
            <button type="submit" className="cb-main" disabled={busy || code.length !== 6 || fresh.length < 10}>
              {busy ? t('Меняем…') : t('Сменить пароль и войти')}
            </button>
            <button type="button" className="cb-text" onClick={() => setReset('email')} disabled={busy}>
              {t('Прислать код ещё раз')}
            </button>
          </form>
        ) : emailAuth === 'password' && signup ? (
          <form className="cb-login-form" onSubmit={finishSignup}>
            <p className="cb-login-note" role="status">
              {signup.again
                ? t('Новое письмо отправлено на {email}. Код в нём действует 15 минут.', { email: signup.email })
                : t('Письмо отправлено на {email}. Код в нём действует 15 минут.', { email: signup.email })}
            </p>
            <label className="cb-field">
              <span>{t('Код из письма')}</span>
              <input
                value={code}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder={t('6 цифр')}
                autoFocus
              />
            </label>
            {humanAsk(setHuman)}
            <button type="submit" className="cb-main" disabled={busy || code.length !== 6 || (!!spare && !human)}>
              {busy ? t('Проверяем…') : t('Подтвердить и войти')}
            </button>
            <span className="cb-login-help cb-muted">{t('Если с этой почтой уже есть аккаунт, кода в письме не будет: там подсказка, как войти.')}</span>
            <button type="button" className="cb-text" onClick={() => void resendSignup()} disabled={busy || wait > 0}>
              {wait > 0 ? t('Отправить ещё раз через {seconds} с', { seconds: String(wait) }) : t('Отправить ещё раз')}
            </button>
            <button type="button" className="cb-text" onClick={leaveSignup} disabled={busy}>
              {t('Изменить почту')}
            </button>
          </form>
        ) : emailAuth === 'password' ? (
          <form className="cb-login-form" onSubmit={byPassword}>
            <div role="group" aria-label={t('Есть ли аккаунт')} className="cb-seg">
              <button type="button" aria-pressed={!joining} onClick={() => setJoining(false)}>
                {t('Вход')}
              </button>
              <button type="button" aria-pressed={joining} onClick={() => setJoining(true)}>
                {t('Регистрация')}
              </button>
            </div>
            <label className="cb-field">
              <span>{t('Почта')}</span>
              <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} autoComplete="email" placeholder="you@example.com" />
            </label>
            <label className="cb-field">
              <span>{t('Пароль')}</span>
              <input
                type="password"
                value={password}
                onChange={(e) => setPassword(e.target.value)}
                autoComplete={joining ? 'new-password' : 'current-password'}
                placeholder={joining ? t('Не короче 10 символов') : ''}
              />
            </label>
            {joining && humanCheck ? <Captcha widget={humanCheck} round={humanRound} onToken={setHuman} onExpire={() => setHuman(null)} /> : null}
            {joining && !humanCheck ? humanAsk(setHuman) : null}
            <button type="submit" className="cb-main" disabled={busy || !email.includes('@') || password.length < (joining ? 10 : 1) || (joining && !!(humanCheck ?? spare) && !human)}>
              {busy ? (joining ? (confirmJoin ? t('Отправляем…') : t('Создаём…')) : t('Входим…')) : joining ? t('Создать аккаунт') : t('Войти')}
            </button>
            {joining && confirmJoin ? <span className="cb-login-help cb-muted">{t('Пришлём на почту код: аккаунт появится, когда вы его введёте.')}</span> : null}
            {joining ? null : site.email_reset ? (
              <button
                type="button"
                className="cb-text"
                onClick={() => {
                  setError(null);
                  setReset('email');
                }}
                disabled={busy}
              >
                {t('Забыли пароль?')}
              </button>
            ) : (
              <span className="cb-login-help cb-muted">
                {forgotBefore}
                {site.support_url ? (
                  <a className="cb-accent-link" href={site.support_url}>
                    {t('напишите в поддержку')}
                  </a>
                ) : (
                  t('напишите в поддержку')
                )}
                {forgotAfter}
              </span>
            )}
          </form>
        ) : sent ? (
          <form className="cb-login-form" onSubmit={verify}>
            <p className="cb-login-note" role="status">
              {sent.again ? t('Новый код отправлен на {email}. Он действует 15 минут.', { email: sent.email }) : t('Код отправлен на {email}. Он действует 15 минут.', { email: sent.email })}
            </p>
            <label className="cb-field">
              <span>{t('Код из письма')}</span>
              <input
                value={code}
                onChange={(e) => setCode(e.target.value.replace(/\D/g, '').slice(0, 6))}
                inputMode="numeric"
                autoComplete="one-time-code"
                placeholder={t('6 цифр')}
                autoFocus
              />
            </label>
            {humanAsk(setHuman)}
            <button type="submit" className="cb-main" disabled={busy || code.length !== 6 || (!!spare && !human)}>
              {t('Войти')}
            </button>
            {humanCheck && wait === 0 ? <Captcha widget={humanCheck} round={humanRound} onToken={setHuman} onExpire={() => setHuman(null)} /> : null}
            <button type="button" className="cb-text" onClick={() => void requestCode(true)} disabled={busy || wait > 0 || (!!humanCheck && !human)}>
              {wait > 0 ? t('Отправить ещё раз через {seconds} с', { seconds: String(wait) }) : t('Отправить ещё раз')}
            </button>
            <button type="button" className="cb-text" onClick={changeEmail} disabled={busy}>
              {t('Изменить почту')}
            </button>
          </form>
        ) : (
          <form className="cb-login-form" onSubmit={sendCode}>
            <label className="cb-field">
              <span>{t('Почта')}</span>
              <input type="email" value={email} onChange={(e) => setEmail(e.target.value)} autoComplete="email" placeholder="you@example.com" />
            </label>
            {humanCheck ? <Captcha widget={humanCheck} round={humanRound} onToken={setHuman} onExpire={() => setHuman(null)} /> : null}
            <button type="submit" className="cb-main" disabled={busy || !email.includes('@') || (!!humanCheck && !human)}>
              {t('Прислать код')}
            </button>
          </form>
        )}
        {!siteOpen || challenge || finishing || providers.length === 0 ? null : (
          <div className="cb-oauth" role="group" aria-label={t('Другие способы входа')}>
            <span className="cb-oauth-or">{t('или')}</span>
            {providers.map((p) => (
              <button key={p} type="button" className="cb-second cb-oauth-btn" onClick={() => void startOauth(p)} disabled={busy || leaving !== null}>
                <ProviderMark provider={p} />
                {leaving === p ? t('Переходим…') : t('Войти через {provider}', { provider: PROVIDER_TITLE[p] })}
              </button>
            ))}
          </div>
        )}
        {error && !challenge ? (
          <p className="cb-error" role="alert">
            {error}
          </p>
        ) : null}
        {langSwitch ? (
          <button type="button" className="cb-text cb-lang" lang={langSwitch.lang} onClick={langSwitch.onPick}>
            {NATIVE[langSwitch.lang]}
          </button>
        ) : null}
      </div>
    </div>
  );
}
