import { createContext, useCallback, useContext, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { Link, Route, Routes, useLocation } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { documentTitle } from '../lib/documents';
import { avatarColor, money } from '../lib/format';
import { Icon } from '../lib/icons';
import { applyLang, guessLang, msg, NATIVE, otherLang, rememberLang, siteLangs, t, useApplied, useLang, type Lang } from '../lib/i18n';
import { usePhone } from '../lib/media';
import { styleFile, useOwnStyles } from '../lib/ownStyles';
import { maintenanceOf, useMaintenanceWatch } from '../lib/maintenance';
import { DEFAULT_SITE, showTitle, useSite } from '../lib/site';
import { supportLink, supportTickets } from '../lib/support';
import { useSupportUnread } from '../lib/supportUnread';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { Site } from '../lib/types';
import { AddonsPage } from './Addons';
import { DevicesPage } from './Devices';
import { FriendsPage } from './Friends';
import { GiftsPage } from './Gifts';
import { HelpPage } from './Help';
import { Login } from './Login';
import { CabinetMaintenance } from './Maintenance';
import { partOf, type Part } from './parts';
import { photoUrl, type ClientMe } from './model';
import { Pay } from './Pay';
import { ForcedPassword } from './ForcedPassword';
import { PaymentsHistory } from './PaymentHistory';
import { SecurityPage } from './Security';
import { SubscriptionPage } from './Subscription';
import { TopUpPage } from './TopUp';
import './cabinet.css';

const THEME_KEY = 'sn.client.theme';

type State = { kind: 'loading' } | { kind: 'guest' } | { kind: 'blocked' } | { kind: 'in'; me: ClientMe } | { kind: 'error'; message: string };

export type CabinetCtx = {
  me: ClientMe;
  site: Site;
  reload: () => Promise<void>;
  logout: () => Promise<void>;
  theme: Theme;
  flipTheme: () => void;
  phone: boolean;
  lang: Lang;
  nextLang: Lang | null;
  switchLang: (next: Lang) => void;
  unread: number;
  setUnread: (tickets: number) => void;
};

const Ctx = createContext<CabinetCtx | null>(null);

export function useCabinet(): CabinetCtx {
  const value = useContext(Ctx);
  if (!value) {
    throw new Error('нет кабинета');
  }
  return value;
}

function storedTheme(): Theme | null {
  const value = readStored(THEME_KEY);
  return value === 'dark' || value === 'light' ? value : null;
}

export function CabinetApp() {
  const loadedSite = useSite();
  const site = loadedSite ?? DEFAULT_SITE;
  const system = useSystemTheme();
  const phone = usePhone();
  const { pathname } = useLocation();
  const [state, setState] = useState<State>({ kind: 'loading' });
  const maintenance = maintenanceOf(loadedSite);
  const [unread, setUnread] = useSupportUnread(state.kind === 'in' && !maintenance && supportTickets(site), pathname);
  const [personal, setPersonal] = useState<Theme | null>(storedTheme);
  const [picked, setPicked] = useState<Lang | null>(null);
  const langs = siteLangs(loadedSite);
  const account = state.kind === 'in' && langs.includes(state.me.locale as Lang) ? (state.me.locale as Lang) : null;
  const wanted: Lang | null = loadedSite ? (picked && langs.includes(picked) ? picked : (account ?? guessLang(loadedSite))) : null;
  const ready = useApplied(wanted);
  const lang = useLang();
  const styled = useOwnStyles(styleFile(loadedSite?.cabinet?.css, 'cabinet'));
  const opened = ready && styled && state.kind !== 'loading';
  useMaintenanceWatch(maintenance !== null);
  useEffect(() => {
    if (opened) {
      showTitle(loadedSite, (known) => t('Личный кабинет — {brand}', { brand: known.brand_name }));
    }
  }, [opened, lang, loadedSite]);

  const load = useCallback(async () => {
    try {
      const me = await api<ClientMe>('/api/client/me');
      setUnread(me.support_unread ?? 0);
      setState(me.client.status === 'blocked' ? { kind: 'blocked' } : { kind: 'in', me });
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        setState({ kind: 'guest' });
      } else if (err instanceof ApiError && err.status === 403) {
        setState({ kind: 'blocked' });
      } else {
        setState({ kind: 'error', message: err instanceof ApiError ? err.message : t('Сервер не ответил') });
      }
    }
  }, [setUnread]);

  useEffect(() => {
    void load();
  }, [load]);

  const choice = site.appearance.client_theme;
  const fixed = choice === 'dark' || choice === 'light' ? choice : null;
  const theme: Theme = fixed ?? personal ?? system;
  useSurface(phone ? 'cabinet-phone' : 'cabinet', theme, site.appearance.client === 'mono' ? 'b' : 'a');

  const flipTheme = useCallback(() => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    writeStored(THEME_KEY, next);
    setPersonal(next);
  }, [theme]);

  const signedIn = state.kind === 'in';
  const accountLocale = state.kind === 'in' ? state.me.locale : null;
  useEffect(() => {
    if (!signedIn || !picked || picked === accountLocale) {
      return;
    }
    api('/api/client/locale', { method: 'PUT', body: { locale: picked } })
      .then(() => load())
      .catch(() => undefined);
  }, [signedIn, picked, accountLocale, load]);

  const switchLang = useCallback((next: Lang) => {
    rememberLang(next);
    setPicked(next);
    void applyLang(next);
  }, []);

  const logout = useCallback(async () => {
    try {
      await api('/api/client/auth/logout', { method: 'POST', body: {} });
    } finally {
      setState({ kind: 'guest' });
    }
  }, []);

  const nextLang = otherLang(lang, langs);
  const ctx = useMemo<CabinetCtx | null>(
    () =>
      state.kind === 'in'
        ? { me: state.me, site, reload: load, logout, theme, flipTheme: fixed ? () => undefined : flipTheme, phone, lang, nextLang, switchLang, unread, setUnread }
        : null,
    [state, site, load, logout, theme, flipTheme, fixed, phone, lang, nextLang, switchLang, unread, setUnread],
  );

  if (state.kind === 'loading' || !ready || !styled) {
    return <div className="cb-blank" aria-busy="true" />;
  }
  if (maintenance) {
    return <CabinetMaintenance site={site} mode={maintenance} />;
  }
  if (state.kind === 'guest') {
    return <Login key={lang} site={site} onDone={() => void load()} langSwitch={nextLang ? { lang: nextLang, onPick: () => switchLang(nextLang) } : null} />;
  }
  if (state.kind === 'blocked') {
    return (
      <div className="cb-solo">
        <div className="cb-solo-box">
          <h1>{t('Доступ закрыт')}</h1>
          <p>{t('Аккаунт заблокирован. Если это ошибка, напишите в поддержку.')}</p>
          {site.support_url ? (
            <a className="cb-main" href={site.support_url}>
              {t('Написать в поддержку')}
            </a>
          ) : null}
        </div>
      </div>
    );
  }
  if (state.kind === 'in' && state.me.must_change_password) {
    return <ForcedPassword onDone={() => void load()} onLogout={() => void logout()} />;
  }
  if (state.kind === 'error' || !ctx) {
    return (
      <div className="cb-solo">
        <div className="cb-solo-box" role="alert">
          <h1>{t('Кабинет не открылся')}</h1>
          <p>{state.kind === 'error' ? state.message : t('Сервер не ответил')}</p>
          <button type="button" className="cb-main" onClick={() => void load()}>
            {t('Повторить')}
          </button>
        </div>
      </div>
    );
  }

  return <CabinetShell ctx={ctx} />;
}

export function CabinetShell({ ctx }: { ctx: CabinetCtx }) {
  const { pathname } = useLocation();
  return (
    <Ctx.Provider value={ctx}>
      <div key={ctx.lang} className={ctx.phone ? 'cb cb-phone' : 'cb'}>
        {ctx.phone ? <PhoneHeader /> : <Header />}
        <main className="cb-main-area" data-part={partOf(pathname)}>
          <Routes>
            <Route index element={<SubscriptionPage />} />
            <Route path="pay" element={<Pay />} />
            <Route path="topup" element={<TopUpPage />} />
            <Route path="addons" element={<AddonsPage />} />
            <Route path="payments" element={<PaymentsHistory />} />
            <Route path="security" element={<SecurityPage />} />
            <Route path="devices" element={<DevicesPage />} />
            <Route path="friends" element={<FriendsPage />} />
            <Route path="gifts" element={<GiftsPage />} />
            <Route path="help" element={<HelpPage />} />
            <Route path="help/:id" element={<HelpPage />} />
            <Route path="*" element={<SubscriptionPage />} />
          </Routes>
        </main>
        <Footer />
      </div>
    </Ctx.Provider>
  );
}

const TABS: Array<{ key: string; label: string; to: string }> = [
  { key: 'sub', label: msg('Подписка'), to: '/cabinet' },
  { key: 'pay', label: msg('Тарифы'), to: '/cabinet/pay' },
  { key: 'devices', label: msg('Устройства'), to: '/cabinet/devices' },
  { key: 'friends', label: msg('Друзья'), to: '/cabinet/friends' },
  { key: 'gifts', label: msg('Подарки'), to: '/cabinet/gifts' },
  { key: 'help', label: msg('Помощь'), to: '/cabinet/help' },
];

let referralKnown: boolean | null = null;
let giftsKnown: boolean | null = null;

function useProbe(path: string, known: () => boolean | null, remember: (value: boolean) => void): boolean | null {
  const [value, setValue] = useState<boolean | null>(known());
  useEffect(() => {
    if (known() !== null) {
      return;
    }
    let alive = true;
    api(path)
      .then(() => {
        remember(true);
        if (alive) {
          setValue(true);
        }
      })
      .catch(() => {
        remember(false);
        if (alive) {
          setValue(false);
        }
      });
    return () => {
      alive = false;
    };
  }, [path, known, remember]);
  return value;
}

const knowReferral = () => referralKnown;
const rememberReferral = (value: boolean) => {
  referralKnown = value;
};
const knowGifts = () => giftsKnown;
const rememberGifts = (value: boolean) => {
  giftsKnown = value;
};

function useTabs() {
  const { site } = useCabinet();
  const referral = useProbe('/api/client/m/referral/me', knowReferral, rememberReferral);
  const gifts = useProbe('/api/client/m/gifts/offers', knowGifts, rememberGifts);
  const help = supportTickets(site) || supportLink(site) !== null;
  return TABS.filter((t) => (t.key === 'friends' ? referral === true : t.key === 'gifts' ? gifts === true : t.key === 'help' ? help : true));
}

function useAnswered(): number {
  const { site, unread } = useCabinet();
  return supportTickets(site) ? unread : 0;
}

function TabLabel({ tab, answered }: { tab: { key: string; label: string }; answered: number }) {
  if (tab.key !== 'help' || answered === 0) {
    return <>{t(tab.label)}</>;
  }
  return (
    <>
      {t(tab.label)}
      <span className="cb-tabdot" aria-label={answered === 1 ? t(', есть ответ поддержки') : t(', ответов поддержки: {n}', { n: answered })} />
    </>
  );
}

function currentTab(path: string): string {
  if (path.startsWith('/cabinet/security')) {
    return 'security';
  }
  if (path.startsWith('/cabinet/topup') || path.startsWith('/cabinet/payments')) {
    return 'money';
  }
  if (path.startsWith('/cabinet/pay')) {
    return 'pay';
  }
  if (path.startsWith('/cabinet/devices')) {
    return 'devices';
  }
  if (path.startsWith('/cabinet/friends')) {
    return 'friends';
  }
  if (path.startsWith('/cabinet/gifts')) {
    return 'gifts';
  }
  if (path.startsWith('/cabinet/help')) {
    return 'help';
  }
  return 'sub';
}

function ThemeButton({ className }: { className: string }) {
  const { theme, flipTheme, site } = useCabinet();
  const choice = site.appearance.client_theme;
  if (choice === 'dark' || choice === 'light') {
    return null;
  }
  const label = theme === 'dark' ? t('Светлая тема') : t('Тёмная тема');
  return (
    <button type="button" className={className} aria-label={label} title={label} onClick={flipTheme}>
      <Icon name={theme === 'dark' ? 'sun' : 'moon'} size={20} />
    </button>
  );
}

function Brand({ small }: { small?: boolean }) {
  const { site } = useCabinet();
  return (
    <Link to="/cabinet" className={small ? 'cb-brand small' : 'cb-brand'} aria-label={t('{brand}, на главную', { brand: site.brand_name })}>
      <BrandMark name={site.brand_name} logo={site.logo_url} className="cb-mark" />
      <span className="cb-brand-name">{site.brand_name.toUpperCase()}</span>
    </Link>
  );
}

function Header() {
  const { me, logout, nextLang, switchLang } = useCabinet();
  const location = useLocation();
  const [open, setOpen] = useState(false);
  const photo = photoUrl(me);
  const [failed, setFailed] = useState<string | null>(null);
  const ref = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (ref.current && !ref.current.contains(event.target as Node)) {
        setOpen(false);
      }
    };
    document.addEventListener('mousedown', onDown);
    return () => document.removeEventListener('mousedown', onDown);
  }, [open]);
  const tab = currentTab(location.pathname);
  const tabs = useTabs();
  const answered = useAnswered();
  return (
    <header className="cb-head" data-part="header">
      <div className="cb-head-in">
        <Brand />
        <nav aria-label={t('Кабинет')} className="cb-tabs">
          {tabs.map((t) => (
            <Link key={t.key} to={t.to} className="cb-tab hov" aria-current={tab === t.key ? 'page' : undefined}>
              <TabLabel tab={t} answered={answered} />
            </Link>
          ))}
        </nav>
        <div className="cb-head-end">
          {me.balance_enabled !== false ? (
            <Link to="/cabinet/topup" className="cb-balance hov" aria-label={t('Баланс {amount}, пополнить', { amount: money(me.balance) })}>
              <span>{money(me.balance)}</span>
              <span className="cb-accent">{t('Пополнить')}</span>
            </Link>
          ) : me.balance.minor > 0 ? (
            <Link to="/cabinet/pay" className="cb-balance hov" aria-label={t('Остаток на балансе {amount}', { amount: money(me.balance) })}>
              <span>{money(me.balance)}</span>
            </Link>
          ) : null}
          <ThemeButton className="cb-icon hov" />
          <div className="cb-prof" ref={ref}>
            <button type="button" className="cb-ava" aria-label={t('Профиль')} aria-expanded={open} onClick={() => setOpen((v) => !v)} style={{ background: avatarColor(me.client.id) }}>
              <Icon name="user" size={20} />
              {photo && failed !== photo ? <img className="cb-ava-img" src={photo} alt="" decoding="async" onError={() => setFailed(photo)} /> : null}
            </button>
            {open ? (
              <div className="cb-pop" role="menu">
                <Link to="/cabinet/payments" role="menuitem" className="cb-pop-item plain hov" onClick={() => setOpen(false)}>
                  <Icon name="receipt" size={18} />
                  {t('История оплат')}
                </Link>
                <Link to="/cabinet/security" role="menuitem" className="cb-pop-item plain hov" onClick={() => setOpen(false)}>
                  <Icon name="sliders" size={18} />
                  {t('Безопасность')}
                </Link>
                {nextLang ? (
                  <button type="button" role="menuitem" className="cb-pop-item plain hov" lang={nextLang} onClick={() => switchLang(nextLang)}>
                    <Icon name="globe" size={18} />
                    {NATIVE[nextLang]}
                  </button>
                ) : null}
                <button type="button" role="menuitem" className="cb-pop-item hov" onClick={() => void logout()}>
                  <Icon name="logout" size={18} />
                  {t('Выйти')}
                </button>
              </div>
            ) : null}
          </div>
        </div>
      </div>
    </header>
  );
}

function PhoneHeader() {
  const { me, logout, nextLang, switchLang } = useCabinet();
  const location = useLocation();
  const [open, setOpen] = useState(false);
  useEffect(() => {
    setOpen(false);
  }, [location.pathname, location.hash]);
  const tab = currentTab(location.pathname);
  const tabs = useTabs();
  const answered = useAnswered();
  return (
    <header className="cb-phead" data-part="header">
      <Brand small />
      <span className="cb-grow" />
      {me.balance_enabled !== false || me.balance.minor > 0 ? (
        <Link
          to={me.balance_enabled !== false ? '/cabinet/topup' : '/cabinet/pay'}
          className="cb-chip"
          aria-label={me.balance_enabled !== false ? t('Баланс {amount}, пополнить', { amount: money(me.balance) }) : t('Остаток на балансе {amount}', { amount: money(me.balance) })}
        >
          <span>{money(me.balance)}</span>
        </Link>
      ) : null}
      <ThemeButton className="cb-circ" />
      <button type="button" className="cb-circ cb-menu-btn" aria-label={answered > 0 ? t('Меню, есть ответ поддержки') : t('Меню')} aria-expanded={open} onClick={() => setOpen((v) => !v)}>
        <Icon name={open ? 'close' : 'menu'} size={20} />
        {answered > 0 && !open ? <span className="cb-tabdot on-btn" aria-hidden="true" /> : null}
      </button>
      {open ? (
        <nav aria-label={t('Разделы кабинета')} className="cb-menu">
          {tabs.map((t) => (
            <Link
              key={t.key}
              to={t.to}
              className="cb-menu-item"
              aria-current={tab === t.key ? 'page' : undefined}
              onClick={() => setOpen(false)}
            >
              <TabLabel tab={t} answered={answered} />
            </Link>
          ))}
          <Link to="/cabinet/payments" className="cb-menu-item" aria-current={location.pathname.startsWith('/cabinet/payments') ? 'page' : undefined}>
            {t('История оплат')}
          </Link>
          <Link to="/cabinet/security" className="cb-menu-item" aria-current={location.pathname.startsWith('/cabinet/security') ? 'page' : undefined}>
            {t('Безопасность')}
          </Link>
          {nextLang ? (
            <button type="button" className="cb-menu-item" lang={nextLang} onClick={() => switchLang(nextLang)}>
              {NATIVE[nextLang]}
            </button>
          ) : null}
          <button type="button" className="cb-menu-item" onClick={() => void logout()}>
            {t('Выйти')}
          </button>
        </nav>
      ) : null}
    </header>
  );
}

function Footer() {
  const { site } = useCabinet();
  const link = supportLink(site);
  return (
    <footer className="cb-foot" data-part="footer">
      {supportTickets(site) ? (
        <Link to="/cabinet/help">{t('Помощь')}</Link>
      ) : link ? (
        <a href={link} target="_blank" rel="noreferrer">
          {t('Помощь')}
        </a>
      ) : site.bot_username ? (
        <a href={`https://t.me/${site.bot_username}`} target="_blank" rel="noreferrer">
          {t('Помощь')}
        </a>
      ) : null}
      {(site.documents ?? []).map((doc) => (
        <a key={doc.url} href={doc.url} target="_blank" rel="noreferrer noopener">
          {documentTitle(doc)}
        </a>
      ))}
    </footer>
  );
}

export function Card({ id, part, className, children, label }: { id?: string; part?: Part; className?: string; children: ReactNode; label?: string }) {
  return (
    <section id={id} data-part={part} aria-label={label} className={className ? `cb-card ${className}` : 'cb-card'}>
      {children}
    </section>
  );
}
