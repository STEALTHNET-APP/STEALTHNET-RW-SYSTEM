import { count } from '../../lib/format';
import type { Lang } from '../../lib/i18n';
import {
  ALWAYS_TABS,
  AUDIENCES,
  isSection,
  normalizeConfig,
  SECTION_ICONS,
  standardConfig,
  standardHome,
  standardProfile,
  type Audience,
  type Banner,
  type HomeBlock,
  type Header,
  type HomeKind,
  type Item,
  type MiniConfig,
  type MiniImage,
  type Part,
  type Phrase,
  type ProfileBlock,
  type ProfileKind,
  type SectionKey,
  type Tab,
} from '../../mini/config';
import type { IconName } from '../../lib/icons';

export type Screen = 'home' | 'tabs' | 'profile';

export type Limits = {
  banners: number;
  banner_title: number;
  banner_text: number;
  greeting_title: number;
  greeting_text: number;
  empty_title: number;
  empty_line: number;
  button: number;
  label: number;
  tab: number;
  friends_title: number;
  friends_text: number;
  url: number;
  actions: number;
  tabs_min: number;
  tabs_max: number;
  links: number;
  page_css: number;
  block_css: number;
};

export const LIMITS: Limits = {
  banners: 5,
  banner_title: 80,
  banner_text: 200,
  greeting_title: 60,
  greeting_text: 300,
  empty_title: 40,
  empty_line: 80,
  button: 24,
  label: 40,
  tab: 12,
  friends_title: 40,
  friends_text: 300,
  url: 500,
  actions: 4,
  tabs_min: 3,
  tabs_max: 5,
  links: 8,
  page_css: 50_000,
  block_css: 10_000,
};

export type ImageRules = { max_bytes: number; min_side: number; max_side: number; types: string[] };

export const IMAGE_RULES: ImageRules = {
  max_bytes: 1024 * 1024,
  min_side: 100,
  max_side: 4096,
  types: ['image/png', 'image/jpeg', 'image/webp'],
};

export type SectionInfo = { key: SectionKey; title: string; off: string | null };

export type AdminView = {
  header?: unknown;
  home?: unknown;
  tabs?: unknown;
  profile?: unknown;
  css?: unknown;
  styles?: unknown;
  limits?: Partial<Limits>;
  image?: ImageRules;
  sections?: SectionInfo[];
  english?: boolean;
  off?: string | null;
  friends_percent?: number;
};

export const SCREENS: Array<{ key: Screen; name: string }> = [
  { key: 'home', name: 'Главная' },
  { key: 'tabs', name: 'Меню внизу' },
  { key: 'profile', name: 'Профиль' },
];

export const HOME_NAMES: Record<HomeKind, string> = {
  banners: 'Баннеры',
  greeting: 'Приветствие',
  subscription: 'Подписка',
  actions: 'Кнопки под подпиской',
  sections: 'Разделы',
};

export const PROFILE_NAMES: Record<ProfileKind, string> = {
  person: 'Фото и имя',
  balance: 'Баланс',
  friends: 'Друзья',
  help: 'Оплаты и поддержка',
  links: 'Свои ссылки',
  documents: 'Документы',
  account: 'Аккаунт',
};

export const AUDIENCE_NAMES: Record<Audience, string> = {
  new: 'Новые',
  trial: 'На пробном',
  active: 'С подпиской',
  ended: 'Закончилась',
};

export const STATE_NAMES: Record<Audience, string> = {
  new: 'Новый',
  trial: 'Пробный',
  active: 'С подпиской',
  ended: 'Закончилась',
};

export const SECTION_TITLES: Record<SectionKey, string> = {
  home: 'Главная',
  plans: 'Тарифы',
  connect: 'Подключение',
  profile: 'Профиль',
  friends: 'Друзья',
  roulette: 'Рулетка',
  addons: 'Докупить устройства или трафик',
  gifts: 'Подарить подписку',
  promo: 'Промокод',
  payments: 'История оплат',
  guide: 'Инструкции',
  support: 'Поддержка',
  topup: 'Пополнение баланса',
  trial: 'Пробный период',
  renew: 'Продлить',
  devices: 'Устройства',
};

export const TAB_NOTES: Partial<Record<SectionKey, string>> = {
  home: 'без неё клиент не увидит подписку',
  plans: 'без неё клиент не оплатит подписку',
  profile: 'без неё клиент не найдёт поддержку и свой аккаунт',
};

export function chars(value: string): number {
  return [...value].length;
}

export function screenOf(part: Part): Screen {
  if (part === 'tabs') {
    return 'tabs';
  }
  return part === 'header' || part in HOME_NAMES ? 'home' : 'profile';
}

export function moved<T>(list: T[], from: number, to: number): T[] {
  if (from < 0 || from >= list.length || from === to) {
    return list;
  }
  const next = [...list];
  const [item] = next.splice(from, 1);
  if (item === undefined) {
    return list;
  }
  next.splice(Math.min(Math.max(to, 0), next.length), 0, item);
  return next;
}

export function dropIndex(from: number, over: number, after: boolean): number {
  const at = after ? over + 1 : over;
  return from < at ? at - 1 : at;
}

export function setPhrase(phrase: Phrase | undefined, lang: Lang, value: string | null): Phrase | undefined {
  const next: Phrase = { ...(phrase ?? {}) };
  if (value === null) {
    delete next[lang];
  } else {
    next[lang] = value;
  }
  return Object.keys(next).length ? next : undefined;
}

function phraseOut(phrase: Phrase | null | undefined, keepEmpty = false): Phrase | undefined {
  const out: Phrase = {};
  for (const lang of ['ru', 'en'] as const) {
    const value = phrase?.[lang];
    if (value === null || value === undefined) {
      continue;
    }
    const text = value.trim();
    if (text || keepEmpty) {
      out[lang] = text;
    }
  }
  return Object.keys(out).length ? out : undefined;
}

function audienceOut(list: Audience[] | null | undefined): Audience[] | undefined {
  const kept = AUDIENCES.filter((key) => (list ?? []).includes(key));
  return kept.length && kept.length < AUDIENCES.length ? kept : undefined;
}

function imageOut(image: MiniImage | null | undefined): MiniImage | undefined {
  return image?.tag ? { tag: image.tag } : undefined;
}

function compact<T extends object>(value: T): T {
  const out: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(value)) {
    if (entry !== undefined && entry !== null) {
      out[key] = entry;
    }
  }
  return out as T;
}

function shown(on: boolean | undefined): false | undefined {
  return on === false ? false : undefined;
}

function bannerOut(banner: Banner): Banner {
  return compact({
    on: shown(banner.on),
    image: imageOut(banner.image),
    image_en: imageOut(banner.image_en),
    title: phraseOut(banner.title),
    text: phraseOut(banner.text),
    to: banner.to?.trim() || undefined,
    for: audienceOut(banner.for),
  });
}

function itemOut(item: Item, hideable: boolean): Item {
  const on = hideable ? shown(item.on) : undefined;
  if (item.std) {
    return compact({ std: item.std, on, label: phraseOut(item.label) });
  }
  return compact({ on, label: phraseOut(item.label), to: item.to?.trim() || undefined, icon: item.icon, for: audienceOut(item.for) });
}

function homeOut(block: HomeBlock): HomeBlock {
  switch (block.kind) {
    case 'banners':
      return compact({ kind: block.kind, on: shown(block.on), items: block.items?.length ? block.items.map(bannerOut) : undefined });
    case 'greeting':
      return compact({ kind: block.kind, on: block.on === true, title: phraseOut(block.title), text: phraseOut(block.text), for: audienceOut(block.for) });
    case 'subscription': {
      const empty = compact({ title: phraseOut(block.empty?.title), line: phraseOut(block.empty?.line, true), button: phraseOut(block.empty?.button) });
      return compact({ kind: block.kind, look: block.look === 'compact' ? block.look : undefined, empty: Object.keys(empty).length ? empty : undefined });
    }
    case 'actions':
      return compact({ kind: block.kind, on: shown(block.on), items: (block.items ?? []).map((item) => itemOut(item, false)) });
    case 'sections':
      return compact({ kind: block.kind, on: shown(block.on), items: (block.items ?? []).map((item) => itemOut(item, true)) });
  }
}

function profileOut(block: ProfileBlock): ProfileBlock {
  switch (block.kind) {
    case 'person':
      return { kind: block.kind, on: block.on === true };
    case 'help':
      return compact({ kind: block.kind, gifts: block.gifts === true ? block.gifts : undefined });
    case 'balance':
      return compact({ kind: block.kind, on: shown(block.on) });
    case 'friends':
      return compact({
        kind: block.kind,
        on: shown(block.on),
        place: block.place === 'screen' ? block.place : undefined,
        title: phraseOut(block.title),
        text: phraseOut(block.text, true),
      });
    case 'links':
      return compact({ kind: block.kind, on: shown(block.on), items: block.items?.length ? block.items.map((item) => itemOut(item, true)) : undefined });
    default:
      return { kind: block.kind };
  }
}

function tabOut(tab: Tab): Tab {
  return compact({ std: tab.std, label: phraseOut(tab.label) });
}

export function headerSummary(header: Header): string {
  const extra = [header.balance ? 'баланс' : null, header.avatar ? 'аватар' : null].filter((part) => part !== null);
  return extra.length > 0 ? `Логотип, название, ${extra.join(', ')}` : 'Логотип и название';
}

export function canonical(config: MiniConfig): MiniConfig {
  return {
    header: { balance: config.header.balance, avatar: config.header.avatar },
    home: config.home.map(homeOut),
    tabs: config.tabs.map(tabOut),
    profile: config.profile.map(profileOut),
  };
}

export function sameConfig(a: MiniConfig, b: MiniConfig): boolean {
  return JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
}

export function isStandard(config: MiniConfig): boolean {
  return sameConfig(config, standardConfig());
}

export function homeChanged(block: HomeBlock): boolean {
  const plain = { ...homeOut(block), on: undefined };
  const standard = { ...homeOut(standardHome(block.kind)), on: undefined };
  return JSON.stringify(plain) !== JSON.stringify(standard);
}

export function profileChanged(block: ProfileBlock): boolean {
  const plain = { ...profileOut(block), on: undefined };
  const standard = { ...profileOut(standardProfile(block.kind)), on: undefined };
  return JSON.stringify(plain) !== JSON.stringify(standard);
}

export function resetHome(block: HomeBlock): HomeBlock {
  const standard = standardHome(block.kind);
  return block.kind === 'subscription' ? standard : ({ ...standard, on: block.on } as HomeBlock);
}

export function resetProfile(block: ProfileBlock): ProfileBlock {
  return 'on' in block ? ({ ...standardProfile(block.kind), on: block.on } as ProfileBlock) : standardProfile(block.kind);
}

export function fromView(view: AdminView): MiniConfig {
  return normalizeConfig({ header: view.header, home: view.home, tabs: view.tabs, profile: view.profile });
}

export function normalizeUrl(raw: string): string {
  const value = raw.trim();
  return /^t\.me\/./i.test(value) ? `https://${value}` : value;
}

export function urlProblem(raw: string, max: number): string | null {
  const url = normalizeUrl(raw);
  if (!url) {
    return 'укажите, куда ведёт';
  }
  if (chars(url) > max) {
    return `не длиннее ${max} символов`;
  }
  if (!/^https:\/\//i.test(url)) {
    return 'нужна ссылка вида https://… или t.me/…';
  }
  if (/[\s"'<>`\\{}|^\u0000-\u001f\u007f]/.test(url)) {
    return 'в ссылке не должно быть пробелов, кавычек и скобок';
  }
  const rest = url.slice('https://'.length);
  const end = rest.search(/[/?#]/);
  const authority = end < 0 ? rest : rest.slice(0, end);
  const colon = authority.lastIndexOf(':');
  const host = colon >= 0 ? authority.slice(0, colon) : authority;
  const port = colon >= 0 ? authority.slice(colon + 1) : null;
  const hostOk = /^[\p{L}\p{N}.-]+$/u.test(host) && /[\p{L}\p{N}]/u.test(host) && !/^[.-]/.test(host) && !/[.-]$/.test(host);
  return !hostOk || (port !== null && !/^\d{1,5}$/.test(port)) ? 'в ссылке нет адреса сайта' : null;
}

export function targetProblem(to: string | null | undefined, max: number): string | null {
  return isSection(to) ? null : urlProblem(to ?? '', max);
}

export function lengthProblem(value: string | null | undefined, max: number): string | null {
  const length = chars(value ?? '');
  return length > max ? `не длиннее ${max} символов, сейчас ${length}` : null;
}

function phraseProblems(phrase: Phrase | null | undefined, max: number): string[] {
  const out: string[] = [];
  const ru = lengthProblem(phrase?.ru, max);
  const en = lengthProblem(phrase?.en, max);
  if (ru) {
    out.push(ru);
  }
  if (en) {
    out.push(`на английском ${en}`);
  }
  return out;
}

function filled(phrase: Phrase | null | undefined): boolean {
  return (phrase?.ru ?? '').trim() !== '';
}

export type Problem = { part: Part; text: string };

export function ownItemProblem(item: Item, labelMax: number, limits: Limits): string | null {
  if (!filled(item.label)) {
    return 'нужно название';
  }
  const label = phraseProblems(item.label, labelMax)[0];
  if (label) {
    return `название: ${label}`;
  }
  const link = targetProblem(item.to, limits.url);
  return link ? `ссылка: ${link}` : null;
}

export function bannerProblem(banner: Banner, limits: Limits): string | null {
  if (!banner.image?.tag && !filled(banner.title)) {
    return 'нужна картинка или заголовок';
  }
  const title = phraseProblems(banner.title, limits.banner_title)[0];
  if (title) {
    return `заголовок: ${title}`;
  }
  const text = phraseProblems(banner.text, limits.banner_text)[0];
  if (text) {
    return `текст: ${text}`;
  }
  const link = banner.to === null || banner.to === undefined ? null : targetProblem(banner.to, limits.url);
  return link ? `ссылка: ${link}` : null;
}

export function problemsOf(config: MiniConfig, limits: Limits): Problem[] {
  const out: Problem[] = [];
  const add = (part: Part, place: string, problem: string | null | undefined) => {
    if (problem) {
      out.push({ part, text: `${place}: ${problem}` });
    }
  };
  const items = (part: Part, place: string, list: Item[], labelMax: number) => {
    list.forEach((item) => {
      if (item.std) {
        add(part, `${place}, «${SECTION_TITLES[item.std]}»`, phraseProblems(item.label, labelMax)[0]);
      } else {
        add(part, `${place}, ${item.label?.ru?.trim() ? `«${item.label.ru.trim()}»` : 'ссылка без названия'}`, ownItemProblem(item, labelMax, limits));
      }
    });
  };
  for (const block of config.home) {
    switch (block.kind) {
      case 'banners':
        (block.items ?? []).forEach((banner, index) => add('banners', `Главная, баннер ${index + 1}`, bannerProblem(banner, limits)));
        break;
      case 'greeting':
        if (block.on === true && !filled(block.title) && !filled(block.text)) {
          add('greeting', 'Главная, приветствие', 'нужен заголовок или текст');
        }
        add('greeting', 'Главная, приветствие, заголовок', phraseProblems(block.title, limits.greeting_title)[0]);
        add('greeting', 'Главная, приветствие, текст', phraseProblems(block.text, limits.greeting_text)[0]);
        break;
      case 'subscription':
        add('subscription', 'Главная, подписка, заголовок', phraseProblems(block.empty?.title, limits.empty_title)[0]);
        add('subscription', 'Главная, подписка, строка', phraseProblems(block.empty?.line, limits.empty_line)[0]);
        add('subscription', 'Главная, подписка, кнопка', phraseProblems(block.empty?.button, limits.button)[0]);
        break;
      case 'actions':
        items('actions', 'Главная, кнопки', block.items ?? [], limits.button);
        break;
      case 'sections':
        items('sections', 'Главная, разделы', block.items ?? [], limits.label);
        break;
    }
  }
  for (const tab of config.tabs) {
    add('tabs', `Меню внизу, «${SECTION_TITLES[tab.std]}»`, phraseProblems(tab.label, limits.tab)[0]);
  }
  for (const block of config.profile) {
    if (block.kind === 'friends') {
      add('friends', 'Профиль, друзья, заголовок', phraseProblems(block.title, limits.friends_title)[0]);
      add('friends', 'Профиль, друзья, текст', phraseProblems(block.text, limits.friends_text)[0]);
    } else if (block.kind === 'links') {
      items('links', 'Профиль, свои ссылки', block.items ?? [], limits.label);
    }
  }
  return out;
}

export function tabFixed(key: SectionKey): boolean {
  return ALWAYS_TABS.includes(key);
}

export function itemIcon(item: Item): IconName {
  if (item.std) {
    return SECTION_ICONS[item.std];
  }
  if (item.icon) {
    return item.icon;
  }
  return isSection(item.to) ? SECTION_ICONS[item.to] : /^https:\/\/t\.me\//i.test(item.to ?? '') ? 'send' : 'globe';
}

export function itemName(item: Item, standard: (key: SectionKey) => string): string {
  const own = item.label?.ru?.trim();
  return own || (item.std ? standard(item.std) : 'Без названия');
}

export function linkText(to: string | null | undefined): string {
  if (isSection(to)) {
    return `Раздел «${SECTION_TITLES[to]}»`;
  }
  return (to ?? '').replace(/^https:\/\//i, '');
}

export function audienceText(list: Audience[] | null | undefined): string {
  const kept = audienceOut(list);
  return kept ? kept.map((key) => AUDIENCE_NAMES[key].toLowerCase()).join(', ') : 'всем';
}

export function homeSummary(block: HomeBlock, standard: (key: SectionKey) => string): string {
  switch (block.kind) {
    case 'banners': {
      const items = block.items ?? [];
      const hidden = items.filter((item) => item.on === false).length;
      if (items.length === 0) {
        return 'Пока нет';
      }
      const all = count(items.length, 'баннер', 'баннера', 'баннеров');
      return hidden ? `${all}, скрыто ${hidden}` : all;
    }
    case 'greeting':
      return block.on === true ? (block.title?.ru ?? '').trim() || (block.text?.ru ?? '').trim() || 'Нет текста' : 'Скрыто';
    case 'subscription':
      return block.look === 'compact' ? 'Компактно, трафик и главная кнопка' : 'Круг с днями, трафик и главная кнопка';
    case 'actions': {
      const names = (block.items ?? []).map((item) => itemName(item, standard));
      return names.length ? names.join(', ') : 'Без кнопок';
    }
    case 'sections': {
      const items = block.items ?? [];
      const hidden = items.filter((item) => item.on === false).length;
      const all = count(items.length, 'пункт', 'пункта', 'пунктов');
      return hidden ? `${all}, скрыто ${hidden}` : all;
    }
  }
}

export function profileSummary(block: ProfileBlock): string {
  switch (block.kind) {
    case 'person':
      return block.on === true ? 'Фото, имя и @username из Telegram' : 'Скрыто';
    case 'balance':
      return block.on === false ? 'Видят только клиенты с деньгами' : 'Сумма и пополнение';
    case 'friends':
      return block.on === false ? 'Скрыты' : block.place === 'screen' ? 'Отдельный экран' : 'Карточка со ссылками';
    case 'help':
      return block.gifts === true ? 'Подарок, история оплат и поддержка' : 'История оплат и поддержка';
    case 'links': {
      const n = (block.items ?? []).length;
      return n ? count(n, 'ссылка', 'ссылки', 'ссылок') : 'Пока нет';
    }
    case 'documents':
      return 'Из раздела «Документы»';
    case 'account':
      return 'Почта, пароль, вход по коду, язык';
  }
}
