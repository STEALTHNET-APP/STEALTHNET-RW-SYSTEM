import type { Lang } from '../lib/i18n';
import type { IconName } from '../lib/icons';

export type Phrase = { ru?: string | null; en?: string | null };

export type Audience = 'new' | 'trial' | 'active' | 'ended';

export type SectionKey =
  | 'home'
  | 'plans'
  | 'connect'
  | 'profile'
  | 'friends'
  | 'roulette'
  | 'addons'
  | 'gifts'
  | 'promo'
  | 'payments'
  | 'guide'
  | 'support'
  | 'topup'
  | 'trial'
  | 'renew'
  | 'devices';

export type Glyph = 'send' | 'globe' | 'chat' | 'mail' | 'book' | 'file' | 'gift' | 'percent' | 'trophy' | 'zap' | 'flag' | 'phone';

export type ImageKind = 'png' | 'jpeg' | 'webp';

export type MiniImage = { tag: string; kind?: ImageKind; width?: number; height?: number; url?: string };

export type Banner = {
  on?: boolean;
  image?: MiniImage | null;
  image_en?: MiniImage | null;
  title?: Phrase;
  text?: Phrase;
  to?: string | null;
  for?: Audience[];
};

export type Item = { std?: SectionKey; on?: boolean; label?: Phrase; to?: string; icon?: Glyph; for?: Audience[] };

export type Empty = { title?: Phrase; line?: Phrase; button?: Phrase };

export type BannersBlock = { kind: 'banners'; on?: boolean; items?: Banner[] };
export type GreetingBlock = { kind: 'greeting'; on?: boolean; title?: Phrase; text?: Phrase; for?: Audience[] };
export type SubscriptionBlock = { kind: 'subscription'; look?: 'ring' | 'compact'; empty?: Empty };
export type ActionsBlock = { kind: 'actions'; on?: boolean; items?: Item[] };
export type SectionsBlock = { kind: 'sections'; on?: boolean; items?: Item[] };
export type HomeBlock = BannersBlock | GreetingBlock | SubscriptionBlock | ActionsBlock | SectionsBlock;
export type HomeKind = HomeBlock['kind'];

export type PersonBlock = { kind: 'person'; on?: boolean };
export type BalanceBlock = { kind: 'balance'; on?: boolean };
export type FriendsBlock = { kind: 'friends'; on?: boolean; place?: 'card' | 'screen'; title?: Phrase; text?: Phrase };
export type HelpBlock = { kind: 'help'; gifts?: boolean };
export type LinksBlock = { kind: 'links'; on?: boolean; items?: Item[] };
export type DocumentsBlock = { kind: 'documents' };
export type AccountBlock = { kind: 'account' };
export type ProfileBlock = PersonBlock | BalanceBlock | FriendsBlock | HelpBlock | LinksBlock | DocumentsBlock | AccountBlock;
export type ProfileKind = ProfileBlock['kind'];

export type Tab = { std: SectionKey; label?: Phrase };

export type Header = { balance: boolean; avatar: boolean };

export type MiniConfig = { header: Header; home: HomeBlock[]; tabs: Tab[]; profile: ProfileBlock[] };

export const HOME_KINDS: HomeKind[] = ['banners', 'greeting', 'subscription', 'actions', 'sections'];
export const PROFILE_KINDS: ProfileKind[] = ['person', 'balance', 'friends', 'help', 'links', 'documents', 'account'];

export const ROW_KEYS: SectionKey[] = ['friends', 'roulette', 'addons', 'gifts', 'promo', 'payments', 'guide', 'support'];
export const ACTION_KEYS: SectionKey[] = ['renew', 'devices', 'addons', 'topup', 'friends', 'gifts', 'promo', 'roulette', 'payments', 'guide', 'support'];
export const TAB_KEYS: SectionKey[] = ['home', 'plans', 'connect', 'profile', 'friends', 'support', 'roulette', 'gifts', 'promo', 'payments', 'topup'];
export const TARGET_KEYS: SectionKey[] = ['plans', 'connect', 'trial', 'topup', 'addons', 'friends', 'gifts', 'promo', 'roulette', 'payments', 'support', 'profile'];
export const ALWAYS_TABS: SectionKey[] = ['home', 'plans', 'profile'];
export const STANDARD_ACTIONS: SectionKey[] = ['renew', 'devices'];
export const STANDARD_TABS: SectionKey[] = ['home', 'plans', 'connect', 'profile'];
export const AUDIENCES: Audience[] = ['new', 'trial', 'active', 'ended'];
export const GLYPHS: Glyph[] = ['send', 'globe', 'chat', 'mail', 'book', 'file', 'gift', 'percent', 'trophy', 'zap', 'flag', 'phone'];
export const TABS_MIN = 3;
export const TABS_MAX = 5;

export const SECTION_ICONS: Record<SectionKey, IconName> = {
  home: 'home',
  plans: 'tag',
  connect: 'zap',
  profile: 'user',
  friends: 'share',
  roulette: 'trophy',
  addons: 'plus',
  gifts: 'gift',
  promo: 'percent',
  payments: 'receipt',
  guide: 'book',
  support: 'chat',
  topup: 'wallet',
  trial: 'gift',
  renew: 'refresh',
  devices: 'phone',
};

const PATHS: Record<SectionKey, string> = {
  home: '/app',
  plans: '/app/plans',
  connect: '/app/connect',
  profile: '/app/profile',
  friends: '/app/friends',
  roulette: '/app/roulette',
  addons: '/app/addons',
  gifts: '/app/gifts',
  promo: '/app/promo',
  payments: '/app/payments',
  guide: '/app/connect',
  support: '/app/support',
  topup: '/app/topup',
  trial: '/app/trial',
  renew: '/app/plans',
  devices: '/app/connect',
};

const HOME_PATHS = ['/app', '/app/', '/app/topup', '/app/payments', '/app/roulette', '/app/promo'];

const SECTIONS = new Set<string>(Object.keys(PATHS));

export function isSection(value: unknown): value is SectionKey {
  return typeof value === 'string' && SECTIONS.has(value);
}

export function sectionPath(key: SectionKey): string {
  return PATHS[key];
}

export function standardHome(kind: HomeKind): HomeBlock {
  switch (kind) {
    case 'banners':
      return { kind, items: [] };
    case 'greeting':
      return { kind, on: false };
    case 'actions':
      return { kind, items: STANDARD_ACTIONS.map((std) => ({ std })) };
    case 'sections':
      return { kind, items: ROW_KEYS.map((std) => ({ std })) };
    default:
      return { kind };
  }
}

export function standardProfile(kind: ProfileKind): ProfileBlock {
  switch (kind) {
    case 'person':
      return { kind, on: false };
    case 'links':
      return { kind, items: [] };
    default:
      return { kind };
  }
}

export function standardTabs(): Tab[] {
  return STANDARD_TABS.map((std) => ({ std }));
}

export function standardConfig(): MiniConfig {
  return {
    header: { balance: true, avatar: false },
    home: HOME_KINDS.map(standardHome),
    tabs: standardTabs(),
    profile: PROFILE_KINDS.map(standardProfile),
  };
}

function record(value: unknown): Record<string, unknown> | null {
  return value && typeof value === 'object' && !Array.isArray(value) ? (value as Record<string, unknown>) : null;
}

function blocksOf<K extends string, B extends { kind: K }>(raw: unknown, kinds: K[], standard: (kind: K) => B): B[] {
  const out: B[] = [];
  for (const entry of Array.isArray(raw) ? raw : []) {
    const kind = record(entry)?.kind;
    if (typeof kind === 'string' && (kinds as string[]).includes(kind) && !out.some((block) => block.kind === kind)) {
      out.push(entry as B);
    }
  }
  kinds.forEach((kind, place) => {
    if (out.some((block) => block.kind === kind)) {
      return;
    }
    let at = 0;
    for (let i = place - 1; i >= 0; i -= 1) {
      const found = out.findIndex((block) => block.kind === kinds[i]);
      if (found >= 0) {
        at = found + 1;
        break;
      }
    }
    out.splice(at, 0, standard(kind));
  });
  return out;
}

function itemsOf(raw: unknown, sections: SectionKey[]): Item[] {
  const out: Item[] = [];
  for (const entry of Array.isArray(raw) ? raw : []) {
    const item = record(entry) as Item | null;
    if (item && (item.std === undefined || (sections.includes(item.std) && !out.some((other) => other.std === item.std)))) {
      out.push(item);
    }
  }
  return out;
}

export function arrangeRows(raw: unknown): Item[] {
  const out = itemsOf(raw, ROW_KEYS);
  ROW_KEYS.forEach((key, place) => {
    if (out.some((item) => item.std === key)) {
      return;
    }
    let at = 0;
    for (let i = place - 1; i >= 0; i -= 1) {
      const found = out.findIndex((item) => item.std === ROW_KEYS[i]);
      if (found >= 0) {
        at = found + 1;
        break;
      }
    }
    out.splice(at, 0, { std: key });
  });
  return out;
}

function tabsOf(raw: unknown): Tab[] {
  const out: Tab[] = [];
  for (const entry of Array.isArray(raw) ? raw : []) {
    const std = record(entry)?.std;
    if (isSection(std) && TAB_KEYS.includes(std) && !out.some((tab) => tab.std === std)) {
      out.push(entry as Tab);
    }
  }
  const complete = ALWAYS_TABS.every((key) => out.some((tab) => tab.std === key));
  return complete && out.length <= TABS_MAX ? out : standardTabs();
}

export function normalizeConfig(raw: unknown): MiniConfig {
  const source = record(raw);
  if (!source) {
    return standardConfig();
  }
  const home = blocksOf<HomeKind, HomeBlock>(source.home, HOME_KINDS, standardHome).map((block): HomeBlock => {
    switch (block.kind) {
      case 'banners':
        return { ...block, items: Array.isArray(block.items) ? block.items.filter((item) => !!record(item)) : [] };
      case 'actions':
        return { ...block, items: block.items === undefined || block.items === null ? STANDARD_ACTIONS.map((std) => ({ std })) : itemsOf(block.items, ACTION_KEYS) };
      case 'sections':
        return { ...block, items: arrangeRows(block.items) };
      default:
        return block;
    }
  });
  const profile = blocksOf<ProfileKind, ProfileBlock>(source.profile, PROFILE_KINDS, standardProfile).map(
    (block): ProfileBlock => (block.kind === 'links' ? { ...block, items: itemsOf(block.items, []) } : block),
  );
  return {
    header: { balance: record(source.header)?.balance !== false, avatar: record(source.header)?.avatar === true },
    home,
    tabs: tabsOf(source.tabs),
    profile,
  };
}

export function homeBlock<K extends HomeKind>(config: MiniConfig, kind: K): Extract<HomeBlock, { kind: K }> {
  return (config.home.find((block) => block.kind === kind) ?? standardHome(kind)) as Extract<HomeBlock, { kind: K }>;
}

export function profileBlock<K extends ProfileKind>(config: MiniConfig, kind: K): Extract<ProfileBlock, { kind: K }> {
  return (config.profile.find((block) => block.kind === kind) ?? standardProfile(kind)) as Extract<ProfileBlock, { kind: K }>;
}

export function homeOn(block: HomeBlock): boolean {
  switch (block.kind) {
    case 'subscription':
      return true;
    case 'greeting':
      return block.on === true;
    default:
      return block.on !== false;
  }
}

export function profileOn(block: ProfileBlock): boolean {
  switch (block.kind) {
    case 'person':
      return block.on === true;
    case 'balance':
    case 'friends':
    case 'links':
      return block.on !== false;
    default:
      return true;
  }
}

export function friendsPath(config: MiniConfig): string {
  const block = profileBlock(config, 'friends');
  return block.on !== false && block.place !== 'screen' ? PATHS.profile : PATHS.friends;
}

export function say(phrase: Phrase | null | undefined, lang: Lang): string {
  const ru = phrase?.ru ?? '';
  return lang === 'en' ? phrase?.en || ru : ru;
}

export function pick(phrase: Phrase | null | undefined, lang: Lang, standard: string): string {
  const own = lang === 'en' ? phrase?.en : phrase?.ru;
  return own === null || own === undefined ? standard : own;
}

export function withName(text: string, name: string | null): string {
  if (name) {
    return text.split('{name}').join(name);
  }
  if (!text.includes('{name}')) {
    return text;
  }
  const opened = /^\s*\{name\}/.test(text);
  const bare = text
    .replace(/[\s,]*\{name\}/g, '')
    .replace(/^[\s,.!?—–-]+/, '')
    .trim();
  return opened ? bare.charAt(0).toUpperCase() + bare.slice(1) : bare;
}

export function withPercent(text: string, percent: number): string {
  return text.split('{percent}').join(String(percent));
}

export type AudienceSub = { status: string; subscription: { kind: string } };

export function audienceOf(subs: AudienceSub[]): Audience {
  if (subs.length === 0) {
    return 'new';
  }
  const alive = subs.filter((view) => view.status !== 'expired');
  if (alive.some((view) => view.subscription.kind !== 'trial')) {
    return 'active';
  }
  return alive.length > 0 ? 'trial' : 'ended';
}

export function shownTo(list: Audience[] | null | undefined, state: Audience): boolean {
  return !list || list.length === 0 || list.includes(state);
}

export function imageOf(banner: Banner, lang: Lang): MiniImage | null {
  return (lang === 'en' ? banner.image_en : null) ?? banner.image ?? null;
}

export function imageUrl(image: MiniImage | null | undefined): string | null {
  if (!image?.tag) {
    return null;
  }
  if (image.url) {
    return image.url;
  }
  return `/api/public/miniapp/${image.tag}.${image.kind === 'jpeg' ? 'jpg' : (image.kind ?? 'png')}`;
}

export function linkUrl(to: string | null | undefined): string | null {
  return to && /^https:\/\/[^\s"'<>`\\{}|^]+$/i.test(to) ? to : null;
}

export function linkIcon(item: Item): IconName {
  return item.icon ?? (item.to && isSection(item.to) ? SECTION_ICONS[item.to] : /^https:\/\/t\.me\//i.test(item.to ?? '') ? 'send' : 'globe');
}

export function currentTab(tabs: Tab[], pathname: string): SectionKey | null {
  const own = tabs.find((tab) => tab.std !== 'home' && pathname.startsWith(PATHS[tab.std]));
  if (own) {
    return own.std;
  }
  if (pathname.startsWith(PATHS.friends)) {
    return 'profile';
  }
  return HOME_PATHS.includes(pathname) ? 'home' : null;
}

export const MINI_PREVIEW = {
  draft: 'sn-mini-draft',
  ready: 'sn-mini-ready',
  pick: 'sn-mini-pick',
} as const;

export type PreviewScreen = 'home' | 'profile' | 'friends';

export type Part = 'header' | 'tabs' | HomeKind | ProfileKind;

export const PARTS: Part[] = ['header', 'tabs', ...HOME_KINDS, ...PROFILE_KINDS];

export function isPart(value: unknown): value is Part {
  return typeof value === 'string' && (PARTS as string[]).includes(value);
}

export type MiniDraft = {
  type: typeof MINI_PREVIEW.draft;
  config: unknown;
  css: string;
  theme: 'dark' | 'light';
  lang: Lang;
  state: Audience;
  screen: PreviewScreen;
  off: SectionKey[];
  percent: number;
  focus: Part | null;
};
