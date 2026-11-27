import { count, dateLong, money, moneyExact, NBSP, plural, timeHM, type Money } from '../../lib/format';
import { amountText, lowerFirst, methodText } from '../client/history';
import type { Amount, OrderBrief } from '../client/types';
import type { Attention } from '../money/journal';
import { START_VIA_TEXT, startMark, startVia } from '../../lib/utmTags';
import { termText } from '../tariffs/model';
import type { Decisions } from './todo';
import type { DayStat } from './weekStats';

export type NodeState = 'online' | 'connecting' | 'down' | 'disabled';

export type NodeBrief = {
  uuid: string;
  name: string;
  country_code: string;
  state: NodeState;
  users_online: number;
  traffic_alert: boolean;
  traffic_used_bytes: number | null;
  traffic_limit_bytes: number | null;
  traffic_reset_day: number | null;
  last_status_change: string | null;
  last_status_message: string | null;
  down_since: string | null;
};

export type Dashboard = {
  currency: 'USD' | 'RUB';
  subscriptions: { active_paid: number; trials: number; expiring_soon: number; never_connected: number; limited: number } | null;
  today: { payments: number | null; new_clients: number; new_clients_paid: number | null } | null;
  revenue: { today: Money; yesterday: Money; yesterday_same_time?: Money; week: Money; week_before?: Money; month: Money; today_stars: number; by_day: Array<{ day: string; amount: Money }> } | null;
  days?: DayStat[] | null;
  support_waiting: number;
  webhooks?: Attention | null;
  decisions?: Decisions;
  remna: { online_now: number; users_total: number; nodes_total: number; nodes_online: number; nodes: NodeBrief[] } | null;
  remna_error: string | null;
  server?: ServerLoad | null;
  setup?: Setup;
};

export type Setup = { remnawave: boolean; bot: boolean; tariffs: boolean };

export type Usage = { used_bytes: number; total_bytes: number };

export type ServerLoad = {
  cpu_percent: number | null;
  cores: number | null;
  load: [number, number, number] | null;
  memory: Usage | null;
  disk: Usage | null;
  uptime_seconds: number | null;
  app_memory_bytes: number | null;
  database_bytes: number | null;
};

export type Person = { client_id: number; name: string | null; telegram_username: string | null; telegram_id: string | null; email: string | null };

export type FeedItem = {
  source: 'remnawave' | 'core';
  id: number;
  at: string;
  kind: string;
  title: string;
  client_id: number | null;
  subject: string | null;
  data: Record<string, unknown> | null;
  client: Person | null;
  order: OrderBrief | null;
  payment: { id: string; status: string; amount: Amount; provider: string; method: string | null } | null;
  ticket: { id: number; subject: string | null; status: string; state?: 'waiting' | 'answered' | 'closed' } | null;
};

export type FeedPage = { items: FeedItem[]; remna_before: number | null; core_before: number | null; has_more: boolean };

export type TicketItem = {
  ticket: { id: number; client_id: number; subject: string | null; status: string; created_at: string; updated_at: string };
  client_name: string | null;
  telegram_username: string | null;
  email: string | null;
  waiting_since: string | null;
  last_message_at: string | null;
  preview: string | null;
};

export type PaymentItem = {
  id: number;
  public_id: string;
  client_id: number;
  provider: string;
  method: string | null;
  amount: Amount;
  status: string;
  paid_at: string | null;
  expires_at: string | null;
  created_at: string;
  client: Person | null;
  order: OrderBrief | null;
  failure_reason?: string | null;
};

export type BillingNode = {
  uuid: string;
  name: string | null;
  provider: { uuid: string; name: string; loginUrl?: string | null };
  node: { uuid: string; name: string; countryCode?: string } | null;
  nextBillingAt: string;
};

export function personName(person: Person | null, fallbackId?: number | null): string {
  if (person?.name) {
    return person.name;
  }
  if (person?.telegram_id) {
    return `id ${person.telegram_id}`;
  }
  if (person?.email) {
    return person.email;
  }
  return fallbackId ? `Клиент ${fallbackId}` : 'Клиент';
}

export { countryName } from '../../lib/geo';

export function minutesSince(iso: string | null, now = Date.now()): number | null {
  if (!iso) {
    return null;
  }
  const at = Date.parse(iso);
  return Number.isNaN(at) ? null : Math.max(0, Math.floor((now - at) / 60_000));
}

export function spanText(minutes: number): string {
  if (minutes < 60) {
    return `${minutes}${NBSP}мин`;
  }
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  if (hours < 24) {
    return rest ? `${hours}${NBSP}ч ${rest}${NBSP}мин` : `${hours}${NBSP}ч`;
  }
  const daysCount = Math.floor(hours / 24);
  return `${daysCount}${NBSP}д`;
}

export function shortAgo(iso: string, now = Date.now()): string {
  const minutes = minutesSince(iso, now) ?? 0;
  if (minutes < 1) {
    return 'только что';
  }
  if (minutes < 60) {
    return `${minutes}${NBSP}мин`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}${NBSP}ч`;
  }
  return dateLong(iso);
}

export function agoLong(iso: string | null): string {
  const minutes = minutesSince(iso);
  if (minutes === null) {
    return '';
  }
  if (minutes < 1) {
    return 'только что';
  }
  if (minutes < 60) {
    return `${minutes}${NBSP}мин назад`;
  }
  const hours = Math.floor(minutes / 60);
  if (hours < 24) {
    return `${hours}${NBSP}ч назад`;
  }
  return `${dateLong(iso)}, ${timeHM(iso)}`;
}

export function nodeStateText(node: NodeBrief, capital: boolean): { text: string; tone: 'ok' | 'bad' | 'warn' | 'muted' } {
  const cap = (s: string) => (capital ? s.charAt(0).toUpperCase() + s.slice(1) : s);
  switch (node.state) {
    case 'online':
      return { text: `${node.users_online.toLocaleString('ru-RU').replace(/\s/g, NBSP)}${NBSP}онлайн`, tone: 'ok' };
    case 'down': {
      const minutes = minutesSince(node.down_since ?? node.last_status_change);
      return { text: cap(minutes === null ? 'не отвечает' : `не отвечает ${spanText(minutes)}`), tone: 'bad' };
    }
    case 'connecting':
      return { text: cap('подключается'), tone: 'warn' };
    default:
      return { text: cap('выключена'), tone: 'muted' };
  }
}

export function trafficAlert(node: NodeBrief): { text: string; title: string } | null {
  if (!node.traffic_alert || !node.traffic_limit_bytes || node.traffic_used_bytes === null) {
    return null;
  }
  const percent = Math.round((node.traffic_used_bytes / node.traffic_limit_bytes) * 100);
  const tb = 1024 ** 4;
  const fmt = (v: number) => (v / tb).toFixed(1).replace('.', ',');
  let reset = '';
  if (node.traffic_reset_day) {
    const now = new Date();
    const day = node.traffic_reset_day;
    const candidate = new Date(now.getFullYear(), now.getMonth(), day);
    const next = candidate > now ? candidate : new Date(now.getFullYear(), now.getMonth() + 1, day);
    reset = `, сброс ${dateLong(next.toISOString())}`;
  }
  return { text: `трафик ${percent}%`, title: `${fmt(node.traffic_used_bytes)} из ${fmt(node.traffic_limit_bytes)} ТБ${reset}` };
}

export function nodesSummary(nodes: NodeBrief[]): { text: string; tone: 'bad' | 'muted' } {
  const down = nodes.filter((n) => n.state === 'down').length;
  const connecting = nodes.filter((n) => n.state === 'connecting').length;
  if (down > 0) {
    return { text: down === 1 ? 'Одна нода не отвечает' : `${count(down, 'нода', 'ноды', 'нод')} не отвечают`, tone: 'bad' };
  }
  if (connecting > 0) {
    return { text: connecting === 1 ? 'Одна нода подключается' : `${count(connecting, 'нода', 'ноды', 'нод')} подключаются`, tone: 'muted' };
  }
  return { text: 'Все ноды в сети', tone: 'muted' };
}

export function nodesHeader(nodes: NodeBrief[]): string {
  const disabled = nodes.filter((n) => n.state === 'disabled').length;
  const online = nodes.filter((n) => n.state === 'online').length;
  const base = `${online} из ${nodes.length - disabled} в сети`;
  return disabled ? `${base} · ${disabled}${NBSP}${plural(disabled, 'выключена', 'выключены', 'выключены')}` : base;
}

export function orderLine(order: OrderBrief | null): string {
  if (!order) {
    return 'Заказ';
  }
  if (order.kind === 'top_up') {
    return 'Пополнение баланса';
  }
  if (order.kind === 'module') {
    return order.label ?? 'Дополнение';
  }
  return [order.tariff_name ?? 'Подписка', order.days ? termText(order.days) : null].filter(Boolean).join(' · ');
}

export type FeedLine = {
  key: string;
  who: string;
  what: string;
  note: string;
  tone: 0 | 1 | 2;
  icon: 'person' | 'server' | 'users' | 'wallet';
  clientId: number | null;
  action: { label: string; aria: string; kind: 'restart' | 'reply' | 'open' | 'client'; target: string } | null;
  when: string;
};

function bought(order: OrderBrief | null): string {
  if (!order) {
    return 'оплата заказа';
  }
  switch (order.kind) {
    case 'top_up':
      return 'пополнение баланса';
    case 'change':
      return order.tariff_name ? `переход на «${order.tariff_name}»` : 'смена тарифа';
    case 'module':
      return order.label ? `оплата: ${order.label}` : 'оплата дополнения';
    default: {
      const span = order.days ? termText(order.days) : '';
      const name = order.tariff_name ? `«${order.tariff_name}»` : 'подписки';
      return span ? `оплата ${name} на ${span}` : `оплата ${name}`;
    }
  }
}

const SOURCE: Record<string, string> = {
  bot: 'новый клиент из бота',
  miniapp: 'новый клиент из мини-приложения',
  web: 'новый клиент из кабинета',
  admin: 'клиент добавлен администратором',
  import: 'клиент перенесён из прежней панели',
  migration: 'клиент перенесён из прежней панели',
};

function startNote(raw: unknown): string {
  const mark = startMark(raw);
  if (mark) {
    return `Метка ${mark}`;
  }
  const via = startVia(raw);
  return via ? START_VIA_TEXT[via] : '';
}

export function feedLine(item: FeedItem, nodes: NodeBrief[]): FeedLine | null {
  const base = { key: `${item.source}-${item.id}`, when: shortAgo(item.at), clientId: item.client_id };
  const who = personName(item.client, item.client_id);
  if (item.source === 'core') {
    switch (item.kind) {
      case 'payment_paid': {
        const amount = item.payment ? amountText(item.payment.amount) : '';
        const via = item.payment ? lowerFirst(methodText(item.payment.provider, item.payment.method)) : '';
        return { ...base, who, what: bought(item.order), note: [amount, via].filter(Boolean).join(' · '), tone: 0, icon: 'person', action: null };
      }
      case 'payment_refunded': {
        const amount = item.data?.amount as Amount | undefined;
        return { ...base, who, what: 'возврат', note: amount ? amountText(amount) : '', tone: 1, icon: 'person', action: null };
      }
      case 'client_registered':
        return {
          ...base,
          who,
          what: SOURCE[String(item.data?.source ?? '')] ?? 'новый клиент',
          note: startNote(item.data?.start_param),
          tone: 0,
          icon: 'person',
          action: null,
        };
      case 'client_blocked':
        return { ...base, who, what: 'блокировка', note: item.data?.reason ? String(item.data.reason) : '', tone: 2, icon: 'person', action: null };
      case 'ticket_opened':
        return {
          ...base,
          who,
          what: 'обращение в поддержку',
          note: item.ticket?.subject ? `«${item.ticket.subject}»` : '',
          tone: 0,
          icon: 'person',
          action: item.ticket?.state === 'waiting' ? { label: 'Ответить', aria: `Ответить: ${who}`, kind: 'reply', target: String(item.ticket.id) } : null,
        };
      case 'ticket_answered':
        return { ...base, who, what: 'ответ поддержки', note: item.ticket?.subject ? `«${item.ticket.subject}»` : '', tone: 0, icon: 'person', action: null };
      case 'balance_changed': {
        const delta = item.data?.delta as Money | undefined;
        const kind = String(item.data?.kind);
        const what = kind === 'topup_bonus' ? 'бонус за пополнение' : kind === 'topup_bonus_reversal' ? 'бонус за пополнение снят' : 'изменение баланса';
        return { ...base, who, what, note: delta ? (kind.startsWith('topup_bonus') ? moneyExact(delta) : money(delta)) : '', tone: 0, icon: 'wallet', action: null };
      }
      default:
        return { ...base, who, what: item.title, note: '', tone: 0, icon: 'person', action: null };
    }
  }
  const data = item.data ?? {};
  if (item.kind.startsWith('node.')) {
    const name = typeof data.name === 'string' ? data.name : '';
    const node = nodes.find((n) => n.uuid === item.subject || n.name === name);
    const title = item.title;
    const nodeWho = title.startsWith('Нода ') ? title.split(' ').slice(0, 2).join(' ') : title;
    const rest = title.startsWith('Нода ') ? title.split(' ').slice(2).join(' ') : '';
    if (item.kind === 'node.connection_lost') {
      const message = typeof data.lastStatusMessage === 'string' ? data.lastStatusMessage : '';
      const since = `С ${timeHM(item.at)}`;
      return {
        ...base,
        clientId: null,
        who: nodeWho,
        what: rest,
        note: [since, message].filter(Boolean).join(' · '),
        tone: 2,
        icon: 'server',
        action: node && node.state === 'down' ? { label: 'Перезапустить', aria: `Перезапустить ${node.name}`, kind: 'restart', target: node.uuid } : null,
      };
    }
    return { ...base, clientId: null, who: nodeWho, what: rest, note: '', tone: item.kind === 'node.traffic_notify' ? 1 : 0, icon: 'server', action: null };
  }
  if (item.kind.startsWith('crm.')) {
    return { ...base, clientId: null, who: item.title, what: '', note: '', tone: 1, icon: 'server', action: null };
  }
  return { ...base, clientId: null, who: item.title, what: '', note: '', tone: 0, icon: 'users', action: null };
}

export function weekdayLine(date = new Date()): string {
  const weekday = date.toLocaleDateString('ru-RU', { weekday: 'long' });
  return `${weekday.charAt(0).toUpperCase()}${weekday.slice(1)}, ${dateLong(date.toISOString(), false)}`;
}
