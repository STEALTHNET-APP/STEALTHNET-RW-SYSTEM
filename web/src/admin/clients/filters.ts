import type { Currency } from '../../lib/types';

const NBSP = '\u00a0';

function groupDigits(value: number): string {
  const sign = value < 0 ? '−' : '';
  return sign + Math.abs(Math.trunc(value)).toString().replace(/\B(?=(\d{3})+(?!\d))/g, NBSP);
}

function plural(n: number, one: string, few: string, many: string): string {
  const mod10 = Math.abs(n) % 10;
  const mod100 = Math.abs(n) % 100;
  if (mod10 === 1 && mod100 !== 11) {
    return one;
  }
  return mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14) ? few : many;
}

export type Status = 'active' | 'expiring_soon' | 'trial' | 'never_connected' | 'expired' | 'limited' | 'blocked' | 'no_subscription';

export type Origin = 'bot' | 'miniapp' | 'web' | 'admin' | 'transfer';

export type Flag = boolean | null;

export type Conditions = {
  tariff: number[];
  category: number[];
  status: Status | null;
  ends: number | null;
  ended: number | null;
  autorenew: Flag;
  source: Origin[];
  lang: 'ru' | 'en' | null;
  from: string | null;
  to: string | null;
  paid: Flag;
  paid_from: number | null;
  paid_to: number | null;
  balance: Flag;
  telegram: Flag;
  email: Flag;
  bot_stopped: Flag;
  invited: Flag;
  invites: Flag;
  utm: string | null;
};

export type FilterTariff = { id: number; name: string; category_id: number; deleted: boolean };
export type FilterCategory = { id: number; name: string };
export type FilterOptions = { tariffs: FilterTariff[]; categories: FilterCategory[]; utm: string[] };

export const EMPTY: Conditions = {
  tariff: [],
  category: [],
  status: null,
  ends: null,
  ended: null,
  autorenew: null,
  source: [],
  lang: null,
  from: null,
  to: null,
  paid: null,
  paid_from: null,
  paid_to: null,
  balance: null,
  telegram: null,
  email: null,
  bot_stopped: null,
  invited: null,
  invites: null,
  utm: null,
};

export const DAYS_MAX = 365;
export const AMOUNT_MAX = 1_000_000_000;

export const STATUSES: Array<[Status, string]> = [
  ['active', 'Активные'],
  ['expiring_soon', 'Истекают за 3 дня'],
  ['trial', 'Пробный период'],
  ['never_connected', 'Не подключились'],
  ['expired', 'Истекли'],
  ['limited', 'Трафик исчерпан'],
  ['blocked', 'Заблокированы'],
  ['no_subscription', 'Нет подписки'],
];

export const ORIGINS: Array<[Origin, string]> = [
  ['bot', 'Бот'],
  ['miniapp', 'Мини-приложение'],
  ['web', 'Сайт'],
  ['admin', 'Админка'],
  ['transfer', 'Перенос'],
];

export type Kind =
  | 'status'
  | 'tariff'
  | 'category'
  | 'term'
  | 'autorenew'
  | 'source'
  | 'lang'
  | 'registered'
  | 'paid'
  | 'balance'
  | 'contact'
  | 'bot'
  | 'referral'
  | 'utm';

export type KindDef = { kind: Kind; title: string; group: string; money?: boolean; keys: Array<keyof Conditions> };

export const KINDS: KindDef[] = [
  { kind: 'status', title: 'Состояние подписки', group: 'Подписка', keys: ['status'] },
  { kind: 'tariff', title: 'Тариф', group: 'Подписка', keys: ['tariff'] },
  { kind: 'category', title: 'Категория тарифа', group: 'Подписка', keys: ['category'] },
  { kind: 'term', title: 'Срок подписки', group: 'Подписка', keys: ['ends', 'ended'] },
  { kind: 'autorenew', title: 'Автопродление', group: 'Подписка', keys: ['autorenew'] },
  { kind: 'source', title: 'Откуда пришёл', group: 'Клиент', keys: ['source'] },
  { kind: 'registered', title: 'Дата регистрации', group: 'Клиент', keys: ['from', 'to'] },
  { kind: 'lang', title: 'Язык', group: 'Клиент', keys: ['lang'] },
  { kind: 'contact', title: 'Telegram и почта', group: 'Клиент', keys: ['telegram', 'email'] },
  { kind: 'bot', title: 'Бот остановлен', group: 'Клиент', keys: ['bot_stopped'] },
  { kind: 'paid', title: 'Оплаты', group: 'Деньги', money: true, keys: ['paid', 'paid_from', 'paid_to'] },
  { kind: 'balance', title: 'Баланс', group: 'Деньги', money: true, keys: ['balance'] },
  { kind: 'referral', title: 'Приглашения', group: 'Привлечение', keys: ['invited', 'invites'] },
  { kind: 'utm', title: 'UTM-метка или старт-параметр', group: 'Привлечение', keys: ['utm'] },
];

export function kindDef(kind: Kind): KindDef {
  return KINDS.find((def) => def.kind === kind) ?? { kind, title: kind, group: '', keys: [] };
}

function flagOf(value: string | null): Flag {
  if (value === 'yes') {
    return true;
  }
  if (value === 'no') {
    return false;
  }
  return null;
}

function idsOf(value: string | null): number[] {
  if (!value) {
    return [];
  }
  const out: number[] = [];
  for (const part of value.split(',')) {
    const id = Number.parseInt(part.trim(), 10);
    if (Number.isSafeInteger(id) && id > 0 && !out.includes(id)) {
      out.push(id);
    }
  }
  return out;
}

function numberOf(value: string | null, min: number, max: number): number | null {
  if (!value || !/^\d+$/.test(value.trim())) {
    return null;
  }
  const n = Number.parseInt(value, 10);
  return n >= min && n <= max ? n : null;
}

function dateOf(value: string | null): string | null {
  return value && /^\d{4}-\d{2}-\d{2}$/.test(value) ? value : null;
}

const STATUS_KEYS = new Set<string>(STATUSES.map(([key]) => key));
const ORIGIN_KEYS = new Set<string>(ORIGINS.map(([key]) => key));

export function readParams(params: URLSearchParams): Conditions {
  const status = params.get('status') ?? params.get('segment');
  const utm = params.get('utm')?.trim() ?? '';
  return {
    tariff: idsOf(params.get('tariff')),
    category: idsOf(params.get('category')),
    status: status && STATUS_KEYS.has(status) ? (status as Status) : null,
    ends: numberOf(params.get('ends'), 1, DAYS_MAX),
    ended: numberOf(params.get('ended'), 1, DAYS_MAX),
    autorenew: flagOf(params.get('autorenew')),
    source: (params.get('source') ?? '')
      .split(',')
      .map((s) => s.trim())
      .filter((s, i, all) => ORIGIN_KEYS.has(s) && all.indexOf(s) === i) as Origin[],
    lang: params.get('lang') === 'en' ? 'en' : params.get('lang') === 'ru' ? 'ru' : null,
    from: dateOf(params.get('from')),
    to: dateOf(params.get('to')),
    paid: flagOf(params.get('paid')),
    paid_from: numberOf(params.get('paid_from'), 0, AMOUNT_MAX),
    paid_to: numberOf(params.get('paid_to'), 0, AMOUNT_MAX),
    balance: flagOf(params.get('balance')),
    telegram: flagOf(params.get('telegram')),
    email: flagOf(params.get('email')),
    bot_stopped: flagOf(params.get('bot_stopped')),
    invited: flagOf(params.get('invited')),
    invites: flagOf(params.get('invites')),
    utm: utm ? utm.slice(0, 256) : null,
  };
}

const PARAM_KEYS = [
  'tariff',
  'category',
  'status',
  'segment',
  'ends',
  'ended',
  'autorenew',
  'source',
  'lang',
  'from',
  'to',
  'paid',
  'paid_from',
  'paid_to',
  'balance',
  'telegram',
  'email',
  'bot_stopped',
  'invited',
  'invites',
  'utm',
];

function yes(value: boolean): string {
  return value ? 'yes' : 'no';
}

function pairs(c: Conditions, dates: (key: 'from' | 'to', value: string) => string): Array<[string, string]> {
  const out: Array<[string, string]> = [];
  if (c.tariff.length) {
    out.push(['tariff', c.tariff.join(',')]);
  }
  if (c.category.length) {
    out.push(['category', c.category.join(',')]);
  }
  if (c.status) {
    out.push(['status', c.status]);
  }
  if (c.ends !== null) {
    out.push(['ends', String(c.ends)]);
  }
  if (c.ended !== null) {
    out.push(['ended', String(c.ended)]);
  }
  if (c.autorenew !== null) {
    out.push(['autorenew', yes(c.autorenew)]);
  }
  if (c.source.length) {
    out.push(['source', c.source.join(',')]);
  }
  if (c.lang) {
    out.push(['lang', c.lang]);
  }
  if (c.from) {
    out.push(['from', dates('from', c.from)]);
  }
  if (c.to) {
    out.push(['to', dates('to', c.to)]);
  }
  if (c.paid !== null) {
    out.push(['paid', yes(c.paid)]);
  }
  if (c.paid_from !== null) {
    out.push(['paid_from', String(c.paid_from)]);
  }
  if (c.paid_to !== null) {
    out.push(['paid_to', String(c.paid_to)]);
  }
  for (const key of ['balance', 'telegram', 'email', 'bot_stopped', 'invited', 'invites'] as const) {
    const value = c[key];
    if (value !== null) {
      out.push([key, yes(value)]);
    }
  }
  if (c.utm) {
    out.push(['utm', c.utm]);
  }
  return out;
}

export function writeParams(c: Conditions, base: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(base);
  for (const key of PARAM_KEYS) {
    next.delete(key);
  }
  for (const [key, value] of pairs(c, (_, value) => value)) {
    next.set(key, value);
  }
  return next;
}

export function dayStart(date: string, shift = 0): string {
  const [y = 1970, m = 1, d = 1] = date.split('-').map((part) => Number.parseInt(part, 10));
  return new Date(y, m - 1, d + shift).toISOString();
}

export function localDate(iso: string, shift = 0): string {
  const at = new Date(iso);
  const day = new Date(at.getFullYear(), at.getMonth(), at.getDate() + shift);
  const pad = (n: number) => String(n).padStart(2, '0');
  return `${day.getFullYear()}-${pad(day.getMonth() + 1)}-${pad(day.getDate())}`;
}

export function apiPairs(c: Conditions): Array<[string, string]> {
  return pairs(c, (key, value) => (key === 'from' ? dayStart(value) : dayStart(value, 1)));
}

export function apiQuery(c: Conditions, search: URLSearchParams): URLSearchParams {
  const next = new URLSearchParams(search);
  for (const [key, value] of apiPairs(c)) {
    next.set(key, value);
  }
  return next;
}

export type FilterAudience = { kind: 'filter' } & Partial<{
  tariff: number[];
  category: number[];
  status: Status;
  ends: number;
  ended: number;
  autorenew: boolean;
  source: Origin[];
  lang: 'ru' | 'en';
  from: string;
  to: string;
  paid: boolean;
  paid_from: number;
  paid_to: number;
  balance: boolean;
  telegram: boolean;
  email: boolean;
  bot_stopped: boolean;
  invited: boolean;
  invites: boolean;
  utm: string;
}>;

export function conditionsJson(c: Conditions): Omit<FilterAudience, 'kind'> | null {
  const { kind: _kind, ...rest } = toAudience(c);
  return Object.keys(rest).length ? rest : null;
}

export function toAudience(c: Conditions): FilterAudience {
  const out: FilterAudience = { kind: 'filter' };
  if (c.tariff.length) {
    out.tariff = c.tariff;
  }
  if (c.category.length) {
    out.category = c.category;
  }
  if (c.status) {
    out.status = c.status;
  }
  if (c.ends !== null) {
    out.ends = c.ends;
  }
  if (c.ended !== null) {
    out.ended = c.ended;
  }
  if (c.autorenew !== null) {
    out.autorenew = c.autorenew;
  }
  if (c.source.length) {
    out.source = c.source;
  }
  if (c.lang) {
    out.lang = c.lang;
  }
  if (c.from) {
    out.from = dayStart(c.from);
  }
  if (c.to) {
    out.to = dayStart(c.to, 1);
  }
  if (c.paid_from !== null) {
    out.paid_from = c.paid_from;
  }
  if (c.paid_to !== null) {
    out.paid_to = c.paid_to;
  }
  for (const key of ['paid', 'balance', 'telegram', 'email', 'bot_stopped', 'invited', 'invites'] as const) {
    const value = c[key];
    if (value !== null) {
      out[key] = value;
    }
  }
  if (c.utm) {
    out.utm = c.utm;
  }
  return out;
}

export function fromAudience(a: FilterAudience): Conditions {
  return {
    tariff: a.tariff ?? [],
    category: a.category ?? [],
    status: a.status ?? null,
    ends: a.ends ?? null,
    ended: a.ended ?? null,
    autorenew: a.autorenew ?? null,
    source: a.source ?? [],
    lang: a.lang ?? null,
    from: a.from ? localDate(a.from) : null,
    to: a.to ? localDate(a.to, -1) : null,
    paid: a.paid ?? null,
    paid_from: a.paid_from ?? null,
    paid_to: a.paid_to ?? null,
    balance: a.balance ?? null,
    telegram: a.telegram ?? null,
    email: a.email ?? null,
    bot_stopped: a.bot_stopped ?? null,
    invited: a.invited ?? null,
    invites: a.invites ?? null,
    utm: a.utm ?? null,
  };
}

export function isSet(c: Conditions, kind: Kind): boolean {
  return kindDef(kind).keys.some((key) => {
    const value = c[key];
    return Array.isArray(value) ? value.length > 0 : value !== null;
  });
}

export function clearKind(c: Conditions, kind: Kind): Conditions {
  const next = { ...c };
  for (const key of kindDef(kind).keys) {
    (next as Record<string, unknown>)[key] = Array.isArray(EMPTY[key]) ? [] : null;
  }
  return next;
}

export function activeKinds(c: Conditions, skipStatus: boolean): Kind[] {
  return KINDS.filter((def) => !(skipStatus && def.kind === 'status') && isSet(c, def.kind)).map((def) => def.kind);
}

export function sameConditions(a: Conditions, b: Conditions): boolean {
  return JSON.stringify(a) === JSON.stringify(b);
}

function names(ids: number[], all: Array<{ id: number; name: string }>): string {
  const found = ids.map((id) => all.find((item) => item.id === id)?.name ?? `№${id}`);
  if (found.length <= 2) {
    return found.join(', ');
  }
  return `${found.slice(0, 2).join(', ')} и ещё ${found.length - 2}`;
}

function dayText(iso: string): string {
  const [y, m, d] = iso.split('-');
  return `${d}.${m}.${y}`;
}

function amount(n: number, currency: Currency): string {
  return currency === 'RUB' ? `${groupDigits(n)} ₽` : `$${groupDigits(n)}`;
}

function daysWord(n: number): string {
  return `${n} ${plural(n, 'день', 'дня', 'дней')}`;
}

export function label(kind: Kind, c: Conditions, options: FilterOptions | null, currency: Currency): string {
  switch (kind) {
    case 'status':
      return `Состояние: ${(STATUSES.find(([key]) => key === c.status)?.[1] ?? '').toLowerCase()}`;
    case 'tariff':
      return `${c.tariff.length > 1 ? 'Тарифы' : 'Тариф'}: ${names(c.tariff, options?.tariffs ?? [])}`;
    case 'category':
      return `${c.category.length > 1 ? 'Категории' : 'Категория'}: ${names(c.category, options?.categories ?? [])}`;
    case 'term':
      return c.ends !== null ? `Закончится в ближайшие ${daysWord(c.ends)}` : `Закончилась за последние ${daysWord(c.ended ?? 0)}`;
    case 'autorenew':
      return c.autorenew ? 'Автопродление включено' : 'Автопродление выключено';
    case 'source':
      return `Откуда: ${c.source.map((key) => (ORIGINS.find(([k]) => k === key)?.[1] ?? key).toLowerCase()).join(', ')}`;
    case 'lang':
      return c.lang === 'en' ? 'Язык: английский' : 'Язык: русский';
    case 'registered':
      if (c.from && c.to) {
        return `Регистрация ${dayText(c.from)} — ${dayText(c.to)}`;
      }
      return c.from ? `Регистрация с ${dayText(c.from)}` : `Регистрация по ${dayText(c.to ?? '')}`;
    case 'contact': {
      const parts: string[] = [];
      if (c.telegram !== null) {
        parts.push(c.telegram ? 'есть Telegram' : 'без Telegram');
      }
      if (c.email !== null) {
        parts.push(c.email ? 'есть почта' : 'без почты');
      }
      const text = parts.join(', ');
      return text.charAt(0).toUpperCase() + text.slice(1);
    }
    case 'bot':
      return c.bot_stopped ? 'Бот остановлен' : 'Бот не остановлен';
    case 'paid': {
      if (c.paid === false) {
        return 'Не платили';
      }
      if (c.paid_from !== null && c.paid_to !== null) {
        return `Оплатили от ${amount(c.paid_from, currency)} до ${amount(c.paid_to, currency)}`;
      }
      if (c.paid_from !== null) {
        return `Оплатили от ${amount(c.paid_from, currency)}`;
      }
      if (c.paid_to !== null) {
        return `Оплатили до ${amount(c.paid_to, currency)}`;
      }
      return 'Платили';
    }
    case 'balance':
      return c.balance ? 'Есть деньги на балансе' : 'Баланс пустой';
    case 'referral': {
      const parts: string[] = [];
      if (c.invited !== null) {
        parts.push(c.invited ? 'пришли по приглашению' : 'пришли не по приглашению');
      }
      if (c.invites !== null) {
        parts.push(c.invites ? 'приглашали друзей' : 'не приглашали друзей');
      }
      const text = parts.join(', ');
      return text.charAt(0).toUpperCase() + text.slice(1);
    }
    case 'utm':
      return `Метка: ${c.utm ?? ''}`;
  }
}

export function found(n: number): string {
  return `Найдено ${groupDigits(n)}`;
}
