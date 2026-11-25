import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState } from 'react';
import { Link, Route, Routes, useLocation, useNavigate } from 'react-router';
import { api, ApiError, setBearer } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { applyLang, guessLang, likelyLang, otherLang, rememberLang, siteLangs, t, useApplied, useLang, type Lang } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { styleFile, useOwnStyles } from '../lib/ownStyles';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { supportTickets } from '../lib/support';
import { useSupportUnread } from '../lib/supportUnread';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { Site } from '../lib/types';
import type { ClientMe, Device, SubscriptionView } from '../cabinet/model';
import { liveSubs, pickSub, readRemembered, remember as rememberSub, subName, subState, takeBetterStart, useShowPressed } from '../cabinet/subs';
import { initData, launchData, loadTelegram, openExternal, paintTelegram, telegramLanguage, telegramName, telegramUsername, webApp, type WebApp } from './telegram';
import { backTarget } from './back';
import { currentTab, normalizeConfig, pick, SECTION_ICONS, sectionPath, type MiniConfig } from './config';
import { watchInsets } from './insets';
import { fullscreenOn, holdFullscreen, holdScreen } from './screen';
import { lockZoom } from './zoom';
import { MiniConnect } from './MiniConnect';
import { MiniFriends } from './MiniFriends';
import { MiniHome } from './MiniHome';
import { MiniPayments } from './MiniPayments';
import { MiniAddons } from './MiniAddons';
import { MiniGifts } from './MiniGifts';
import { MiniPlans } from './MiniPlans';
import { MiniProfile } from './MiniProfile';
import { MiniPromo } from './MiniPromo';
import { MiniEmail, MiniLanguage, MiniPassword, MiniTotp } from './MiniAccount';
import { MiniTopUp } from './MiniTopUp';
import { MiniTrial } from './MiniTrial';
import { MiniRoulette } from './MiniRoulette';
import { MiniWithdraw } from './MiniWithdraw';
import { MiniNewTicket, MiniSupport, MiniTicket } from './MiniSupport';
import { supportTarget, tabLabel, unreadText, type Extras } from './sections';
import './mini.css';
import './support.css';

const TOKEN_KEY = 'sn.mini.token';
const THEME_KEY = 'sn.mini.theme';
export const MINI_SUB_KEY = 'sn.mini.sub';

type Login = { token: string };

type Detail = { view: SubscriptionView; devices: Device[] | null };

export type MiniCtx = {
  me: ClientMe;
  site: Site;
  subs: SubscriptionView[];
  view: SubscriptionView | null;
  devices: Device[] | null;
  ready: boolean;
  choose: (id: number) => void;
  setView: (view: SubscriptionView) => void;
  setDevices: (devices: Device[]) => void;
  reload: () => Promise<void>;
  theme: Theme;
  flipTheme: (() => void) | null;
  lang: Lang;
  nextLang: Lang | null;
  switchLang: (next: Lang) => void;
  config: MiniConfig;
  name: string | null;
  username: string | null;
  unread: number;
  setUnread: (tickets: number) => void;
  demo: Extras | null;
};

export const MiniContext = createContext<MiniCtx | null>(null);

export function useMini(): MiniCtx {
  const value = useContext(MiniContext);
  if (!value) {
    throw new Error('нет мини-приложения');
  }
  return value;
}

function stored(): string | null {
  try {
    return window.sessionStorage.getItem(TOKEN_KEY);
  } catch {
    return null;
  }
}

function remember(token: string | null) {
  try {
    if (token) {
      window.sessionStorage.setItem(TOKEN_KEY, token);
    } else {
      window.sessionStorage.removeItem(TOKEN_KEY);
    }
  } catch {
    return;
  }
}

type State = { kind: 'loading' } | { kind: 'outside' } | { kind: 'blocked' } | { kind: 'error'; message: string } | { kind: 'in'; me: ClientMe };

export function MiniApp() {
  const loadedSite = useSite();
  const site = loadedSite ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [subs, setSubs] = useState<SubscriptionView[]>([]);
  const [chosenId, setChosenId] = useState<number | null>(null);
  const cache = useRef(new Map<number, Detail>());
  const [, setTick] = useState(0);
  const [unread, setUnread] = useSupportUnread(state.kind === 'in' && supportTickets(site));
  const [tg, setTg] = useState<WebApp | null>(null);
  const [tgScheme, setTgScheme] = useState<Theme | null>(null);
  const [personal, setPersonal] = useState<Theme | null>(() => {
    const v = readStored(THEME_KEY);
    return v === 'dark' || v === 'light' ? v : null;
  });
  const [picked, setPicked] = useState<Lang | null>(null);
  const langs = siteLangs(loadedSite);
  const account = state.kind === 'in' && langs.includes(state.me.locale as Lang) ? (state.me.locale as Lang) : null;
  const wanted: Lang | null =
    loadedSite && state.kind !== 'loading' ? (picked && langs.includes(picked) ? picked : (account ?? guessLang(loadedSite, telegramLanguage()))) : null;
  const applied = useApplied(wanted);
  const lang = useLang();

  const fetchDetail = useCallback(async (live: SubscriptionView) => {
    const id = live.subscription.id;
    const [view, devices] = await Promise.all([
      api<SubscriptionView>(`/api/client/subscriptions/${id}`).catch(() => live),
      api<Device[]>(`/api/client/subscriptions/${id}/devices`).catch(() => null),
    ]);
    cache.current.set(id, { view, devices });
    setTick((n) => n + 1);
  }, []);

  const loadData = useCallback(async () => {
    const me = await api<ClientMe>('/api/client/me');
    const list = liveSubs(await api<SubscriptionView[]>('/api/client/subscriptions'));
    const chosen = pickSub(list, takeBetterStart(MINI_SUB_KEY, list) ?? readRemembered(MINI_SUB_KEY));
    setState({ kind: 'in', me });
    setUnread(me.support_unread ?? 0);
    setSubs(list);
    setChosenId(chosen?.subscription.id ?? null);
    if (chosen) {
      await fetchDetail(chosen);
    }
  }, [fetchDetail, setUnread]);

  const choose = useCallback(
    (id: number) => {
      const next = subs.find((v) => v.subscription.id === id);
      if (!next) {
        return;
      }
      rememberSub(MINI_SUB_KEY, id);
      setChosenId(id);
      void fetchDetail(next);
    },
    [subs, fetchDetail],
  );

  const base = subs.find((v) => v.subscription.id === chosenId) ?? null;
  const hit = chosenId === null ? undefined : cache.current.get(chosenId);

  const putDetail = useCallback(
    (patch: Partial<Detail>) => {
      if (!base) {
        return;
      }
      const id = base.subscription.id;
      cache.current.set(id, { ...(cache.current.get(id) ?? { view: base, devices: null }), ...patch });
      setTick((n) => n + 1);
    },
    [base],
  );
  const setView = useCallback((next: SubscriptionView) => putDetail({ view: next }), [putDetail]);
  const setDevices = useCallback((next: Device[]) => putDetail({ devices: next }), [putDetail]);

  const attach = useCallback((app: WebApp | null) => {
    setTg(app);
    if (app?.colorScheme) {
      setTgScheme(app.colorScheme);
    }
  }, []);

  const start = useCallback(async () => {
    attach(await loadTelegram(attach));
    try {
      const token = launchData() ? null : stored();
      const data = token ? '' : initData();
      if (token) {
        setBearer(token);
      } else if (data) {
        const login = await api<Login>('/api/client/auth/miniapp', { method: 'POST', body: { init_data: data }, lang: likelyLang(telegramLanguage()) });
        setBearer(login.token);
        remember(login.token);
      } else {
        setState({ kind: 'outside' });
        return;
      }
      await loadData();
    } catch (err) {
      if (err instanceof ApiError && err.status === 403) {
        setState({ kind: 'blocked' });
      } else if (err instanceof ApiError && err.status === 401) {
        remember(null);
        setState({ kind: 'outside' });
      } else {
        setState({ kind: 'error', message: err instanceof ApiError ? err.message : t('Сервер не ответил') });
      }
    }
  }, [attach, loadData]);

  useEffect(() => {
    void start();
  }, [start]);

  const styled = useOwnStyles(styleFile((site.miniapp as { css?: unknown } | null | undefined)?.css, 'miniapp'));
  const shown = state.kind !== 'loading' && applied && styled;

  const choice = site.appearance.client_theme;
  const fixed = choice === 'dark' || choice === 'light' ? choice : null;
  const theme: Theme = fixed ?? personal ?? tgScheme ?? system;
  useSurface('mini', theme, site.appearance.client === 'mono' ? 'b' : 'a');

  useEffect(() => (tg ? watchInsets(tg, document.documentElement) : undefined), [tg]);
  useEffect(() => (tg ? holdScreen(tg, document.documentElement) : undefined), [tg]);
  const fullscreen = fullscreenOn(loadedSite);
  useEffect(() => (tg && fullscreen ? holdFullscreen(tg) : undefined), [tg, fullscreen]);
  useEffect(() => (tg ? lockZoom(document, window.parent !== window) : undefined), [tg]);

  useEffect(() => {
    if (tg) {
      const root = document.querySelector('.mn') ?? document.documentElement;
      paintTelegram(tg, getComputedStyle(root).getPropertyValue('--bg').trim());
    }
  }, [tg, theme, shown]);

  const flipTheme = useCallback(() => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    writeStored(THEME_KEY, next);
    setPersonal(next);
  }, [theme]);

  const reload = useCallback(async () => {
    try {
      await loadData();
    } catch {
      return;
    }
  }, [loadData]);

  const switchLang = useCallback(
    (next: Lang) => {
      rememberLang(next);
      setPicked(next);
      void applyLang(next);
      api('/api/client/locale', { method: 'PUT', body: { locale: next } })
        .then(() => reload())
        .catch(() => undefined);
    },
    [reload],
  );

  const view = hit?.view ?? base;
  const devices = hit?.devices ?? null;
  const ready = hit !== undefined;
  const nextLang = otherLang(lang, langs);
  const config = useMemo(() => normalizeConfig(site.miniapp), [site.miniapp]);
  const name = useMemo(() => (state.kind === 'in' ? telegramName() : null), [state.kind]);
  const username = useMemo(() => (state.kind === 'in' ? telegramUsername() : null), [state.kind]);
  const ctx = useMemo<MiniCtx | null>(
    () =>
      state.kind === 'in'
        ? { me: state.me, site, subs, view, devices, ready, choose, setView, setDevices, reload, theme, flipTheme: fixed ? null : flipTheme, lang, nextLang, switchLang, config, name, username, unread, setUnread, demo: null }
        : null,
    [state, site, subs, view, devices, ready, choose, setView, setDevices, reload, theme, flipTheme, fixed, lang, nextLang, switchLang, config, name, username, unread, setUnread],
  );

  if (!shown) {
    return <div className="mn-blank" aria-busy="true" />;
  }
  if (!ctx) {
    const text =
      state.kind === 'outside'
        ? t('Откройте мини-приложение из бота в Telegram.')
        : state.kind === 'blocked'
          ? t('Аккаунт заблокирован. Напишите в поддержку.')
          : state.kind === 'error'
            ? state.message
            : '';
    return (
      <div className="mn mn-solo">
        <BrandMark name={site.brand_name} logo={site.logo_url} className="mn-logo" />
        <h1>{site.brand_name}</h1>
        <p>{text}</p>
        {state.kind === 'error' ? (
          <button type="button" className="mn-main" onClick={() => void start()}>
            {t('Повторить')}
          </button>
        ) : null}
        {state.kind === 'outside' && site.bot_username ? (
          <a className="mn-main" href={`https://t.me/${site.bot_username.replace(/^@/, '')}`}>
            {t('Открыть бота')}
          </a>
        ) : null}
      </div>
    );
  }

  return (
    <MiniContext.Provider value={ctx}>
      <div key={lang} className="mn">
        <Routes>
          <Route index element={<MiniHome />} />
          <Route path="plans" element={<MiniPlans />} />
          <Route path="trial" element={<MiniTrial />} />
          <Route path="connect" element={<MiniConnect />} />
          <Route path="profile" element={<MiniProfile />} />
          <Route path="friends" element={<MiniFriends />} />
          <Route path="profile/email" element={<MiniEmail />} />
          <Route path="profile/password" element={<MiniPassword />} />
          <Route path="profile/totp" element={<MiniTotp />} />
          <Route path="profile/language" element={<MiniLanguage />} />
          <Route path="topup" element={<MiniTopUp />} />
          <Route path="payments" element={<MiniPayments />} />
          <Route path="addons" element={<MiniAddons />} />
          <Route path="gifts" element={<MiniGifts />} />
          <Route path="promo" element={<MiniPromo />} />
          <Route path="roulette" element={<MiniRoulette />} />
          <Route path="withdraw" element={<MiniWithdraw />} />
          <Route path="support" element={<MiniSupport />} />
          <Route path="support/new" element={<MiniNewTicket />} />
          <Route path="support/:id" element={<MiniTicket />} />
          <Route path="*" element={<MiniHome />} />
        </Routes>
        <MiniNav />
        <TelegramBack app={tg} />
      </div>
    </MiniContext.Provider>
  );
}

function TelegramBack({ app }: { app: WebApp | null }) {
  const { pathname } = useLocation();
  const navigate = useNavigate();
  useEffect(() => {
    const back = app?.BackButton;
    if (!back) {
      return;
    }
    const target = backTarget(pathname);
    if (!target) {
      back.hide();
      return;
    }
    const go = () => {
      const idx = (window.history.state as { idx?: number } | null)?.idx ?? 0;
      if (idx > 0) {
        navigate(-1);
      } else {
        navigate(target, { replace: true });
      }
    };
    back.onClick(go);
    back.show();
    return () => back.offClick(go);
  }, [app, pathname, navigate]);
  useEffect(() => () => webApp()?.BackButton?.hide(), []);
  return null;
}

export function MiniNav({ path }: { path?: string }) {
  const { pathname } = useLocation();
  const { config, lang, site, unread } = useMini();
  const current = currentTab(config.tabs, path ?? pathname);
  const tabs = config.tabs.flatMap((tab) => {
    const target = tab.std === 'support' ? supportTarget(site) : { to: sectionPath(tab.std) };
    return target ? [{ tab, target }] : [];
  });
  const own = tabs.length !== 4 || tabs.some(({ tab }) => tab.label);
  const marked = unread > 0 && supportTickets(site) ? (tabs.some(({ tab }) => tab.std === 'support') ? 'support' : 'profile') : null;
  return (
    <nav aria-label={t('Меню')} className={own ? 'mn-nav own' : 'mn-nav'} data-part="tabs" style={tabs.length === 4 ? undefined : { gridTemplateColumns: `repeat(${tabs.length}, minmax(0, 1fr))` }}>
      {tabs.map(({ tab, target }) => {
        const inner = (
          <>
            <Icon name={SECTION_ICONS[tab.std]} size={23} />
            <span>{pick(tab.label, lang, tabLabel(tab.std))}</span>
            {marked === tab.std ? (
              <span className="mn-tab-dot">
                <span className="sr-only">{unreadText(unread)}</span>
              </span>
            ) : null}
          </>
        );
        return target.to ? (
          <Link key={tab.std} to={target.to} className="mn-tab press" aria-current={current === tab.std ? 'page' : undefined}>
            {inner}
          </Link>
        ) : (
          <button key={tab.std} type="button" className="mn-tab press" onClick={() => target.url && openExternal(target.url)}>
            {inner}
          </button>
        );
      })}
    </nav>
  );
}

export function ThemeCirc() {
  const { theme, flipTheme } = useMini();
  if (!flipTheme) {
    return null;
  }
  const label = theme === 'dark' ? t('Светлая тема') : t('Тёмная тема');
  return (
    <button type="button" className="mn-circ press" aria-label={label} onClick={flipTheme}>
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={19} />
    </button>
  );
}

export function BackHead({ title, to = '/app' }: { title: string; to?: string }) {
  return (
    <header className="mn-head">
      <Link to={to} className="mn-circ press back" aria-label={t('Назад')}>
        <Icon name="chevron-left" size={20} />
      </Link>
      <h1 className="mn-h1">{title}</h1>
      <ThemeCirc />
    </header>
  );
}

export function MiniSubSwitch({ part }: { part?: string }) {
  const { me, subs, view, choose } = useMini();
  const box = useRef<HTMLDivElement>(null);
  const chosenId = view?.subscription.id ?? null;
  useShowPressed(box, `${chosenId}:${subs.length}`);
  if (subs.length < 2) {
    return null;
  }
  const blocked = me.client.status === 'blocked';
  return (
    <div ref={box} role="group" aria-label={t('Подписки')} className="mn-subs" data-part={part}>
      {subs.map((v) => {
        const on = v.subscription.id === chosenId;
        const state = subState(v, blocked);
        return (
          <button key={v.subscription.id} type="button" className="mn-subs-item press" aria-pressed={on} onClick={() => !on && choose(v.subscription.id)}>
            <strong>{subName(v, subs)}</strong>
            <span className={`mn-tone ${state.tone}`}>{state.text}</span>
          </button>
        );
      })}
    </div>
  );
}
