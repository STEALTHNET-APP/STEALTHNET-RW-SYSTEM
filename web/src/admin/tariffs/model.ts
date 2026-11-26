import { count, groupDigits, money, NBSP, periodLabel, plural } from '../../lib/format';
import type { Currency } from '../../lib/types';

export type Prices = { usd: number | null; rub: number | null };
export type Period = { id: number; days: number; prices: Prices; stars: number | null; sort: number; active: boolean };
export type Devices = { included: number; extra: { max: number; monthly: Prices; monthly_stars: number | null } | null };
export type TrafficReset = 'no_reset' | 'day' | 'week' | 'month' | 'month_rolling' | 'on_payment';
export type Traffic = { limit_bytes: number | null; reset: TrafficReset };
export type Leftover = 'burn' | 'carry';
export type TrafficPack = { gb: number; prices: Prices; stars: number | null };
export type TrafficAddon = { packs: TrafficPack[]; leftover: Leftover };
export type Discount = { from: number; percent: number };
export type Tariff = {
  id: number;
  category_id: number;
  name: string;
  description: string | null;
  on_sale: boolean;
  sort: number;
  periods: Period[];
  devices: Devices;
  traffic: Traffic;
  traffic_addon?: TrafficAddon | null;
  squads: string[];
  external_squad: string | null;
  device_discounts?: Discount[];
};
export type Category = { id: number; name: string; sort: number; visible: boolean; subscription_limit?: number | null };
export type Channels = { bot: boolean; cabinet: boolean; miniapp: boolean };
export type Stats = { tariff_id: number; sales_30d: number; channels: Channels };
export type BoundTerm = { provider: string; title: string; tariff_id: number; period_id: number; amount: { minor: number; currency: Currency }; every: string };
export type View = { currency: Currency; categories: Category[]; tariffs: Tariff[]; stats: Stats[]; renewals?: BoundTerm[] };
export type Squad = { uuid: string; name: string; nodes: number | null };
export type Applied = { users: number; failed: number; squads: boolean; traffic: boolean; devices: boolean } | null;

export const GIB = 1024 ** 3;

export const RESETS: Array<{ value: TrafficReset; label: string; short: string; hint: string }> = [
  { value: 'month', label: 'Каждый месяц, 1-го числа', short: 'сброс 1-го числа', hint: '' },
  { value: 'month_rolling', label: 'Каждый месяц, в день оформления', short: 'сброс в день оформления', hint: 'В то же число, когда клиент впервые получил подписку' },
  { value: 'week', label: 'Каждую неделю, по понедельникам', short: 'сброс по понедельникам', hint: '' },
  { value: 'day', label: 'Каждый день', short: 'сброс каждый день', hint: '' },
  { value: 'on_payment', label: 'При каждой оплате', short: 'сброс при оплате', hint: 'Включая автопродление' },
  { value: 'no_reset', label: 'Без сброса', short: 'без сброса', hint: 'Один лимит на весь срок подписки' },
];

export function priceOf(prices: Prices, currency: Currency): number | null {
  return currency === 'RUB' ? prices.rub : prices.usd;
}

export function priceText(minor: number | null, currency: Currency): string {
  return minor === null ? '—' : money({ minor, currency }, false);
}

export function toRaw(minor: number | null, currency: Currency): string {
  if (minor === null) {
    return '';
  }
  if (currency === 'RUB') {
    return String(Math.round(minor / 100));
  }
  return (minor / 100).toFixed(2);
}

export function fromRaw(raw: string, currency: Currency): number | null | 'bad' {
  const clean = raw.replace(/\s/g, '').replace(',', '.');
  if (!clean) {
    return null;
  }
  if (currency === 'RUB') {
    if (!/^\d+$/.test(clean)) {
      return 'bad';
    }
    return Number(clean) * 100;
  }
  if (!/^\d+(\.\d{0,2})?$/.test(clean)) {
    return 'bad';
  }
  return Math.round(Number(clean) * 100);
}

export function monthsOf(days: number): number {
  return Math.max(1, Math.floor((days * 12 + 182) / 365));
}

export function perMonthNote(days: number, minor: number | null, base: number | null, currency: Currency): string {
  if (minor === null || days < 45) {
    return '';
  }
  const months = monthsOf(days);
  const perMonth = Math.round(minor / months);
  let note = `= ${money({ minor: currency === 'RUB' ? Math.round(perMonth / 100) * 100 : perMonth, currency })} в месяц`;
  if (base && base > 0) {
    const saving = Math.round((1 - minor / months / base) * 100);
    if (saving > 0) {
      note += `, выгоднее на ${saving}%`;
    }
  }
  return note;
}

export function fromDevices(n: number): string {
  return `${n}${NBSP}${plural(n, 'доп. устройства', 'доп. устройств', 'доп. устройств')}`;
}

export function discountsText(discounts: Discount[]): string {
  return discounts.map((d, index) => `от ${index === 0 ? fromDevices(d.from) : d.from} — ${d.percent}%`).join(', ');
}

export function devicesText(devices: Devices, discounts: Discount[] = []): string {
  const base = count(devices.included, 'устройство', 'устройства', 'устройств');
  if (!devices.extra) {
    return base;
  }
  const extra = `${base}, можно докупить ещё ${devices.extra.max}`;
  return discounts.length ? `${extra}, скидка ${discountsText(discounts)}` : extra;
}

export function limitText(limit: number | null | undefined): string {
  return limit ? `не больше ${limit}` : 'без ограничений';
}

export function trafficText(traffic: Traffic): string {
  if (!traffic.limit_bytes) {
    return 'без лимита трафика';
  }
  const gb = Math.round(traffic.limit_bytes / GIB);
  const reset = RESETS.find((r) => r.value === traffic.reset)?.short ?? '';
  return `${groupDigits(gb)}${NBSP}ГБ${reset ? `, ${reset}` : ''}`;
}

export function termText(days: number): string {
  return periodLabel(days) ?? `${days}${NBSP}дней`;
}

export function salesWord(n: number): string {
  const mod10 = n % 10;
  const mod100 = n % 100;
  const word = mod10 === 1 && mod100 !== 11 ? 'продажа' : mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14) ? 'продажи' : 'продаж';
  return `${word} за 30 дней`;
}

export type DraftPeriod = { key: string; id: number | null; days: string; price: string; stars: string; other: number | null; sort: number; active: boolean };

export type DraftPack = { key: string; gb: string; price: string; stars: string; other: number | null };

export type DraftDiscount = { key: string; from: string; percent: string };

export type Draft = {
  id: number | null;
  category_id: number;
  name: string;
  description: string;
  on_sale: boolean;
  sort: number;
  periods: DraftPeriod[];
  included: string;
  extraOn: boolean;
  extraPrice: string;
  extraOther: number | null;
  extraStars: string;
  extraMax: string;
  discounts: DraftDiscount[];
  gb: string;
  reset: TrafficReset;
  packs: DraftPack[];
  leftover: Leftover;
  squads: string[];
  external_squad: string | null;
  channels: Channels;
  apply: boolean;
};

let seq = 0;

export function nextKey(): string {
  seq += 1;
  return `p${seq}`;
}

export function draftOf(tariff: Tariff, channels: Channels, currency: Currency): Draft {
  const other = (prices: Prices) => (currency === 'RUB' ? prices.usd : prices.rub);
  return {
    id: tariff.id,
    category_id: tariff.category_id,
    name: tariff.name,
    description: tariff.description ?? '',
    on_sale: tariff.on_sale,
    sort: tariff.sort,
    periods: tariff.periods.map((p) => ({
      key: nextKey(),
      id: p.id,
      days: String(p.days),
      price: toRaw(priceOf(p.prices, currency), currency),
      stars: p.stars === null ? '' : String(p.stars),
      other: other(p.prices),
      sort: p.sort,
      active: p.active,
    })),
    included: String(tariff.devices.included),
    extraOn: tariff.devices.extra !== null,
    extraPrice: tariff.devices.extra ? toRaw(priceOf(tariff.devices.extra.monthly, currency), currency) : currency === 'RUB' ? '49' : '0.99',
    extraOther: tariff.devices.extra ? other(tariff.devices.extra.monthly) : null,
    extraStars: tariff.devices.extra?.monthly_stars ? String(tariff.devices.extra.monthly_stars) : '',
    extraMax: tariff.devices.extra ? String(tariff.devices.extra.max) : '2',
    discounts: (tariff.device_discounts ?? []).map((d) => ({ key: nextKey(), from: String(d.from), percent: String(d.percent) })),
    gb: tariff.traffic.limit_bytes ? String(Math.round(tariff.traffic.limit_bytes / GIB)) : '',
    reset: tariff.traffic.reset,
    packs: (tariff.traffic_addon?.packs ?? []).map((pack) => ({
      key: nextKey(),
      gb: String(pack.gb),
      price: toRaw(priceOf(pack.prices, currency), currency),
      stars: pack.stars === null ? '' : String(pack.stars),
      other: other(pack.prices),
    })),
    leftover: tariff.traffic_addon?.leftover ?? 'burn',
    squads: [...tariff.squads],
    external_squad: tariff.external_squad,
    channels: { ...channels },
    apply: true,
  };
}

export function blankDraft(categoryId: number, squads: Squad[], currency: Currency): Draft {
  return {
    id: null,
    category_id: categoryId,
    name: '',
    description: '',
    on_sale: true,
    sort: 0,
    periods: [{ key: nextKey(), id: null, days: '30', price: '', stars: '', other: null, sort: 0, active: true }],
    included: '3',
    extraOn: false,
    extraPrice: currency === 'RUB' ? '49' : '0.99',
    extraOther: null,
    extraStars: '',
    extraMax: '2',
    discounts: [],
    gb: '',
    reset: 'no_reset',
    packs: [],
    leftover: 'burn',
    squads: squads[0] ? [squads[0].uuid] : [],
    external_squad: null,
    channels: { bot: true, cabinet: true, miniapp: true },
    apply: false,
  };
}

export function inputOf(draft: Draft, currency: Currency): { body: unknown; error: string | null } {
  const periods = [];
  for (const [index, p] of draft.periods.entries()) {
    const days = Number(p.days);
    if (!Number.isInteger(days) || days < 1 || days > 3650) {
      return { body: null, error: 'Срок: от 1 до 3650 дней' };
    }
    const price = fromRaw(p.price, currency);
    if (price === 'bad') {
      return { body: null, error: currency === 'RUB' ? 'Цена: целое число рублей' : 'Цена: число, до двух знаков после точки' };
    }
    if (price === null) {
      return { body: null, error: `Цена: укажите цену для срока ${days} дн.` };
    }
    const stars = p.stars.trim() ? Number(p.stars) : null;
    if (stars !== null && (!Number.isInteger(stars) || stars <= 0)) {
      return { body: null, error: 'Цена в звёздах: целое число больше нуля' };
    }
    const prices = currency === 'RUB' ? { rub: price, usd: p.other } : { usd: price, rub: p.other };
    periods.push({ days, prices, stars, sort: index, active: p.active });
  }
  const included = Number(draft.included);
  if (!Number.isInteger(included) || included < 1 || included > 100) {
    return { body: null, error: 'Устройств в тарифе: от 1 до 100' };
  }
  let extra = null;
  const discounts: Discount[] = [];
  if (draft.extraOn) {
    const max = Number(draft.extraMax);
    const price = fromRaw(draft.extraPrice, currency);
    const stars = draft.extraStars.trim() ? Number(draft.extraStars) : null;
    if (!Number.isInteger(max) || max < 1 || max > 100) {
      return { body: null, error: 'Доп. устройств не больше: от 1 до 100' };
    }
    if (price === 'bad' || price === null) {
      return { body: null, error: 'Цена доп. устройства не заполнена' };
    }
    if (stars !== null && (!Number.isInteger(stars) || stars <= 0)) {
      return { body: null, error: 'Цена устройства в звёздах: целое число больше нуля' };
    }
    const monthly = currency === 'RUB' ? { rub: price, usd: draft.extraOther } : { usd: price, rub: draft.extraOther };
    extra = { max, monthly, monthly_stars: stars };
    for (const d of draft.discounts) {
      const from = Number(d.from);
      const percent = Number(d.percent);
      if (!Number.isInteger(from) || from < 1 || from > max) {
        return { body: null, error: `Скидка за доп. устройства: число устройств от 1 до ${max}` };
      }
      if (!Number.isInteger(percent) || percent < 1 || percent > 99) {
        return { body: null, error: 'Скидка за доп. устройства: от 1 до 99 %' };
      }
      if (discounts.some((x) => x.from === from)) {
        return { body: null, error: `Скидка от ${fromDevices(from)} указана дважды` };
      }
      discounts.push({ from, percent });
    }
    discounts.sort((a, b) => a.from - b.from);
    for (const [index, step] of discounts.entries()) {
      const previous = index > 0 ? discounts[index - 1] : undefined;
      if (previous && step.percent <= previous.percent) {
        return { body: null, error: `Скидка от ${fromDevices(step.from)} должна быть больше, чем от ${previous.from}` };
      }
    }
  }
  const gb = draft.gb.trim() ? Number(draft.gb) : null;
  if (gb !== null && (!Number.isInteger(gb) || gb < 1)) {
    return { body: null, error: 'Лимит трафика: целое число гигабайт' };
  }
  const packs: TrafficPack[] = [];
  if (gb !== null) {
    for (const pack of draft.packs) {
      const size = Number(pack.gb);
      if (!Number.isInteger(size) || size < 1 || size > 100000) {
        return { body: null, error: 'Пакет трафика: от 1 до 100 000 ГБ' };
      }
      if (packs.some((p) => p.gb === size)) {
        return { body: null, error: `Пакет ${size} ГБ указан дважды` };
      }
      const price = fromRaw(pack.price, currency);
      if (price === 'bad' || price === null) {
        return { body: null, error: `Пакет ${size} ГБ: укажите цену` };
      }
      const stars = pack.stars.trim() ? Number(pack.stars) : null;
      if (stars !== null && (!Number.isInteger(stars) || stars <= 0)) {
        return { body: null, error: 'Цена пакета в звёздах: целое число больше нуля' };
      }
      const prices = currency === 'RUB' ? { rub: price, usd: pack.other } : { usd: price, rub: pack.other };
      packs.push({ gb: size, prices, stars });
    }
  }
  return {
    error: null,
    body: {
      category_id: draft.category_id,
      name: draft.name,
      description: draft.description.trim() ? draft.description.trim() : null,
      on_sale: draft.on_sale,
      sort: draft.sort,
      devices: { included, extra },
      traffic: { limit_bytes: gb === null ? null : gb * GIB, reset: gb === null ? 'no_reset' : draft.reset },
      traffic_addon: packs.length ? { packs, leftover: draft.leftover } : null,
      squads: draft.squads,
      external_squad: draft.external_squad,
      periods,
      device_discounts: discounts,
    },
  };
}
