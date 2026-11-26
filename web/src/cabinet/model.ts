import type { Renewal, Subscription } from '../admin/clients/types';
import { count, dateLong, days, NBSP, type Money } from '../lib/format';
import { lang, t } from '../lib/i18n';
import type { OfferPromotion, QuotePromotion } from '../lib/promo';
import type { Currency } from '../lib/types';

export type TrafficReset = 'no_reset' | 'day' | 'week' | 'month' | 'month_rolling' | 'on_payment';

export type DeviceDiscount = { from: number; percent: number };

export type TariffBrief = {
  id: number;
  category_id?: number;
  name: string;
  on_sale: boolean;
  devices: { included: number; extra: { max: number; monthly: { usd?: number; rub?: number }; monthly_stars: number | null } | null };
  traffic: { limit_bytes: number | null; reset: TrafficReset };
};

export type SubStatus = 'active' | 'expiring' | 'trial' | 'limited' | 'expired' | 'blocked' | 'no_subscription';

export type SubscriptionView = {
  subscription: Subscription;
  status: SubStatus;
  progress: { days_left: number; days_total: number } | null;
  subscription_url: string | null;
  countries: string[];
  tariff: TariffBrief | null;
  renewal?: Renewal | null;
};

export type ClientMe = {
  client: { id: number; status: 'active' | 'blocked'; created_at: string; locale: string };
  locale?: 'ru' | 'en';
  balance: Money;
  balance_held?: Money;
  balance_enabled?: boolean;
  multi_subscriptions?: boolean;
  promo_codes?: boolean;
  options?: boolean;
  autorenew_card?: boolean;
  referral_balance: Money;
  currency: Currency;
  channel: 'cabinet' | 'miniapp';
  full_access: boolean;
  two_factor: TwoFactor;
  payments_count?: number;
  email?: string | null;
  email_change?: boolean;
  email_unconfirmed?: boolean;
  has_password?: boolean;
  must_change_password?: boolean;
  photo?: string | null;
  support_unread?: number;
};

export function photoUrl(me: ClientMe): string | null {
  return me.photo ? `/api/client/photo?v=${encodeURIComponent(me.photo)}` : null;
}

export type TwoFactor = { enabled: boolean; enabled_at: string | null };

export function heldBalance(me: ClientMe): Money | null {
  return me.balance_held && me.balance_held.minor > 0 ? me.balance_held : null;
}

export type Device = { hwid: string; title: string | null; platform: string | null; os_version: string | null; app: string | null; added_at: string | null };

export type PeriodOffer = {
  period_id: number;
  days: number;
  months: number;
  price: Money;
  per_month: Money;
  savings_percent: number | null;
  stars: number | null;
  stars_per_month: number | null;
  stars_savings_percent: number | null;
  promotion?: OfferPromotion | null;
};

export type TariffOffer = {
  category: { id: number; name: string; sort: number; visible: boolean; subscription_limit?: number | null };
  tariff: TariffBrief & { category_id: number; description: string | null; sort: number; device_discounts?: DeviceDiscount[] };
  periods: PeriodOffer[];
};

export type QuoteLineView = { kind: { type: string }; label: string; amount: Money; stars: number | null };

export function ladderText(discounts: DeviceDiscount[] | undefined): string | null {
  if (!discounts || discounts.length === 0) {
    return null;
  }
  const steps = discounts.map((d) => t('от {n} — {percent}%', { n: d.from, percent: d.percent }).replace(/ /g, NBSP)).join(', ');
  return t('Скидка за доп. устройства: {steps}', { steps });
}

export function linesOf(lines: QuoteLineView[], type: string): QuoteLineView[] {
  return lines.filter((line) => line.kind.type === type);
}

export type Catalog = { currency: Currency; tariffs: TariffOffer[] };

export type CheckoutMethod = {
  provider: string;
  method: string | null;
  kind: 'external' | 'stars' | 'balance';
  title: string;
  hint: string;
  amount: { minor?: number; currency?: Currency; stars?: number } | number | null;
  unavailable: null | { type: 'insufficient_balance'; balance: Money; missing: Money } | { type: 'no_stars_price' };
};

export type Quote = {
  quote: {
    currency: Currency;
    lines: QuoteLineView[];
    subtotal: Money;
    discount: Money;
    total: Money;
    stars: { subtotal: number; discount: number; total: number } | null;
    days: number;
    extra_devices: number;
  };
  kind: 'new' | 'renewal' | 'change';
  expire_preview: string | null;
  methods: CheckoutMethod[];
  current?: { subscription_id: number; tariff: string | null; trial: boolean; expire_at: string | null } | null;
  promotion?: QuotePromotion | null;
};

export function transitionText(current: string | null): string {
  return t('Текущая подписка ({current}) будет удалена, новая станет доступна сразу после оплаты. Оставшиеся дни не переносятся, ссылка подписки останется прежней.', {
    current: current ?? t('без тарифа'),
  });
}

export type WithdrawalStatus = 'holding' | 'pending' | 'paid' | 'rejected' | 'canceled';

export type PayoutOption = {
  id: string;
  title: string;
  hint: string;
  min: Money | null;
};

export type Withdrawal = {
  id: number;
  amount: Money;
  method: string;
  method_title?: string;
  details: string;
  status: WithdrawalStatus;
  comment: string | null;
  created_at: string;
  decided_at: string | null;
};

export type Referral = {
  code: string;
  bot_link: string | null;
  invited: number;
  earned: Money;
  balance?: Money;
  levels: number[];
  withdraw_min?: Money | null;
  withdrawals_enabled?: boolean;
  payout_methods?: PayoutOption[];
  withdrawals?: Withdrawal[];
};

export const GB = 1024 ** 3;

export function stars(n: number): string {
  return count(n, 'звезда', 'звезды', 'звёзд');
}

export function gb(bytes: number): string {
  const value = bytes / GB;
  const rounded = value >= 100 ? Math.round(value) : Math.round(value * 10) / 10;
  return lang() === 'en' ? String(rounded) : String(rounded).replace('.', ',');
}

export function gbText(bytes: number): string {
  return t('{n} ГБ', { n: gb(bytes) });
}

function nextMonthFirst(now = new Date()): Date {
  return new Date(now.getFullYear(), now.getMonth() + 1, 1);
}

function nextAnniversary(start: string | null, now = new Date()): Date | null {
  if (!start) {
    return null;
  }
  const day = new Date(start).getDate();
  const clamp = (y: number, m: number) => new Date(y, m, Math.min(day, new Date(y, m + 1, 0).getDate()));
  const thisMonth = clamp(now.getFullYear(), now.getMonth());
  return thisMonth > now ? thisMonth : clamp(now.getFullYear(), now.getMonth() + 1);
}

export function resetWhen(reset: TrafficReset, sub: Subscription | null): { tail: string; until: string } {
  switch (reset) {
    case 'month': {
      const date = dateLong(nextMonthFirst().toISOString());
      return { tail: t('обновится {date}', { date }), until: t('До {date}', { date }) };
    }
    case 'month_rolling': {
      const next = nextAnniversary(sub?.period_started_at ?? sub?.created_at ?? null);
      const date = next ? dateLong(next.toISOString()) : '';
      return {
        tail: date ? t('обновится {date}', { date }) : t('обновится в день оформления'),
        until: date ? t('До {date}', { date }) : t('До дня оформления'),
      };
    }
    case 'week':
      return { tail: t('обновится в понедельник'), until: t('До понедельника') };
    case 'day':
      return { tail: t('обновится завтра'), until: t('До завтра') };
    case 'on_payment':
      return { tail: t('обновится при продлении'), until: t('До продления') };
    default:
      return { tail: t('на весь срок'), until: '' };
  }
}

export function resetPlan(reset: TrafficReset): string {
  switch (reset) {
    case 'month':
      return t('в месяц, обновляется 1-го числа');
    case 'month_rolling':
      return t('в месяц, обновляется в день оформления');
    case 'week':
      return t('в неделю, обновляется по понедельникам');
    case 'day':
      return t('в день, обновляется каждый день');
    case 'on_payment':
      return t('на оплаченный срок, обновляется при каждой оплате');
    default:
      return t('на весь срок');
  }
}

export type Tone = 'ink2' | 'warn' | 'muted' | 'bad';

export function statusWord(view: SubscriptionView | null, blocked: boolean): { text: string; tone: Tone } {
  if (blocked || view?.status === 'blocked') {
    return { text: t('Заблокирована'), tone: 'bad' };
  }
  if (!view) {
    return { text: t('Нет подписки'), tone: 'muted' };
  }
  switch (view.status) {
    case 'limited':
      return { text: t('Трафик закончился'), tone: 'warn' };
    case 'expired':
      return { text: t('Истекла'), tone: 'muted' };
    case 'expiring':
      return { text: t('Скоро закончится'), tone: 'warn' };
    case 'trial':
      return { text: t('Пробный период'), tone: 'ink2' };
    default:
      return { text: t('Активна'), tone: 'ink2' };
  }
}

export function periodTitle(view: SubscriptionView): string {
  const name = view.tariff?.name ?? (view.subscription.kind === 'trial' ? t('Пробный период') : t('Подписка'));
  const d = view.subscription.period_days;
  if (!d || view.subscription.kind === 'trial') {
    return name;
  }
  const months = Math.max(1, Math.round((d * 12) / 365));
  const label = d >= 28 ? count(months, 'месяц', 'месяца', 'месяцев') : days(d);
  return `${name} · ${label}`;
}

export function trafficLine(view: SubscriptionView): { used: string; tail: string; exhausted: boolean } | null {
  const limit = view.subscription.mirror.traffic_limit_bytes ?? view.tariff?.traffic.limit_bytes ?? 0;
  if (!limit) {
    return null;
  }
  const exhausted = view.status === 'limited';
  const used = exhausted ? limit : (view.subscription.mirror.traffic_used_bytes ?? 0);
  const reset = view.tariff?.traffic.reset ?? 'no_reset';
  const when = resetWhen(reset, view.subscription);
  return {
    used: t('{used} из {limit} ГБ', { used: gb(used), limit: gb(limit) }),
    tail: reset === 'no_reset' ? ` ${t('на весь срок')}` : ` · ${when.tail}`,
    exhausted,
  };
}

export type TrafficMeter = { used: string; limit: string; left: string; share: number; tail: string; exhausted: boolean };

export function trafficMeter(view: SubscriptionView): TrafficMeter | null {
  const limit = view.subscription.mirror.traffic_limit_bytes ?? view.tariff?.traffic.limit_bytes ?? 0;
  if (!limit) {
    return null;
  }
  const exhausted = view.status === 'limited';
  const used = exhausted ? limit : Math.min(Math.max(view.subscription.mirror.traffic_used_bytes ?? 0, 0), limit);
  const reset = view.tariff?.traffic.reset ?? 'no_reset';
  return {
    used: gbText(used),
    limit: gbText(limit),
    left: gbText(Math.max(limit - used, 0)),
    share: Math.min(1, used / limit),
    tail: resetWhen(reset, view.subscription).tail,
    exhausted,
  };
}

export function explanation(view: SubscriptionView | null, blocked: boolean): string | null {
  if (blocked || view?.status === 'blocked') {
    return t('Подписка заблокирована. Напишите в поддержку.');
  }
  if (!view) {
    return null;
  }
  if (view.status === 'limited') {
    const reset = view.tariff?.traffic.reset ?? 'no_reset';
    const until = resetWhen(reset, view.subscription).until;
    const head = until ? t('{until} VPN не работает.', { until }) : t('VPN не работает.');
    return `${head} ${reset === 'on_payment' ? t('После оплаты трафик обновится.') : t('Продление трафик не добавляет.')}`;
  }
  if (view.status === 'expired') {
    return t('VPN не работает, пока подписка не продлена. После оплаты обновите подписку в приложении — ссылка останется прежней.');
  }
  return null;
}

export function deviceIcon(device: Device): 'phone' | 'laptop' {
  const p = `${device.platform ?? ''}`.toLowerCase();
  return p.includes('ios') || p.includes('android') || p.includes('ipad') ? 'phone' : 'laptop';
}

export function appName(device: Device): string {
  return (device.app ?? '').replace(/\s+[\d.]+$/, '');
}

export function amountMinor(m: Money): number {
  return m.minor;
}
