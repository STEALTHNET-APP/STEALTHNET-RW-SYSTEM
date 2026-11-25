import { lazy, Suspense, useCallback, useEffect, useLayoutEffect, useMemo, useState } from 'react';
import { Navigate, Route, Routes } from 'react-router';
import { api, ApiError } from '../lib/api';
import { applyLang } from '../lib/i18n';
import { usePhone } from '../lib/media';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { readStored, useSurface, useSystemTheme, writeStored, type Theme } from '../lib/theme';
import type { AdminTheme, Appearance, Me, Permission } from '../lib/types';
import { Login } from './Login';
import { Pending } from './pages/Pending';
import { ForcedPassword, Security } from './pages/Security';
import { firstSection } from './pages/settingsAccess';
import { SessionContext, type Session } from './session';
import { Shell } from './Shell';
const Clients = lazy(() => import('./clients/Clients').then((m) => ({ default: m.Clients })));
const ClientPage = lazy(() => import('./client/Client').then((m) => ({ default: m.ClientPage })));
const Tariffs = lazy(() => import('./tariffs/Tariffs').then((m) => ({ default: m.Tariffs })));
const Home = lazy(() => import('./home/Home').then((m) => ({ default: m.Home })));
const Support = lazy(() => import('./support/Support').then((m) => ({ default: m.Support })));
const InfraNodes = lazy(() => import('./infra/InfraNodes').then((m) => ({ default: m.InfraNodes })));
const InfraSquads = lazy(() => import('./infra/InfraSquads').then((m) => ({ default: m.InfraSquads })));
const InfraHosts = lazy(() => import('./infra/InfraHosts').then((m) => ({ default: m.InfraHosts })));
const InfraBilling = lazy(() => import('./infra/InfraBilling').then((m) => ({ default: m.InfraBilling })));
const Audit = lazy(() => import('./audit/Audit').then((m) => ({ default: m.Audit })));
const ExtInstall = lazy(() => import('./modules/ExtInstall').then((m) => ({ default: m.ExtInstall })));
const ExtModule = lazy(() => import('./modules/ExtModule').then((m) => ({ default: m.ExtModule })));
const ExtPage = lazy(() => import('./modules/ExtPage').then((m) => ({ default: m.ExtPage })));
const Modules = lazy(() => import('./modules/Modules').then((m) => ({ default: m.Modules })));
const ModulePage = lazy(() => import('./modules/ModulePage').then((m) => ({ default: m.ModulePage })));
const ModuleScreen = lazy(() => import('./modules/ModuleScreen').then((m) => ({ default: m.ModuleScreen })));
const Antibot = lazy(() => import('./protect/Antibot').then((m) => ({ default: m.Antibot })));
const Limits = lazy(() => import('./protect/Limits').then((m) => ({ default: m.Limits })));
const Money = lazy(() => import('./money/Money').then((m) => ({ default: m.Money })));
const SettingsGate = lazy(() => import('./pages/SettingsLayout').then((m) => ({ default: m.SettingsGate })));
const SettingsLayout = lazy(() => import('./pages/SettingsLayout').then((m) => ({ default: m.SettingsLayout })));
const SettingsLook = lazy(() => import('./pages/SettingsLook').then((m) => ({ default: m.SettingsLook })));
const SettingsRemna = lazy(() => import('./pages/SettingsRemna').then((m) => ({ default: m.SettingsRemna })));
const SettingsRemnaSync = lazy(() => import('./pages/SettingsRemnaSync').then((m) => ({ default: m.SettingsRemnaSync })));
const SettingsPayments = lazy(() => import('./pages/SettingsPayments').then((m) => ({ default: m.SettingsPayments })));
const SettingsGeneral = lazy(() => import('./pages/SettingsGeneral').then((m) => ({ default: m.SettingsGeneral })));
const SettingsBot = lazy(() => import('./pages/SettingsBot').then((m) => ({ default: m.SettingsBot })));
const SettingsBotImages = lazy(() => import('./pages/SettingsBotImages').then((m) => ({ default: m.SettingsBotImages })));
const SettingsBotTexts = lazy(() => import('./pages/SettingsBotTexts').then((m) => ({ default: m.SettingsBotTexts })));
const SettingsBotLayout = lazy(() => import('./botlayout/BotLayout').then((m) => ({ default: m.SettingsBotLayout })));
const SettingsDocuments = lazy(() => import('./pages/SettingsDocuments').then((m) => ({ default: m.SettingsDocuments })));
const SettingsDocument = lazy(() => import('./pages/SettingsDocument').then((m) => ({ default: m.SettingsDocument })));
const SettingsLanding = lazy(() => import('./pages/SettingsLanding').then((m) => ({ default: m.SettingsLanding })));
const SettingsCabinet = lazy(() => import('./cabinet/SettingsCabinet').then((m) => ({ default: m.SettingsCabinet })));
const SettingsMiniApp = lazy(() => import('./miniapp/MiniAppEditor').then((m) => ({ default: m.SettingsMiniApp })));
const SettingsAdmins = lazy(() => import('./pages/SettingsAdmins').then((m) => ({ default: m.SettingsAdmins })));
const SettingsApiKeys = lazy(() => import('./pages/SettingsApiKeys').then((m) => ({ default: m.SettingsApiKeys })));
const SettingsAlerts = lazy(() => import('./pages/SettingsAlerts').then((m) => ({ default: m.SettingsAlerts })));
const SettingsMail = lazy(() => import('./pages/SettingsMail').then((m) => ({ default: m.SettingsMail })));
const SettingsMailLetters = lazy(() => import('./pages/SettingsMailLetters').then((m) => ({ default: m.SettingsMailLetters })));
const SettingsClientAccess = lazy(() => import('./pages/SettingsClientAccess').then((m) => ({ default: m.SettingsClientAccess })));
const SettingsLanguages = lazy(() => import('./pages/SettingsLanguages').then((m) => ({ default: m.SettingsLanguages })));
const SettingsUpdates = lazy(() => import('./pages/SettingsUpdates').then((m) => ({ default: m.SettingsUpdates })));
const SettingsBackups = lazy(() => import('./pages/SettingsBackups').then((m) => ({ default: m.SettingsBackups })));
const SettingsImport = lazy(() => import('./pages/SettingsImport').then((m) => ({ default: m.SettingsImport })));
const BugReport = lazy(() => import('./report/BugReport').then((m) => ({ default: m.BugReport })));
import type { BuildSummary } from './version';

const THEME_KEY = 'sn.admin.theme';

type State = { kind: 'loading' } | { kind: 'guest' } | { kind: 'in'; me: Me } | { kind: 'error'; message: string };

function resolve(choice: AdminTheme, system: Theme): Theme {
  return choice === 'system' ? system : choice;
}

function storedTheme(): Theme | null {
  const value = readStored(THEME_KEY);
  return value === 'dark' || value === 'light' ? value : null;
}

export function AdminApp() {
  useLayoutEffect(() => {
    void applyLang('ru');
  }, []);
  const site = useSite();
  const system = useSystemTheme();
  const [state, setState] = useState<State>({ kind: 'loading' });
  const [personal, setPersonal] = useState<Theme | null>(storedTheme);

  const load = useCallback(async () => {
    try {
      const me = await api<Me>('/api/admin/me');
      setState({ kind: 'in', me });
    } catch (err) {
      if (err instanceof ApiError && err.status === 401) {
        setState({ kind: 'guest' });
      } else {
        setState({ kind: 'error', message: err instanceof ApiError ? err.message : 'Сервер не ответил' });
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const [supportBadge, setSupportBadge] = useState(0);
  const [moneyBadge, setMoneyBadge] = useState(0);
  const [build, setBuild] = useState<BuildSummary | null>(null);
  const [badgeTick, setBadgeTick] = useState(0);
  const signedIn = state.kind === 'in';
  useEffect(() => {
    if (!signedIn) {
      return;
    }
    let alive = true;
    const tick = () =>
      api<{ support: number; money?: number }>('/api/admin/badges')
        .then((b) => {
          if (alive) {
            setSupportBadge(b.support);
            setMoneyBadge(b.money ?? 0);
          }
        })
        .catch(() => undefined);
    const onShow = () => {
      if (document.visibilityState === 'visible') {
        void tick();
      }
    };
    void tick();
    const timer = window.setInterval(onShow, 30_000);
    window.addEventListener('focus', onShow);
    document.addEventListener('visibilitychange', onShow);
    return () => {
      alive = false;
      window.clearInterval(timer);
      window.removeEventListener('focus', onShow);
      document.removeEventListener('visibilitychange', onShow);
    };
  }, [signedIn, badgeTick]);

  const refreshBadges = useCallback(() => setBadgeTick((n) => n + 1), []);

  const canSupport = state.kind === 'in' && state.me.permissions.includes('support');
  const supportWaiting = useCallback(
    (count: number) => {
      if (canSupport) {
        setSupportBadge(count);
      }
    },
    [canSupport],
  );

  const adminId = state.kind === 'in' ? state.me.admin.admin_id : null;
  useEffect(() => {
    if (adminId === null) {
      setBuild(null);
      return;
    }
    let alive = true;
    const tick = () =>
      api<BuildSummary>('/api/admin/system/summary')
        .then((s) => alive && setBuild(s))
        .catch(() => undefined);
    void tick();
    const timer = window.setInterval(tick, 15 * 60_000);
    return () => {
      alive = false;
      window.clearInterval(timer);
    };
  }, [adminId]);

  const appearance: Appearance = state.kind === 'in' ? state.me.appearance : (site ?? DEFAULT_SITE).appearance;
  const canSettings = state.kind === 'in' && state.me.permissions.includes('settings');
  const theme: Theme = !canSettings && personal ? personal : resolve(appearance.admin_theme, system);
  const phone = usePhone();
  useSurface(phone ? 'admin-phone' : 'admin', theme, appearance.admin === 'top' ? 'b' : 'a');

  const setAppearance = useCallback((next: Appearance) => {
    setState((current) => (current.kind === 'in' ? { kind: 'in', me: { ...current.me, appearance: next } } : current));
  }, []);

  const flipTheme = useCallback(async () => {
    const next: Theme = theme === 'dark' ? 'light' : 'dark';
    if (state.kind === 'in' && canSettings) {
      const optimistic = { ...state.me.appearance, admin_theme: next };
      setAppearance(optimistic);
      try {
        const saved = await api<Appearance>('/api/admin/settings/appearance', { method: 'PUT', body: optimistic });
        setAppearance(saved);
      } catch {
        setAppearance(state.me.appearance);
      }
      return;
    }
    writeStored(THEME_KEY, next);
    setPersonal(next);
  }, [theme, state, canSettings, setAppearance]);

  const session = useMemo<Session | null>(() => {
    if (state.kind !== 'in') {
      return null;
    }
    const me = state.me;
    return {
      me,
      can: (permission: Permission) => me.permissions.includes(permission),
      reload: load,
      logout: async () => {
        try {
          await api('/api/admin/auth/logout', { method: 'POST' });
        } finally {
          setState({ kind: 'guest' });
        }
      },
      setAppearance,
      theme,
      flipTheme: () => void flipTheme(),
      supportWaiting,
      refreshBadges,
    };
  }, [state, load, setAppearance, theme, flipTheme, supportWaiting, refreshBadges]);

  if (state.kind === 'loading') {
    return <div style={{ minHeight: '100vh', background: 'var(--bg)' }} aria-busy="true" />;
  }
  if (state.kind === 'error') {
    return (
      <div className="login">
        <div className="login-box card login-card" role="alert">
          <h1 style={{ fontSize: 22, fontWeight: 800 }}>Админка не открылась</h1>
          <p className="muted">{state.message}</p>
          <button type="button" className="btn-main btn" onClick={() => void load()}>
            Повторить
          </button>
        </div>
      </div>
    );
  }
  if (state.kind === 'guest' || !session) {
    return <Login brand={(site ?? DEFAULT_SITE).brand_name} logo={site?.logo_url ?? null} onDone={() => void load()} />;
  }
  if (state.me.must_change_password) {
    return (
      <ForcedPassword brand={state.me.brand_name} logo={state.me.logo_url ?? null} onDone={() => void load()} onLogout={() => void session.logout()} />
    );
  }

  return (
    <SessionContext.Provider value={session}>
      <Shell theme={theme} onFlipTheme={() => void flipTheme()} supportBadge={supportBadge} moneyBadge={moneyBadge} build={build}>
        <Suspense fallback={<div style={{ minHeight: '60vh' }} aria-busy="true" />}>
        <Routes>
          <Route index element={<Home />} />
          <Route path="clients" element={<Clients />} />
          <Route path="clients/antibot" element={<Antibot />} />
          <Route path="clients/limits" element={<Limits />} />
          <Route path="clients/:id" element={<ClientPage />} />
          <Route path="support" element={<Support />} />
          <Route path="money/*" element={<Money />} />
          <Route path="tariffs" element={<Tariffs />} />
          <Route path="infra" element={<InfraNodes />} />
          <Route path="infra/squads" element={<InfraSquads />} />
          <Route path="infra/hosts" element={<InfraHosts />} />
          <Route path="infra/billing" element={<InfraBilling />} />
          <Route path="audit" element={<Audit />} />
          <Route path="audit/:id" element={<Audit />} />
          <Route path="modules" element={<Modules />} />
          <Route path="modules/install" element={<ExtInstall />} />
          <Route path="modules/ext/:id" element={<ExtModule />} />
          <Route path="modules/ext/:id/page" element={<ExtPage />} />
          <Route path="ext/:id" element={<ExtPage />} />
          <Route path="modules/:id" element={<ModulePage />} />
          <Route path="m/:module/*" element={<ModuleScreen />} />
          <Route path="settings" element={<SettingsGate />}>
            <Route index element={<Navigate to={firstSection(session.can)?.to ?? '/admin'} replace />} />
            <Route path="look" element={<SettingsLook />} />
            <Route path="remnawave" element={<SettingsRemna />} />
            <Route path="remnawave/sync" element={<SettingsRemnaSync />} />
            <Route path="payments" element={<SettingsPayments />} />
            <Route path="general" element={<SettingsGeneral />} />
            <Route path="bot" element={<SettingsBot />} />
            <Route path="updates" element={<SettingsUpdates />} />
            <Route path="backups" element={<SettingsBackups />} />
            <Route path="bot/texts" element={<SettingsBotTexts />} />
            <Route path="bot/layout" element={<SettingsBotLayout />} />
            <Route path="bot/images" element={<SettingsBotImages />} />
            <Route path="documents" element={<SettingsDocuments />} />
            <Route path="documents/:id" element={<SettingsDocument />} />
            <Route path="landing" element={<SettingsLanding />} />
            <Route path="cabinet" element={<SettingsCabinet />} />
            <Route path="miniapp" element={<SettingsMiniApp />} />
            <Route path="admins" element={<SettingsAdmins />} />
            <Route path="api-keys" element={<SettingsApiKeys />} />
            <Route path="mail" element={<SettingsMail />} />
            <Route path="mail/letters" element={<SettingsMailLetters />} />
            <Route path="alerts" element={<SettingsAlerts />} />
            <Route path="client" element={<SettingsClientAccess />} />
            <Route path="languages" element={<SettingsLanguages />} />
            <Route path="import" element={<SettingsImport />} />
            <Route
              path="*"
              element={
                <SettingsLayout>
                  <Pending title="Раздел настроек" />
                </SettingsLayout>
              }
            />
          </Route>
          <Route path="security" element={<Security />} />
          <Route path="bug-report" element={<BugReport />} />
          <Route path="*" element={<Pending title="Страница не найдена" text="Такого раздела нет. Выберите раздел в меню." />} />
        </Routes>
        </Suspense>
      </Shell>
    </SessionContext.Provider>
  );
}
