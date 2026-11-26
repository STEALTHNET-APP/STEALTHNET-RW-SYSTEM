import { useCallback, useEffect, useState } from 'react';
import { useParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { Captcha, type CaptchaWidget } from '../lib/captcha';
import { guessLang, msg, siteLangs, t, useApplied } from '../lib/i18n';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, type Theme } from '../lib/theme';
import { loadTelegram, webApp } from '../mini/telegram';
import './verify.css';

type VerifyView = {
  purpose: string;
  expires_at: string;
  captcha: CaptchaWidget;
  back: { bot: string | null };
  locale?: 'ru' | 'en';
};

type Phase = 'ready' | 'checking' | 'done';

const RETRY_CODES = ['antibot.failed', 'antibot.unavailable', 'auth.rate_limited'];
const EXPIRED = msg('Ссылка на проверку устарела. Вернитесь в бот и запросите пробный период ещё раз.');
const NO_CHECK = msg('Не получилось проверить. Проверьте интернет и попробуйте ещё раз.');
const NO_PAGE = msg('Не получилось открыть проверку. Проверьте интернет и обновите страницу.');

function lost(err: unknown): boolean {
  return !(err instanceof ApiError) || err.status === 0 || (err.status >= 500 && err.code !== 'maintenance');
}

function reason(err: unknown, fallback: string): string {
  if (lost(err) || !(err instanceof ApiError)) {
    return fallback;
  }
  return err.code === 'antibot.expired' ? EXPIRED : err.message;
}

function again(err: unknown): boolean {
  return lost(err) || (err instanceof ApiError && RETRY_CODES.includes(err.code));
}

function clock(iso: string): string {
  const at = new Date(iso);
  return Number.isNaN(at.getTime()) ? '' : at.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
}

function useVerify(token: string) {
  const [view, setView] = useState<VerifyView | null>(null);
  const [fatal, setFatal] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const [phase, setPhase] = useState<Phase>('ready');
  const [round, setRound] = useState(0);
  const [offline, setOffline] = useState(false);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let gone = false;
    api<VerifyView>('/api/public/verify', { method: 'POST', body: { token } })
      .then((data) => {
        if (!gone) {
          setView(data);
          setFatal(null);
        }
      })
      .catch((err) => {
        if (!gone) {
          setOffline(again(err));
          setFatal(reason(err, NO_PAGE));
        }
      });
    return () => {
      gone = true;
    };
  }, [token, attempt]);

  const reopen = useCallback(() => setAttempt((n) => n + 1), []);

  const pass = useCallback(
    async (captcha: string) => {
      setPhase('checking');
      setNotice(null);
      try {
        await api('/api/public/verify/pass', { method: 'POST', body: { token, captcha } });
        setPhase('done');
      } catch (err) {
        setPhase('ready');
        if (again(err)) {
          setNotice(reason(err, NO_CHECK));
          setRound((n) => n + 1);
        } else {
          setOffline(false);
          setFatal(reason(err, NO_CHECK));
        }
      }
    },
    [token],
  );

  return { view, fatal, notice, phase, round, pass, offline, reopen };
}

function useInTelegram(): boolean {
  const [inside, setInside] = useState(false);
  useEffect(() => {
    void loadTelegram().then((app) => {
      if (app) {
        app.ready();
        app.expand?.();
        setInside(true);
      }
    });
  }, []);
  return inside;
}

function BackToBot({ bot, inside, className }: { bot: string | null; inside: boolean; className: string }) {
  if (inside) {
    return (
      <button type="button" className={className} onClick={() => webApp()?.close?.()}>
        {t('Вернуться в бот')}
      </button>
    );
  }
  if (!bot) {
    return null;
  }
  return (
    <a className={className} href={bot}>
      {t('Вернуться в бот')}
    </a>
  );
}

type LayoutProps = {
  site: typeof DEFAULT_SITE;
  view: VerifyView;
  flow: ReturnType<typeof useVerify>;
  inside: boolean;
  theme: Theme;
};

function CardLayout({ site, view, flow, inside, theme }: LayoutProps) {
  const until = clock(view.expires_at);
  const [paused, setPaused] = useState(false);
  useEffect(() => {
    if (flow.notice) {
      setPaused(true);
    }
  }, [flow.notice, flow.round]);
  return (
    <main className="vf">
      <div className="vf-box">
        <div className="vf-brand">
          <BrandMark name={site.brand_name} logo={site.logo_url} className="vf-mark" />
          <span>{site.brand_name.toUpperCase()}</span>
        </div>
        {flow.phase === 'done' ? (
          <section className="vf-card" aria-live="polite">
            <h1 className="vf-title">{t('Пробный период включён')}</h1>
            <p className="vf-sub">{t('Подписка появится через минуту, сообщение о ней придёт в бот.')}</p>
            <BackToBot bot={view.back.bot} inside={inside} className="vf-btn" />
          </section>
        ) : (
          <section className="vf-card" aria-labelledby="vf-title">
            <h1 id="vf-title" className="vf-title">
              {t('Проверка перед пробным периодом')}
            </h1>
            <p className="vf-sub">{t('Пройдите проверку «Я не робот», и пробный период включится сразу.')}</p>
            {paused ? (
              <button type="button" className="vf-btn" onClick={() => setPaused(false)}>
                {t('Пройти проверку ещё раз')}
              </button>
            ) : (
              <Captcha widget={view.captcha} theme={theme} round={flow.round} onToken={(token) => void flow.pass(token)} className="vf-widget" />
            )}
            {flow.phase === 'checking' ? (
              <p className="vf-status" role="status">
                {t('Проверяем и включаем пробный период…')}
              </p>
            ) : null}
            {flow.notice ? (
              <p className="vf-warn" role="alert">
                {t(flow.notice)}
              </p>
            ) : null}
          </section>
        )}
        {flow.phase === 'done' ? null : (
          <p className="vf-note">
            {until ? t('Ссылка на проверку действует до {time}. ', { time: until }) : null}
            {t('Проверка нужна, чтобы пробный период доставался людям, а не ботам.')}
          </p>
        )}
      </div>
    </main>
  );
}

export function VerifyPage() {
  const { token = '' } = useParams();
  const loadedSite = useSite();
  const site = loadedSite ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const phone = usePhone();
  const choice = site.appearance.client_theme;
  const stored = readStored('sn.client.theme');
  const personal: Theme | null = stored === 'dark' || stored === 'light' ? stored : null;
  const theme: Theme = choice === 'dark' || choice === 'light' ? choice : (personal ?? system);
  useSurface(phone ? 'cabinet-phone' : 'cabinet', theme, site.appearance.client === 'mono' ? 'b' : 'a');
  const flow = useVerify(token);
  const inside = useInTelegram();
  const langs = siteLangs(loadedSite);
  const account = flow.view?.locale && langs.includes(flow.view.locale) ? flow.view.locale : null;
  const ready = useApplied(loadedSite && (flow.view || flow.fatal) ? (account ?? guessLang(loadedSite)) : null);

  if (!ready || (!flow.view && !flow.fatal)) {
    return <main className="vf" aria-busy="true" />;
  }
  if (flow.fatal || !flow.view) {
    return (
      <main className="vf">
        <div className="vf-box">
          <div className="vf-brand">
            <BrandMark name={site.brand_name} logo={site.logo_url} className="vf-mark" />
            <span>{site.brand_name.toUpperCase()}</span>
          </div>
          <section className="vf-card">
            <h1 className="vf-title">{t('Проверка недоступна')}</h1>
            <p className="vf-sub" role="alert">
              {flow.fatal ? t(flow.fatal) : null}
            </p>
            {flow.offline ? (
              <button type="button" className="vf-btn" onClick={flow.reopen}>
                {t('Повторить')}
              </button>
            ) : (
              <BackToBot bot={flow.view?.back.bot ?? (site.bot_username ? `https://t.me/${site.bot_username}` : null)} inside={inside} className="vf-btn" />
            )}
          </section>
        </div>
      </main>
    );
  }
  return <CardLayout site={site} view={flow.view} flow={flow} inside={inside} theme={theme} />;
}
