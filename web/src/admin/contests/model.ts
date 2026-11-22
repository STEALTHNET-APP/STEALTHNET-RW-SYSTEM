import { ApiError } from '../../lib/api';
import { count, dateLong, days, money, NBSP, plural, timeHM, type Money } from '../../lib/format';
import type { Currency } from '../../lib/types';

export const BASE = '/api/admin/m/contests';
export const ROOT = '/admin/m/contests';

export type Prize =
  | { kind: 'days'; days: number }
  | { kind: 'balance'; amount: Money }
  | { kind: 'promo'; percent: number; valid_days: number }
  | { kind: 'custom'; title: string; description: string | null }
  | { kind: 'tiered'; tiers: PrizeTier[] }
  | { kind: 'empty' };

export type PrizeTier = { from_place: number; to_place: number; prize: Prize };

export type PrizeKind = 'days' | 'balance' | 'promo' | 'custom';

export type PrizeMode = 'single' | 'tiered';

export type Status = 'scheduled' | 'running' | 'drawing' | 'finished' | 'canceled';

export type Audience = 'outsiders' | 'participants' | 'results';

export type Counts = { entries: number; excluded: number; issued: number; waiting: number; failed: number; pending: number };

export type Conditions = {
  subscription?: { paid_only: boolean };
  purchase?: { from: string | null; until: string | null; min: Money | null };
  friends?: { count: number; paid: boolean };
};

export type ConditionLine = { key: string; text: string };

export type Progress = { total: number; sent: number; failed: number; blocked: number; skipped: number; pending: number };

export type MessageView = {
  id: number;
  audience: Audience;
  audience_title: string;
  only_eligible: boolean;
  send_at: string | null;
  text: string;
  state: 'planned' | 'queued' | 'skipped';
  queued_at: string | null;
  recipients: number | null;
  skipped: string | null;
  progress: Progress | null;
};

export type Person = {
  client_id: number;
  name: string | null;
  telegram_username: string | null;
  telegram_id: string | null;
  email: string | null;
};

export type PrizeState = 'pending' | 'waiting' | 'issuing' | 'issued' | 'failed';

export type Winner = {
  place: number;
  client_id: number;
  client: Person | null;
  prize_label: string | null;
  prize_state: PrizeState | null;
  prize_note: string | null;
  prize_code: string | null;
  prize_error: string | null;
  prize_at: string | null;
  ticket: string | null;
};

export type LogItem = { id: number; kind: string; text: string; at: string };

export type Contest = {
  id: number;
  title: string;
  description: string | null;
  prize: Prize;
  prize_label: string;
  winners: number;
  conditions: Conditions;
  condition_lines: ConditionLine[];
  starts_at: string;
  ends_at: string;
  status: Status;
  status_title: string;
  seed_hash: string;
  seed: string | null;
  list_hash: string | null;
  entries_drawn: number | null;
  drawn_at: string | null;
  canceled_at: string | null;
  created_at: string;
  counts: Counts;
  messages: MessageView[];
  winners_list: Winner[];
  log: LogItem[];
  editable: { all: boolean; texts: boolean };
};

export type ListItem = {
  id: number;
  title: string;
  prize: Prize;
  prize_label: string;
  winners: number;
  starts_at: string;
  ends_at: string;
  status: Status;
  status_title: string;
  counts: Counts;
};

export type Page<T> = { items: T[]; next: number | null };

export type Entry = {
  client_id: number;
  client: Person | null;
  joined_at: string;
  excluded: string | null;
  ticket: string | null;
  place: number | null;
  prize_state: PrizeState | null;
  prize_note: string | null;
};

export type Options = {
  currency: Currency;
  promo_enabled: boolean;
  referral_enabled: boolean;
  broadcast_enabled: boolean;
  winners_max: number;
  messages_max: number;
  friends_max: number;
  title_max: number;
  description_max: number;
};

export type ListFilter = 'all' | 'live' | 'done';

export const FILTERS: Array<[ListFilter, string]> = [
  ['all', 'Все'],
  ['live', 'Идут'],
  ['done', 'Завершены'],
];

export const PRIZE_KINDS: Array<[PrizeKind, string]> = [
  ['days', 'Дни подписки'],
  ['balance', 'Деньги на баланс'],
  ['promo', 'Промокод на скидку'],
  ['custom', 'Свой приз'],
];

export const AUDIENCES: Array<[Audience, string]> = [
  ['outsiders', 'Тем, кто не участвует'],
  ['participants', 'Участникам'],
  ['results', 'Итоги — участникам без приза'],
];

export function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function disabledModule(err: unknown): boolean {
  return err instanceof ApiError && err.code === 'module.disabled';
}

export function personName(person: Person | null, id: number): string {
  if (person?.name) {
    return person.name;
  }
  if (person?.telegram_username) {
    return `@${person.telegram_username}`;
  }
  if (person?.email) {
    return person.email;
  }
  return `Клиент #${id}`;
}

export function personSub(person: Person | null, id: number): string {
  if (person?.name && person.telegram_username) {
    return `@${person.telegram_username}`;
  }
  if (person?.email && person.name) {
    return person.email;
  }
  return `#${id}`;
}

export function initials(person: Person | null): string {
  const name = person?.name ?? person?.telegram_username ?? '';
  const parts = name.replace('@', '').split(/\s+/).filter(Boolean);
  const letters = parts.slice(0, 2).map((p) => p[0]?.toUpperCase() ?? '');
  return letters.join('') || '#';
}

export function when(iso: string | null | undefined): string {
  if (!iso) {
    return '';
  }
  return `${dateLong(iso)}, ${timeHM(iso)}`;
}

export function period(item: { starts_at: string; ends_at: string }): string {
  return `${dateLong(item.starts_at)} — ${dateLong(item.ends_at)}, ${timeHM(item.ends_at)}`;
}

export function statusTone(status: Status): string {
  switch (status) {
    case 'running':
      return 'ok';
    case 'drawing':
      return 'warn';
    case 'canceled':
      return 'muted';
    default:
      return '';
  }
}

export function placeLabel(from: number, to: number): string {
  const medal = from === 1 && to === 1 ? '🥇 ' : from === 2 && to === 2 ? '🥈 ' : from === 3 && to === 3 ? '🥉 ' : '';
  if (from === to) {
    return `${medal}${from} ${plural(from, 'место', 'места', 'мест')}`;
  }
  return `${medal}${from}–${to} ${plural(to, 'место', 'места', 'мест')}`;
}

export function prizeEach(prize: Prize, winners: number): string {
  if (prize.kind === 'tiered') {
    return prize.tiers.map((t) => `${placeLabel(t.from_place, t.to_place)} — ${prizeText(t.prize)}`).join(' · ');
  }
  const label = prizeText(prize);
  return winners > 1 ? `${label} — каждому из ${count(winners, 'победителя', 'победителей', 'победителей')}` : `${label} — одному победителю`;
}

export function prizeText(prize: Prize): string {
  switch (prize.kind) {
    case 'days':
      return `+${days(prize.days)} к подписке`;
    case 'balance':
      return `${money(prize.amount)} на баланс`;
    case 'promo':
      return `Промокод на скидку ${prize.percent}%, действует ${days(prize.valid_days)}`;
    case 'custom':
      return prize.title;
    case 'tiered':
      return prize.tiers.map((t) => `${placeLabel(t.from_place, t.to_place)} — ${prizeText(t.prize)}`).join(' · ');
    default:
      return 'Без приза';
  }
}

export function prizeStateText(w: { prize_state: PrizeState | null; prize_note: string | null; prize_error: string | null; prize_at: string | null }): { text: string; tone: string } {
  switch (w.prize_state) {
    case 'issued':
      return { text: w.prize_at ? `Выдан ${when(w.prize_at)}` : 'Выдан', tone: 'ok' };
    case 'waiting':
      return { text: 'Ждёт подписку: дни добавятся, когда она появится', tone: 'warn' };
    case 'failed':
      return { text: w.prize_error ?? 'Не выдан', tone: 'bad' };
    case 'issuing':
      return { text: 'Выдаётся', tone: 'warn' };
    case 'pending':
      return { text: 'В очереди на выдачу', tone: 'warn' };
    default:
      return { text: '', tone: '' };
  }
}

export function deliveryText(m: MessageView): string {
  if (m.state === 'planned') {
    return m.send_at ? `Уйдёт ${when(m.send_at)}` : 'Уйдёт после итогов';
  }
  if (m.state === 'skipped') {
    return m.skipped ?? 'Не отправлено';
  }
  const p = m.progress;
  if (!p || p.total === 0) {
    return m.recipients === 0 ? 'Получателей не нашлось' : 'В очереди';
  }
  const parts = [`доставлено ${p.sent} из ${p.total}`];
  if (p.pending) {
    parts.push(`ждут ${p.pending}`);
  }
  if (p.blocked) {
    parts.push(`бот остановлен у ${p.blocked}`);
  }
  if (p.failed) {
    parts.push(`не дошло ${p.failed}`);
  }
  return parts.join(', ');
}

export function shortHash(hash: string | null | undefined): string {
  if (!hash) {
    return '';
  }
  return hash.length > 12 ? `${hash.slice(0, 8)}…${hash.slice(-4)}` : hash;
}

export function groups(hash: string): string {
  return hash.replace(/(.{4})/g, `$1${NBSP}`).trim();
}

export type MessageDraft = { key: string; id: number | null; audience: Audience; onlyEligible: boolean; sendAt: string; text: string; locked: boolean };

export type PrizeFields = {
  prizeKind: PrizeKind;
  days: string;
  amount: string;
  percent: string;
  validDays: string;
  customTitle: string;
  customDescription: string;
};

export type TierDraft = { key: string; fromPlace: string; toPlace: string } & PrizeFields;

export type Draft = {
  title: string;
  description: string;
  prizeMode: PrizeMode;
  tiers: TierDraft[];
} & PrizeFields & {
  winners: string;
  startNow: boolean;
  startsAt: string;
  endsAt: string;
  subscription: boolean;
  paidOnly: boolean;
  purchase: boolean;
  purchaseMin: string;
  friends: string;
  friendsPaid: boolean;
  messages: MessageDraft[] | null;
};

export type DraftErrors = Partial<Record<'title' | 'description' | 'value' | 'tiers' | 'winners' | 'startsAt' | 'endsAt' | 'purchaseMin' | 'friends' | 'messages', string>>;

function pad(n: number): string {
  return String(n).padStart(2, '0');
}

export function localInput(iso: string | null | undefined): string {
  if (!iso) {
    return '';
  }
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) {
    return '';
  }
  return `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(d.getHours())}:${pad(d.getMinutes())}`;
}

export function fromLocal(value: string): string | null {
  if (!value) {
    return null;
  }
  const d = new Date(value);
  return Number.isNaN(d.getTime()) ? null : d.toISOString();
}

export function emptyDraft(): Draft {
  const end = new Date();
  end.setDate(end.getDate() + 7);
  end.setHours(20, 0, 0, 0);
  return {
    title: '',
    description: '',
    prizeMode: 'single',
    tiers: [],
    prizeKind: 'days',
    days: '30',
    amount: '',
    percent: '20',
    validDays: '14',
    customTitle: '',
    customDescription: '',
    winners: '3',
    startNow: true,
    startsAt: '',
    endsAt: localInput(end.toISOString()),
    subscription: false,
    paidOnly: false,
    purchase: false,
    purchaseMin: '',
    friends: '',
    friendsPaid: true,
    messages: null,
  };
}

let seq = 0;

export function messageKey(): string {
  seq += 1;
  return `m${seq}`;
}

function units(m: Money | null | undefined): string {
  return m ? (m.currency === 'RUB' ? String(Math.round(m.minor / 100)) : (m.minor / 100).toFixed(2).replace(/\.00$/, '')) : '';
}

function prizeFields(prize: Prize): PrizeFields {
  return {
    prizeKind: prize.kind === 'empty' || prize.kind === 'tiered' ? 'days' : prize.kind,
    days: prize.kind === 'days' ? String(prize.days) : '30',
    amount: prize.kind === 'balance' ? units(prize.amount) : '',
    percent: prize.kind === 'promo' ? String(prize.percent) : '20',
    validDays: prize.kind === 'promo' ? String(prize.valid_days) : '14',
    customTitle: prize.kind === 'custom' ? prize.title : '',
    customDescription: prize.kind === 'custom' ? (prize.description ?? '') : '',
  };
}

export function draftOf(c: Contest): Draft {
  const tiered = c.prize.kind === 'tiered' ? c.prize : null;
  const single = tiered ? (tiered.tiers[0]?.prize ?? { kind: 'empty' as const }) : c.prize;
  return {
    title: c.title,
    description: c.description ?? '',
    prizeMode: tiered ? 'tiered' : 'single',
    tiers: tiered
      ? tiered.tiers.map((t) => ({
          key: messageKey(),
          fromPlace: String(t.from_place),
          toPlace: String(t.to_place),
          ...prizeFields(t.prize),
        }))
      : [],
    ...prizeFields(single),
    winners: String(c.winners),
    startNow: false,
    startsAt: localInput(c.starts_at),
    endsAt: localInput(c.ends_at),
    subscription: !!c.conditions.subscription,
    paidOnly: c.conditions.subscription?.paid_only ?? false,
    purchase: !!c.conditions.purchase,
    purchaseMin: units(c.conditions.purchase?.min ?? null),
    friends: c.conditions.friends ? String(c.conditions.friends.count) : '',
    friendsPaid: c.conditions.friends?.paid ?? true,
    messages: c.messages.map((m) => ({
      key: messageKey(),
      id: m.id,
      audience: m.audience,
      onlyEligible: m.only_eligible,
      sendAt: localInput(m.send_at),
      text: m.text,
      locked: m.state !== 'planned',
    })),
  };
}

function whole(raw: string): number | null {
  const t = raw.trim();
  if (!/^\d+$/.test(t)) {
    return null;
  }
  return Number(t);
}

function amountOk(raw: string, currency: Currency): boolean {
  const t = raw.trim().replace(',', '.');
  return currency === 'RUB' ? /^\d+$/.test(t) && Number(t) > 0 : /^\d+(\.\d{1,2})?$/.test(t) && Number(t) > 0;
}

function buildPrize(fields: PrizeFields, currency: Currency): { value: unknown; error?: string } {
  if (fields.prizeKind === 'days') {
    const n = whole(fields.days);
    if (n === null || n < 1 || n > 365) {
      return { value: { kind: 'days', days: n ?? 0 }, error: 'От 1 до 365 дней' };
    }
    return { value: { kind: 'days', days: n } };
  }
  if (fields.prizeKind === 'balance') {
    if (!amountOk(fields.amount, currency)) {
      return {
        value: { kind: 'balance', amount: fields.amount.trim().replace(',', '.') },
        error: currency === 'RUB' ? 'Целое число рублей' : 'Сумма вида 5 или 2.50',
      };
    }
    return { value: { kind: 'balance', amount: fields.amount.trim().replace(',', '.') } };
  }
  if (fields.prizeKind === 'promo') {
    const p = whole(fields.percent);
    const v = whole(fields.validDays);
    if (p === null || p < 1 || p > 100) {
      return { value: { kind: 'promo', percent: p ?? 0, valid_days: v ?? 0 }, error: 'Скидка от 1 до 100 %' };
    }
    if (v === null || v < 1 || v > 365) {
      return { value: { kind: 'promo', percent: p, valid_days: v ?? 0 }, error: 'Срок промокода — от 1 до 365 дней' };
    }
    return { value: { kind: 'promo', percent: p, valid_days: v } };
  }
  const customTitle = fields.customTitle.trim();
  const customDescription = fields.customDescription.trim();
  const value = { kind: 'custom', title: customTitle, description: customDescription || null };
  if (!customTitle) {
    return { value, error: 'Напишите название приза' };
  }
  if (customTitle.length > 100) {
    return { value, error: 'Название приза — до 100 знаков' };
  }
  if (customDescription.length > 500) {
    return { value, error: 'Инструкция — до 500 знаков' };
  }
  return { value };
}

export function buildBody(draft: Draft, currency: Currency, options: Options | null): { body: unknown; errors: DraftErrors } {
  const errors: DraftErrors = {};
  const title = draft.title.trim();
  if (!title) {
    errors.title = 'Напишите название';
  }
  const max = options?.winners_max ?? 100;
  let prize: unknown = null;
  let winnersValue = 0;
  if (draft.prizeMode === 'tiered') {
    if (!draft.tiers.length) {
      errors.tiers = 'Добавьте хотя бы одно призовое место';
    }
    const sorted = [...draft.tiers].sort((a, b) => (whole(a.fromPlace) ?? 0) - (whole(b.fromPlace) ?? 0));
    const tiers: unknown[] = [];
    let firstError: string | undefined;
    let previous: number | null = null;
    for (const tier of sorted) {
      const from = whole(tier.fromPlace);
      const to = whole(tier.toPlace);
      if (from === null || to === null || from < 1 || to < from || to > max) {
        firstError ??= `Места — от 1 до ${max}, «с» не больше «по»`;
      } else if (previous === null ? from !== 1 : from !== previous + 1) {
        firstError ??= 'Призовые места должны идти подряд и начинаться с 1-го';
      } else {
        previous = to;
      }
      const built = buildPrize(tier, currency);
      if (built.error) {
        firstError ??= `${placeLabel(from ?? 1, to ?? from ?? 1)}: ${built.error}`;
      }
      if (from !== null && to !== null) {
        tiers.push({ from_place: from, to_place: to, prize: built.value });
        winnersValue = Math.max(winnersValue, to);
      }
    }
    if (firstError) {
      errors.tiers = firstError;
    } else if (!(winnersValue >= 1 && winnersValue <= max)) {
      errors.tiers = `Места — от 1 до ${max}`;
    }
    prize = { kind: 'tiered', tiers };
  } else {
    const built = buildPrize(draft, currency);
    if (built.error) {
      errors.value = built.error;
    }
    prize = built.value;
    const winners = whole(draft.winners);
    if (winners === null || winners < 1 || winners > max) {
      errors.winners = `От 1 до ${max}`;
    }
    winnersValue = winners ?? 0;
  }
  const startsAt = draft.startNow ? null : fromLocal(draft.startsAt);
  if (!draft.startNow && !startsAt) {
    errors.startsAt = 'Укажите дату и время';
  }
  const endsAt = fromLocal(draft.endsAt);
  if (!endsAt) {
    errors.endsAt = 'Укажите дату и время итогов';
  } else if (new Date(endsAt).getTime() <= (startsAt ? new Date(startsAt).getTime() : Date.now())) {
    errors.endsAt = 'Итоги должны быть позже начала';
  }
  if (draft.purchase && draft.purchaseMin.trim() && !amountOk(draft.purchaseMin, currency)) {
    errors.purchaseMin = currency === 'RUB' ? 'Целое число рублей' : 'Сумма вида 5 или 2.50';
  }
  const friends = draft.friends.trim() ? whole(draft.friends) : 0;
  if (friends === null || friends > (options?.friends_max ?? 100)) {
    errors.friends = `От 0 до ${options?.friends_max ?? 100}`;
  }
  let messages: unknown = undefined;
  if (draft.messages) {
    const bad = draft.messages.findIndex((m) => !m.locked && (!m.text.trim() || (m.audience !== 'results' && !fromLocal(m.sendAt))));
    if (bad >= 0) {
      errors.messages = `Сообщение ${bad + 1}: заполните время и текст`;
    }
    messages = draft.messages
      .filter((m) => !m.locked)
      .map((m) => ({
        id: m.id,
        audience: m.audience,
        only_eligible: m.audience === 'outsiders' && m.onlyEligible,
        send_at: m.audience === 'results' ? null : fromLocal(m.sendAt),
        text: m.text,
      }));
  }
  return {
    errors,
    body: {
      title,
      description: draft.description.trim() || null,
      prize,
      winners: winnersValue,
      starts_at: startsAt,
      ends_at: endsAt,
      conditions: {
        subscription: draft.subscription,
        paid_only: draft.subscription && draft.paidOnly,
        purchase: draft.purchase,
        purchase_min: draft.purchase ? draft.purchaseMin.trim() || null : null,
        friends: friends ?? 0,
        friends_paid: draft.friendsPaid,
      },
      ...(messages === undefined ? {} : { messages }),
    },
  };
}

export const PLACEHOLDERS: Array<[string, string]> = [
  ['{title}', 'название'],
  ['{prize}', 'приз'],
  ['{winners}', 'победителей'],
  ['{date}', 'когда итоги'],
];
