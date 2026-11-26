import { count, money, type Money } from '../lib/format';
import { msg, t } from '../lib/i18n';
import type { OfferPromotion } from '../lib/promo';
import type { Currency } from '../lib/types';
import { withBotStart } from '../lib/utmTags';

export type Reset = 'no_reset' | 'day' | 'week' | 'month' | 'month_rolling' | 'on_payment';

export type Offer = {
  category: { id: number; name: string; sort: number };
  tariff: {
    id: number;
    name: string;
    description: string | null;
    devices: { included: number; extra: { max: number; monthly: { usd?: number; rub?: number } } | null };
    traffic: { limit_bytes: number | null; reset: Reset };
  };
  periods: Array<{ period_id: number; days: number; months: number; price: Money; per_month: Money; savings_percent: number | null; promotion?: OfferPromotion | null }>;
  countries: string[];
};

export type PublicCatalog = { currency: Currency; tariffs: Offer[] };

export type BlockKind = 'hero' | 'features' | 'steps' | 'plans' | 'servers' | 'faq' | 'cta' | 'footer';
export type SectionKind = Exclude<BlockKind, 'hero' | 'footer'>;
export type MenuKind = 'features' | 'steps' | 'plans' | 'servers' | 'faq';
export type ItemKind = 'features' | 'steps' | 'faq';
export type ImageKind = 'png' | 'jpeg' | 'webp';
export type Art = 'default' | 'image' | 'none';

export type LandingLink = { label: string; to: string };
export type LandingImage = { tag: string; kind?: ImageKind; width?: number; height?: number; url?: string };
export type LandingItem = { std?: string; title?: string; text?: string; link?: LandingLink | null; image?: LandingImage | null };

type Text = string | null | undefined;

export type HeroBlock = { kind: 'hero'; title?: Text; lead?: Text; note?: Text; buttons?: LandingLink[] | null; art?: Art | null; image?: LandingImage | null };
export type FeaturesBlock = { kind: 'features'; on?: boolean; title?: Text; menu?: Text; items?: LandingItem[] | null };
export type StepsBlock = { kind: 'steps'; on?: boolean; title?: Text; menu?: Text; items?: LandingItem[] | null };
export type PlansBlock = { kind: 'plans'; on?: boolean; title?: Text; menu?: Text; text?: Text };
export type ServersBlock = {
  kind: 'servers';
  on?: boolean;
  menu?: Text;
  servers_title?: Text;
  servers_note?: Text;
  apps_title?: Text;
  apps?: string[] | null;
  apps_note?: Text;
};
export type FaqBlock = { kind: 'faq'; on?: boolean; title?: Text; menu?: Text; items?: LandingItem[] | null };
export type CtaBlock = { kind: 'cta'; on?: boolean; title?: Text; text?: Text; buttons?: LandingLink[] | null };
export type FooterBlock = { kind: 'footer'; text?: Text; links?: LandingLink[] | null };

export type Block = HeroBlock | FeaturesBlock | StepsBlock | PlansBlock | ServersBlock | FaqBlock | CtaBlock | FooterBlock;
export type BlockOf<K extends BlockKind> = Extract<Block, { kind: K }>;
export type ItemBlock = FeaturesBlock | StepsBlock | FaqBlock;

export type PublicLanding = { version?: string; blocks?: unknown; css?: unknown };

export const PREVIEW_MESSAGES = {
  draft: 'sn-landing-draft',
  ready: 'sn-landing-ready',
  pick: 'sn-landing-pick',
} as const;

export type PreviewDraft = { type: typeof PREVIEW_MESSAGES.draft; blocks: Block[]; theme: 'dark' | 'light'; css?: string; focus?: BlockKind | null };

export type LandingContext = {
  phone: boolean;
  currency: Currency;
  trial: number;
  bot: string | null;
  support: string | null;
  brand: string;
  year: number;
  offer: Offer | null;
  extra: Money | null;
  from: Money | null;
  botName?: string | null;
  botStart?: string | null;
};

export const DEFAULT_ORDER: BlockKind[] = ['hero', 'features', 'steps', 'plans', 'servers', 'faq', 'cta', 'footer'];

export const BLOCK_NAMES: Record<BlockKind, string> = {
  hero: 'Первый экран',
  features: 'Преимущества',
  steps: 'Как подключиться',
  plans: 'Тарифы',
  servers: 'Серверы и приложения',
  faq: 'Вопросы и ответы',
  cta: 'Призыв в конце',
  footer: 'Подвал',
};

export const ANCHORS: Record<SectionKind, string> = {
  features: 'ways',
  steps: 'steps',
  plans: 'plans',
  servers: 'servers',
  faq: 'faq',
  cta: 'start',
};

export const SECTION_KINDS: SectionKind[] = ['features', 'steps', 'plans', 'servers', 'faq', 'cta'];

export const DEFAULT_MENU: Record<MenuKind, string> = {
  features: msg('Кабинет и бот'),
  steps: '',
  plans: msg('Тарифы'),
  servers: '',
  faq: msg('Вопросы'),
};

export const STD_KEYS: Record<ItemKind, string[]> = {
  features: ['cabinet', 'bot'],
  steps: ['pick', 'pay', 'connect'],
  faq: ['channels', 'devices', 'renewal', 'refund', 'privacy'],
};

export const DEFAULT_APPS = ['Happ', 'v2rayTun', 'Hiddify', 'Streisand', 'Clash Verge', 'NekoBox'];

export const HERO_TITLE = msg('Подключение за минуту');
export const HERO_BUTTONS: LandingLink[] = [
  { label: msg('Начать в браузере'), to: 'cabinet' },
  { label: msg('Начать в Telegram'), to: 'bot' },
];
export const CTA_BUTTONS: LandingLink[] = [
  { label: msg('Личный кабинет'), to: 'cabinet' },
  { label: msg('Telegram-бот'), to: 'bot' },
];
export const FOOTER_LINKS: LandingLink[] = [
  { label: msg('Поддержка'), to: 'support' },
  { label: msg('Telegram-бот'), to: 'bot' },
  { label: msg('Личный кабинет'), to: 'cabinet' },
];

export const DEFAULT_TEXT = {
  featuresTitle: msg('Где удобнее, там и пользуйтесь'),
  stepsTitle: msg('Как подключиться'),
  plansTitle: msg('Тарифы'),
  serversTitle: msg('Серверы'),
  appsTitle: msg('Приложения'),
  faqTitle: msg('Вопросы'),
};

export function trialWord(days: number): string {
  return count(days, 'день', 'дня', 'дней');
}

export function heroLead(phone: boolean): string {
  return phone
    ? t('Купите подписку в личном кабинете на сайте или в Telegram-боте. После оплаты придёт ссылка для приложения на iPhone, Android, Windows, macOS или Linux.')
    : t('Купите подписку в личном кабинете на сайте или в Telegram-боте, как вам удобнее. После оплаты придёт ссылка для приложения на iPhone, Android, Windows, macOS или Linux.');
}

export function heroNote(trial: number): string {
  return trial ? t('Первые {days} бесплатно, оплата не нужна', { days: trialWord(trial) }) : '';
}

export function cheapest(offers: Offer[]): Money | null {
  let best: Money | null = null;
  for (const offer of offers) {
    for (const period of offer.periods) {
      if (!best || period.per_month.minor < best.minor) {
        best = period.per_month;
      }
    }
  }
  return best;
}

export function ctaTitle(trial: number, from: Money | null): string {
  if (trial) {
    return t('{days} бесплатно', { days: trialWord(trial) });
  }
  return from ? t('От {price} в месяц', { price: money(from) }) : t('Оформите подписку');
}

export function ctaText(trial: number): string {
  return trial ? t('Откройте кабинет или бота и включите пробный период.') : t('Откройте кабинет или бота и выберите тариф.');
}

export function serversNote(phone: boolean): string {
  return phone ? t('Страну можно сменить в приложении') : t('Переключаться между странами можно в приложении.');
}

export function appsNote(phone: boolean): string {
  return phone ? t('Кабинет и бот подскажут, какое поставить') : t('Кабинет и бот подскажут, какое поставить на ваше устройство.');
}

export function footerText(text: Text, brand: string, year: number): string {
  if (text === null || text === undefined) {
    return `© ${year} ${brand.toUpperCase()}`;
  }
  return text.split('{year}').join(String(year));
}

export function payText(currency: Currency): string {
  return currency === 'RUB'
    ? t('Через СБП, картой российского банка, ЮMoney или звёздами Telegram.')
    : t('Криптовалютой (USDT, TON, BTC), картой Visa или Mastercard любой страны или звёздами Telegram.');
}

type StdItem = { title: string; text: string; link?: LandingLink };

export function stdItem(kind: ItemKind, key: string, ctx: LandingContext): StdItem | null {
  if (kind === 'features') {
    if (key === 'cabinet') {
      return {
        title: t('Личный кабинет в браузере'),
        text: ctx.phone
          ? t('Вход через Telegram. Удобно с компьютера: оплата, устройства, инструкции и поддержка.')
          : t('Вход через Telegram. Удобно с компьютера: тарифы и оплата, устройства, инструкции по подключению и поддержка.'),
        link: { label: t('Открыть кабинет'), to: 'cabinet' },
      };
    }
    if (key === 'bot') {
      return {
        title: t('Telegram-бот и мини-приложение'),
        text: t('Без регистрации: открыли бота, и подписка уже в чате. Там же оплата, ссылка для приложения и напоминания о продлении.'),
        link: { label: t('Открыть бота'), to: 'bot' },
      };
    }
    return null;
  }
  if (kind === 'steps') {
    if (key === 'pick') {
      return { title: t('Выберите тариф'), text: t('В кабинете или в боте: месяц, три месяца или год.') };
    }
    if (key === 'pay') {
      return { title: t('Оплатите'), text: payText(ctx.currency) };
    }
    if (key === 'connect') {
      return {
        title: t('Подключитесь'),
        text: ctx.phone
          ? t('Ссылка подписки появится в кабинете или в боте. Откройте её в приложении.')
          : t('Откройте ссылку подписки в приложении. Кабинет и бот подскажут, какое поставить.'),
      };
    }
    return null;
  }
  switch (key) {
    case 'channels':
      return {
        title: t('Чем кабинет отличается от бота?'),
        text: t('По возможностям ничем: подписка, оплата, устройства и поддержка есть в обоих. Кабинет удобнее с компьютера, бот — с телефона.'),
      };
    case 'devices': {
      const offer = ctx.offer;
      const extra = offer?.tariff.devices.extra;
      const devices = offer ? count(offer.tariff.devices.included, 'устройство', 'устройства', 'устройств') : '';
      return {
        title: t('Сколько устройств можно подключить?'),
        text: offer
          ? extra?.max && ctx.extra
            ? t('В тариф входят {devices}. Ещё {extra} можно докупить в кабинете или в боте, {price} в месяц за каждое.', { devices, extra: extra.max, price: money(ctx.extra) })
            : t('В тариф входят {devices}.', { devices })
          : t('Сколько устройств входит, указано в тарифе.'),
      };
    }
    case 'renewal':
      return {
        title: t('Что будет, когда подписка закончится?'),
        text:
          ctx.currency === 'RUB'
            ? t('Напомним заранее в боте. Если включено автопродление, оплата спишется с баланса или сохранённой карты.')
            : t('Напомним заранее в боте. Если включено автопродление, оплата спишется с баланса.'),
      };
    case 'refund':
      return { title: t('Можно вернуть деньги?'), text: t('Да, если сервис у вас не заработал. Напишите в поддержку.') };
    case 'privacy':
      return { title: t('Вы храните, какие сайты я открываю?'), text: t('Нет. Мы не записываем, какие сайты вы открываете.') };
    default:
      return null;
  }
}

export function defaultItems(kind: ItemKind): LandingItem[] {
  return STD_KEYS[kind].map((std) => ({ std }));
}

export function itemKind(kind: BlockKind): ItemKind | null {
  return kind === 'features' || kind === 'steps' || kind === 'faq' ? kind : null;
}

export function text(value: Text, fallback: string): string {
  return value === null || value === undefined ? fallback : value;
}

export function isOn(block: Block): boolean {
  if (block.kind === 'hero' || block.kind === 'footer') {
    return true;
  }
  return block.on !== false;
}

export function imageUrl(image: LandingImage | null | undefined): string | null {
  if (!image?.tag) {
    return null;
  }
  if (image.url) {
    return image.url;
  }
  const ext = image.kind === 'jpeg' ? 'jpg' : (image.kind ?? 'png');
  return `/api/public/landing/${image.tag}.${ext}`;
}

export function blankBlock(kind: BlockKind): Block {
  return kind === 'hero' || kind === 'footer' ? ({ kind } as Block) : ({ kind, on: true } as Block);
}

export function defaultBlocks(): Block[] {
  return DEFAULT_ORDER.map(blankBlock);
}

const KINDS = new Set<string>(DEFAULT_ORDER);

export function normalizeBlocks(raw: unknown): Block[] {
  const list = Array.isArray(raw) ? raw : [];
  const seen = new Map<BlockKind, Block>();
  const order: BlockKind[] = [];
  for (const entry of list) {
    if (!entry || typeof entry !== 'object') {
      continue;
    }
    const kind = (entry as { kind?: unknown }).kind;
    if (typeof kind !== 'string' || !KINDS.has(kind) || seen.has(kind as BlockKind)) {
      continue;
    }
    seen.set(kind as BlockKind, entry as Block);
    order.push(kind as BlockKind);
  }
  const middle: BlockKind[] = order.filter((kind) => kind !== 'hero' && kind !== 'footer');
  DEFAULT_ORDER.forEach((kind, index) => {
    if (kind === 'hero' || kind === 'footer' || middle.includes(kind)) {
      return;
    }
    let at = 0;
    for (let i = index - 1; i >= 0; i -= 1) {
      const before = DEFAULT_ORDER[i];
      const found = before ? middle.indexOf(before) : -1;
      if (found >= 0) {
        at = found + 1;
        break;
      }
    }
    middle.splice(at, 0, kind);
  });
  return ['hero' as BlockKind, ...middle, 'footer' as BlockKind].map((kind) => seen.get(kind) ?? blankBlock(kind));
}

export function findBlock<K extends BlockKind>(blocks: Block[], kind: K): BlockOf<K> {
  return (blocks.find((b) => b.kind === kind) ?? blankBlock(kind)) as BlockOf<K>;
}

export function visibleSections(blocks: Block[]): Set<SectionKind> {
  const set = new Set<SectionKind>();
  for (const block of blocks) {
    if (block.kind !== 'hero' && block.kind !== 'footer' && isOn(block)) {
      set.add(block.kind);
    }
  }
  return set;
}

export type ResolvedLink = { label: string; href: string; route: boolean };

export type Availability = { ok: true; href: string; route: boolean } | { ok: false; reason: string };

export function linkTarget(to: string, ctx: LandingContext, sections: Set<SectionKind>): Availability {
  if (to === 'cabinet') {
    return { ok: true, href: '/cabinet', route: true };
  }
  if (to === 'bot') {
    return ctx.bot ? { ok: true, href: ctx.bot, route: false } : { ok: false, reason: 'бот не подключён' };
  }
  if (to === 'support') {
    return ctx.support
      ? { ok: true, href: ctx.support, route: false }
      : { ok: false, reason: 'нет ссылки на поддержку в «Настройки → Общие»' };
  }
  if (to.startsWith('#')) {
    const kind = to.slice(1) as SectionKind;
    if (!(kind in ANCHORS)) {
      return { ok: false, reason: 'такого блока нет' };
    }
    return sections.has(kind) ? { ok: true, href: `#${ANCHORS[kind]}`, route: false } : { ok: false, reason: `блок «${BLOCK_NAMES[kind]}» скрыт` };
  }
  if (/^https?:\/\/\S+$/i.test(to)) {
    return { ok: true, href: withBotStart(to, ctx.botName, ctx.botStart), route: false };
  }
  return { ok: false, reason: 'ссылка не подходит' };
}

export function resolveLink(link: LandingLink, ctx: LandingContext, sections: Set<SectionKind>): ResolvedLink | null {
  const target = linkTarget(link.to, ctx, sections);
  if (!target.ok || !link.label.trim()) {
    return null;
  }
  return { label: link.label, href: target.href, route: target.route };
}

export function resolveLinks(links: LandingLink[] | null | undefined, fallback: LandingLink[], ctx: LandingContext, sections: Set<SectionKind>): ResolvedLink[] {
  return (links ?? fallback.map((link) => ({ ...link, label: t(link.label) }))).map((link) => resolveLink(link, ctx, sections)).filter((link): link is ResolvedLink => link !== null);
}

export type ShownItem = { key: string; title: string; text: string; link: ResolvedLink | null; image: LandingImage | null };

export function itemContent(kind: ItemKind, item: LandingItem, ctx: LandingContext): { title: string; text: string; link: LandingLink | null; image: LandingImage | null } {
  if (item.std) {
    const std = stdItem(kind, item.std, ctx);
    return { title: std?.title ?? '', text: std?.text ?? '', link: std?.link ?? null, image: null };
  }
  return { title: item.title ?? '', text: item.text ?? '', link: item.link ?? null, image: item.image ?? null };
}

export function shownItems(kind: ItemKind, items: LandingItem[] | null | undefined, ctx: LandingContext, sections: Set<SectionKind>): ShownItem[] {
  const out: ShownItem[] = [];
  (items ?? defaultItems(kind)).forEach((item, index) => {
    const content = itemContent(kind, item, ctx);
    if (item.std && !stdItem(kind, item.std, ctx)) {
      return;
    }
    let link: ResolvedLink | null = null;
    if (content.link) {
      link = resolveLink(content.link, ctx, sections);
      if (!link) {
        return;
      }
    }
    out.push({ key: item.std ? `std-${item.std}` : `item-${index}`, title: content.title, text: content.text, link, image: content.image });
  });
  return out;
}

export function menuLinks(blocks: Block[]): Array<{ label: string; href: string }> {
  const out: Array<{ label: string; href: string }> = [];
  for (const block of blocks) {
    if (block.kind === 'hero' || block.kind === 'footer' || block.kind === 'cta' || !isOn(block)) {
      continue;
    }
    const label = text(block.menu, t(DEFAULT_MENU[block.kind])).trim();
    if (label) {
      out.push({ label, href: `#${ANCHORS[block.kind]}` });
    }
  }
  return out;
}

export function landingContext(input: {
  phone: boolean;
  currency: Currency;
  trial: number;
  bot: string | null;
  support: string | null;
  brand: string;
  offer: Offer | null;
  offers: Offer[];
  botName?: string | null;
  botStart?: string | null;
}): LandingContext {
  const { offers, ...rest } = input;
  const extraConfig = input.offer?.tariff.devices.extra;
  let extra: Money | null = null;
  if (extraConfig && extraConfig.max) {
    const minor = input.currency === 'RUB' ? extraConfig.monthly.rub : extraConfig.monthly.usd;
    extra = minor ? { minor, currency: input.currency } : null;
  }
  return { ...rest, year: new Date().getFullYear(), extra, from: cheapest(offers) };
}
