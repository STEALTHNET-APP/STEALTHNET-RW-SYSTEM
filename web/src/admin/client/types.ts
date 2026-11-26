import type { Money } from '../../lib/format';
import type { Client, Renewal, Subscription } from '../clients/types';

export type Identity = {
  id: number;
  client_id: number;
  kind: 'telegram' | 'email' | 'google' | 'apple';
  value: string;
  verified: boolean;
  meta: {
    username?: string | null;
    first_name?: string | null;
    last_name?: string | null;
    name?: string | null;
    email?: string | null;
    private_email?: boolean;
  } | null;
};

export type TrafficReset = 'no_reset' | 'day' | 'week' | 'month' | 'month_rolling' | 'on_payment';

export type TariffBrief = {
  id: number;
  name: string;
  on_sale: boolean;
  devices: { included: number; extra: { max: number } | null };
  traffic: { limit_bytes: number | null; reset: TrafficReset };
};

export type Person = { client_id: number; name: string | null; telegram_username: string | null };

export type CardStats = {
  paid_total?: Money;
  payments_count?: number;
  referrals_count: number;
  referrer: Person | null;
  cabinet_seen_at: string | null;
  miniapp_seen_at: string | null;
};

export type ClientCard = {
  client: Client & { locale: 'ru' | 'en' };
  identities: Identity[];
  balance?: Money;
  referral_balance?: Money;
  subscriptions: Subscription[];
  tariffs: TariffBrief[];
  stats: CardStats;
  bot_available: boolean;
  two_factor: { enabled: boolean; enabled_at: string | null };
  multi_subscriptions?: boolean;
};

export type SubscriptionDetails = {
  subscription: Subscription;
  status: 'active' | 'expiring' | 'trial' | 'limited' | 'expired' | 'blocked' | 'no_subscription';
  progress: { days_left: number; days_total: number } | null;
  subscription_url: string | null;
  countries: string[];
  squads: string[];
  last_node: string | null;
  traffic_30d_bytes: number | null;
  remna_reachable: boolean;
  remna_missing_at: string | null;
  renewal?: Renewal | null;
};

export type Device = {
  hwid: string;
  title: string;
  platform: string | null;
  os_version: string | null;
  app: string | null;
  added_at: string;
};

export type Connection = { node: string; country_code: string; ip: string; last_seen: string };

export type Connections =
  | { state: 'pending'; job_id: string }
  | { state: 'done'; connections: Connection[] }
  | { state: 'failed' };

export type SubRequest = { at: string; response: string; ip: string | null; app: string | null; user_agent: string | null };

export type Amount = { minor: number; currency: 'USD' | 'RUB' | 'XTR' };

export type OrderBrief = {
  id: string;
  kind: 'new' | 'renewal' | 'change' | 'top_up' | 'trial' | 'module' | 'admin_grant' | 'module_grant';
  status: string;
  label: string | null;
  tariff_id: number | null;
  tariff_name: string | null;
  days: number | null;
  months: number | null;
  extra_devices: number;
  total: Money;
};

export type HistoryItem = { at: string } & (
  | {
      type: 'payment';
      payment: {
        id: string;
        number: number;
        status: 'paid' | 'partially_refunded' | 'refunded' | 'failed' | string;
        amount: Amount;
        provider: string;
        method: string | null;
      };
      order: OrderBrief | null;
      review?: string | null;
      failure_reason?: string | null;
    }
  | { type: 'order'; order: OrderBrief }
  | {
      type: 'event';
      kind: string;
      event: Record<string, unknown> & { type: string };
      ticket: { id: number; subject: string | null; status: string; state?: 'waiting' | 'answered' | 'closed' } | null;
    }
  | {
      type: 'audit';
      action: string;
      actor_kind: string;
      actor_id: number | null;
      actor_email: string | null;
      entity_kind: string | null;
      entity_id: number | null;
      details: Record<string, unknown>;
    }
);

export type HistoryPage = { items: HistoryItem[]; next: number | null; total: number };
