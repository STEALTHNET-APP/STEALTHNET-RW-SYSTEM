import type { Money } from '../../lib/format';

export type ClientStatus = 'active' | 'expiring' | 'trial' | 'limited' | 'expired' | 'blocked' | 'no_subscription';

export type Mirror = {
  status: 'active' | 'limited' | 'expired' | 'disabled' | null;
  expire_at: string | null;
  traffic_used_bytes: number | null;
  traffic_limit_bytes: number | null;
  traffic_strategy: string | null;
  hwid_limit: number | null;
  devices_count: number | null;
  online_at: string | null;
  first_connected_at: string | null;
  last_node: string | null;
};

export type Subscription = {
  id: number;
  public_id: string;
  client_id: number;
  tariff_id: number | null;
  kind: 'paid' | 'trial' | 'gift' | 'promo' | 'manual' | 'custom';
  remna: { user_id: number; username: string; short_uuid: string | null } | null;
  extra_devices: number;
  period_started_at: string | null;
  period_days: number | null;
  autorenew: { enabled: boolean; period_id: number | null };
  mirror: Mirror;
  created_at: string;
  ended_at: string | null;
  device_limit?: number | null;
};

export type Client = {
  id: number;
  public_id: string;
  status: 'active' | 'blocked';
  block_reason: string | null;
  note: string | null;
  registration_source: string;
  bot_unreachable: boolean;
  created_at: string;
  blocked_at: string | null;
};

export type ClientRow = {
  client: Client;
  name: string | null;
  telegram_id: string | null;
  telegram_username: string | null;
  email: string | null;
  status: ClientStatus;
  subscription: Subscription | null;
  tariff_name: string | null;
  balance: Money | null;
  paid_total: Money | null;
};

export type Segment = 'all' | 'active' | 'expiring_soon' | 'trial' | 'never_connected' | 'expired' | 'limited' | 'blocked' | 'no_subscription';

export type SegmentCounts = {
  total: number;
  active: number;
  trial: number;
  no_subscription: number;
  expiring_soon: number;
  never_connected: number;
  expired: number;
  limited: number;
  blocked: number;
};

export function displayName(row: { name: string | null; telegram_id: string | null; client: { id: number } }): string {
  if (row.name && row.name.trim()) {
    return row.name.trim();
  }
  if (row.telegram_id) {
    return `id ${row.telegram_id}`;
  }
  return `Клиент ${row.client.id}`;
}

export const STATUS_WORD: Record<ClientStatus, [string, string]> = {
  active: ['Активна', 'var(--ink-2)'],
  expiring: ['Истекает', 'var(--warn)'],
  trial: ['Пробный', 'var(--ink-2)'],
  limited: ['Трафик исчерпан', 'var(--warn)'],
  expired: ['Истекла', 'var(--muted)'],
  blocked: ['Заблокирован', 'var(--bad)'],
  no_subscription: ['Нет подписки', 'var(--muted)'],
};
