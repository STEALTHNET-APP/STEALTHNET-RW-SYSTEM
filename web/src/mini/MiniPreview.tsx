import { useEffect, useMemo, useState, type MouseEvent as ReactMouseEvent } from 'react';
import { api } from '../lib/api';
import { applyLang, type Lang } from '../lib/i18n';
import type { Money } from '../lib/format';
import { DEFAULT_SITE, useSite } from '../lib/site';
import { usePreviewStyles } from '../lib/ownStyles';
import { useSurface } from '../lib/theme';
import type { Currency, Site } from '../lib/types';
import type { ClientMe, SubscriptionView, TrafficReset } from '../cabinet/model';
import { isPart, MINI_PREVIEW, normalizeConfig, type Audience, type MiniDraft } from './config';
import { MiniContext, MiniNav, type MiniCtx } from './MiniApp';
import { MiniFriends } from './MiniFriends';
import { MiniHome } from './MiniHome';
import { MiniProfile } from './MiniProfile';
import type { Extras } from './sections';

type DemoTariff = { id: number; category_id?: number; name: string; devices: { included: number }; traffic: { limit_bytes: number | null; reset: TrafficReset } };

type DemoCatalog = { currency: Currency; tariffs: Array<{ tariff: DemoTariff }> };

const DAY = 86_400_000;
const PATHS = { home: '/app', profile: '/app/profile', friends: '/app/friends' } as const;

function demoView(state: Audience, tariff: DemoTariff | null): SubscriptionView | null {
  if (state === 'new') {
    return null;
  }
  const trial = state === 'trial';
  const ended = state === 'ended';
  const left = trial ? 2 : ended ? 0 : 87;
  const total = trial ? 3 : 90;
  const now = Date.now();
  const expire = new Date(now + (ended ? -6 : left) * DAY).toISOString();
  const limit = trial ? null : (tariff?.traffic.limit_bytes ?? null);
  const devices = tariff?.devices.included ?? 3;
  return {
    subscription: {
      id: 1,
      public_id: 'preview',
      client_id: 1,
      tariff_id: trial ? null : (tariff?.id ?? null),
      kind: trial ? 'trial' : 'paid',
      remna: null,
      extra_devices: 0,
      period_started_at: new Date(now - DAY).toISOString(),
      period_days: total,
      autorenew: { enabled: false, period_id: null },
      mirror: {
        status: ended ? 'expired' : 'active',
        expire_at: expire,
        traffic_used_bytes: limit ? Math.round(limit * 0.28) : 0,
        traffic_limit_bytes: limit,
        hwid_limit: trial ? 1 : devices,
        devices_count: trial ? 1 : Math.min(2, devices),
      } as SubscriptionView['subscription']['mirror'],
      created_at: new Date(now - DAY).toISOString(),
      ended_at: null,
    },
    status: ended ? 'expired' : trial ? 'trial' : 'active',
    progress: { days_left: left, days_total: total },
    subscription_url: null,
    countries: [],
    tariff: trial || !tariff ? null : { id: tariff.id, name: tariff.name, on_sale: true, devices: { included: devices, extra: null }, traffic: tariff.traffic },
  };
}

function demoExtras(draft: MiniDraft, site: Site, currency: Currency): Extras {
  const earned: Money = { minor: currency === 'RUB' ? 34_000 : 420, currency };
  return {
    referral: draft.off.includes('friends')
      ? null
      : {
          code: 'demo',
          bot_link: site.bot_username ? `https://t.me/${site.bot_username.replace(/^@/, '')}?start=demo` : null,
          invited: 2,
          earned,
          balance: { minor: 0, currency },
          levels: [draft.percent],
          withdrawals_enabled: false,
        },
    gifts: !draft.off.includes('gifts'),
    spins: draft.off.includes('roulette') ? null : 1,
    trial: !draft.off.includes('trial'),
  };
}

function demoMe(draft: MiniDraft, currency: Currency): ClientMe {
  const money = (minor: number): Money => ({ minor, currency });
  return {
    client: { id: 1, status: 'active', created_at: new Date().toISOString(), locale: draft.lang },
    locale: draft.lang,
    balance: money(draft.state === 'new' || draft.off.includes('topup') ? 0 : currency === 'RUB' ? 45_000 : 500),
    balance_enabled: !draft.off.includes('topup'),
    promo_codes: !draft.off.includes('promo'),
    options: !draft.off.includes('addons'),
    referral_balance: money(0),
    currency,
    channel: 'miniapp',
    full_access: true,
    two_factor: { enabled: false, enabled_at: null },
    payments_count: draft.state === 'new' ? 0 : 7,
    email: 'client@example.com',
    email_change: true,
    has_password: true,
  };
}

export function MiniPreview() {
  const loaded = useSite();
  const site = loaded ?? DEFAULT_SITE;
  const [draft, setDraft] = useState<MiniDraft | null>(null);
  const [focus, setFocus] = useState<{ part: string; at: number } | null>(null);
  const [catalog, setCatalog] = useState<DemoCatalog | null>(null);
  const [applied, setApplied] = useState<Lang | null>(null);
  const wanted = draft?.lang ?? null;
  usePreviewStyles(typeof draft?.css === 'string' ? draft.css : '');
  useSurface('mini', draft?.theme ?? 'light', site.appearance.client === 'mono' ? 'b' : 'a');

  useEffect(() => {
    api<DemoCatalog>('/api/public/catalog')
      .then(setCatalog)
      .catch(() => setCatalog(null));
  }, []);

  useEffect(() => {
    const origin = window.location.origin;
    const onMessage = (event: MessageEvent) => {
      if (event.origin !== origin || event.source !== window.parent) {
        return;
      }
      const data = event.data as MiniDraft | null;
      if (!data || data.type !== MINI_PREVIEW.draft) {
        return;
      }
      setDraft(data);
      if (data.focus) {
        setFocus({ part: data.focus, at: Date.now() });
      }
    };
    window.addEventListener('message', onMessage);
    if (window.parent !== window) {
      window.parent.postMessage({ type: MINI_PREVIEW.ready }, origin);
    }
    return () => window.removeEventListener('message', onMessage);
  }, []);

  useEffect(() => {
    if (!wanted) {
      return;
    }
    let alive = true;
    const done = () => {
      if (alive) {
        setApplied(wanted);
      }
    };
    applyLang(wanted).then(done, done);
    return () => {
      alive = false;
    };
  }, [wanted]);

  const screen = draft?.screen ?? 'home';
  useEffect(() => {
    window.scrollTo({ top: 0 });
  }, [screen]);

  useEffect(() => {
    if (!focus) {
      return;
    }
    const found = [...document.querySelectorAll<HTMLElement>(`[data-part="${focus.part}"]`)];
    const [first] = found;
    if (!first) {
      return;
    }
    if (focus.part !== 'tabs') {
      const box = first.getBoundingClientRect();
      const room = window.innerHeight - 110;
      if (box.top < 0 || box.bottom > room) {
        window.scrollTo({ top: Math.max(0, box.top + window.scrollY - 24), behavior: 'smooth' });
      }
    }
    for (const node of found) {
      node.classList.add('mn-picked');
    }
    const timer = window.setTimeout(() => {
      for (const node of found) {
        node.classList.remove('mn-picked');
      }
    }, 1400);
    return () => {
      window.clearTimeout(timer);
      for (const node of found) {
        node.classList.remove('mn-picked');
      }
    };
  }, [focus]);

  const currency = catalog?.currency ?? site.currency;
  const ctx = useMemo<MiniCtx | null>(() => {
    if (!draft) {
      return null;
    }
    const view = demoView(draft.state, catalog?.tariffs[0]?.tariff ?? null);
    const nothing = () => undefined;
    const config = normalizeConfig(draft.config);
    return {
      me: demoMe(draft, currency),
      site,
      subs: view ? [view] : [],
      view,
      devices: null,
      ready: true,
      choose: nothing,
      setView: nothing,
      setDevices: nothing,
      reload: async () => undefined,
      theme: draft.theme,
      flipTheme: site.appearance.client_theme === 'client' ? nothing : null,
      lang: draft.lang,
      nextLang: null,
      switchLang: nothing,
      config: { ...config, tabs: config.tabs.filter((tab) => !draft.off.includes(tab.std)) },
      name: draft.lang === 'en' ? 'Alex' : 'Алексей',
      username: 'client',
      unread: 0,
      setUnread: nothing,
      demo: demoExtras(draft, site, currency),
    };
  }, [draft, site, catalog, currency]);

  if (!loaded || !ctx || applied === null) {
    return <div className="mn-blank" aria-busy="true" />;
  }

  const onClick = (event: ReactMouseEvent<HTMLDivElement>) => {
    event.preventDefault();
    event.stopPropagation();
    const part = (event.target as HTMLElement).closest('[data-part]')?.getAttribute('data-part');
    if (isPart(part) && window.parent !== window) {
      window.parent.postMessage({ type: MINI_PREVIEW.pick, part }, window.location.origin);
    }
  };

  return (
    <MiniContext.Provider value={ctx}>
      <div key={ctx.lang} className="mn mn-preview" onClickCapture={onClick}>
        {screen === 'profile' ? <MiniProfile /> : screen === 'friends' ? <MiniFriends /> : <MiniHome />}
        <MiniNav path={PATHS[screen]} />
      </div>
    </MiniContext.Provider>
  );
}
