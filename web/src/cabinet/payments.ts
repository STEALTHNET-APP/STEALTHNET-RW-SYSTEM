import { bonusOf, bonusReached } from '../lib/bonus';
import { count, dateTime, money, moneyExact, periodLabel, timeHM, type Money } from '../lib/format';
import { t } from '../lib/i18n';
import type { Currency } from '../lib/types';
import { stars } from './model';

export type Amount = { minor: number; currency: Currency | 'XTR' };

export type PaymentStatus = 'pending' | 'paid' | 'failed' | 'canceled' | 'refunded' | 'partially_refunded';

export type Purchase = {
  kind: 'new' | 'renewal' | 'change' | 'top_up' | 'trial' | 'module' | 'admin_grant' | 'module_grant';
  tariff: string | null;
  days: number | null;
  months: number | null;
  label: string | null;
  extra_devices: number;
};

export type ClientPayment = {
  id: string;
  status: PaymentStatus;
  amount: Amount;
  refunded: Amount | null;
  bonus?: Amount;
  method: string;
  autorenew: boolean;
  repeat: 'balance' | 'review' | null;
  purchase: Purchase | null;
  created_at: string;
  paid_at: string | null;
  expires_at: string | null;
  pay_url: string | null;
};

export type PaymentsPage = { items: ClientPayment[]; next: string | null };

export type TopUpMethod = { provider: string; method: string | null; title: string; hint: string; amounts?: Money[]; bonus?: { percent: number; min?: Money; max?: Money } };

export type TopUpOptions = {
  currency: Currency;
  balance: Money;
  min: Money;
  max: Money;
  presets: Money[];
  methods: TopUpMethod[];
};

export type PaymentProgress = { payment_id: string; status: PaymentStatus; order_status: string; pay_url: string | null };

export type Tone = 'ink2' | 'warn' | 'muted' | 'bad';

export const PAGE = 20;

export function amountText(amount: Amount): string {
  if (amount.currency === 'XTR') {
    return stars(amount.minor);
  }
  return money({ minor: amount.minor, currency: amount.currency });
}

export function purchaseText(payment: ClientPayment): string {
  const p = payment.purchase;
  if (!p) {
    return t('Оплата');
  }
  const span = periodLabel(p.days) ?? '';
  const tariff = p.tariff ?? t('Подписка');
  switch (p.kind) {
    case 'top_up':
      return t('Пополнение баланса');
    case 'change':
      return span ? t('Переход на «{tariff}» · {span}', { tariff, span }) : t('Переход на «{tariff}»', { tariff });
    case 'trial':
      return t('Пробный период');
    case 'module':
      return p.label ?? t('Дополнительная услуга');
    default:
      return span ? `${tariff} · ${span}` : tariff;
  }
}

export function expired(payment: ClientPayment, now = Date.now()): boolean {
  return payment.status === 'pending' && !!payment.expires_at && new Date(payment.expires_at).getTime() <= now;
}

export function paymentState(payment: ClientPayment, now = Date.now()): { text: string; tone: Tone } {
  switch (payment.status) {
    case 'paid':
      return { text: t('Оплачено'), tone: 'ink2' };
    case 'pending':
      return expired(payment, now) ? { text: t('Отменено'), tone: 'muted' } : { text: t('Ждёт оплаты'), tone: 'warn' };
    case 'refunded':
    case 'partially_refunded':
      return { text: t('Возврат'), tone: 'ink2' };
    case 'failed':
      return { text: t('Не прошла'), tone: 'muted' };
    default:
      return { text: t('Отменено'), tone: 'muted' };
  }
}

export function paymentNote(payment: ClientPayment): string {
  const parts = [payment.autorenew ? t('{method}, автопродление', { method: payment.method }) : payment.method];
  if (payment.status === 'refunded') {
    parts.push(t('деньги вернули'));
  } else if (payment.refunded) {
    parts.push(t('вернули {refunded} из {amount}', { refunded: amountText(payment.refunded), amount: amountText(payment.amount) }));
  } else if (expired(payment)) {
    parts.push(t('время оплаты вышло'));
  }
  if (payment.bonus && payment.bonus.currency !== 'XTR') {
    parts.push(t('бонус за пополнение {bonus}', { bonus: moneyExact({ minor: payment.bonus.minor, currency: payment.bonus.currency }) }));
  }
  if (payment.repeat === 'balance') {
    parts.push(t('повторная оплата, деньги на балансе'));
  } else if (payment.repeat === 'review') {
    parts.push(t('повторная оплата, проверяем'));
  }
  return parts.join(' · ');
}

export function paymentWhen(payment: ClientPayment): string {
  return dateTime(payment.paid_at ?? payment.created_at);
}

export function payUntil(payment: ClientPayment): string {
  return payment.expires_at ? t('Оплатить до {time}', { time: timeHM(payment.expires_at) }) : t('Оплатить');
}

export function absoluteUrl(url: string): string {
  try {
    return new URL(url, window.location.origin).toString();
  } catch {
    return url;
  }
}

export function paymentsCount(n: number): string {
  return count(n, 'оплата', 'оплаты', 'оплат');
}

export function limitsText(options: TopUpOptions): string {
  return t('от\u00a0{min} до\u00a0{max}', { min: money(options.min), max: money(options.max) });
}

export function parseAmount(raw: string, currency: Currency): number | null {
  const cleaned = raw
    .replace(/[\s $₽]/g, '')
    .replace(',', '.')
    .trim();
  if (!cleaned) {
    return null;
  }
  const pattern = currency === 'RUB' ? /^\d{1,9}$/ : /^\d{1,7}(\.\d{1,2})?$/;
  if (!pattern.test(cleaned)) {
    return null;
  }
  const [whole = '0', frac = ''] = cleaned.split('.');
  return Number(whole) * 100 + Number(frac.padEnd(2, '0'));
}

export function wireAmount(minor: number, currency: Currency): string {
  return currency === 'RUB' ? String(Math.round(minor / 100)) : (minor / 100).toFixed(2);
}

export function topUpBonus(minor: number, method: TopUpMethod | null): number {
  return bonusOf(minor, method?.bonus);
}

export function bonusMark(method: TopUpMethod, minor: number | null): string | null {
  const bonus = method.bonus;
  if (!bonus) {
    return null;
  }
  if (bonus.min && (minor === null || !bonusReached(minor, bonus))) {
    return t('+{percent}% от\u00a0{min}', { percent: bonus.percent, min: money(bonus.min) });
  }
  return `+${bonus.percent}%`;
}

export function bonusText(minor: number, currency: Currency, method: TopUpMethod | null): string | null {
  if (method?.bonus?.min && !bonusReached(minor, method.bonus)) {
    return t('Бонус от {min}', { min: money(method.bonus.min) });
  }
  const bonus = topUpBonus(minor, method);
  if (bonus <= 0) {
    return null;
  }
  return t('На баланс придёт {total}: {amount} и бонус {bonus}', {
    total: moneyExact({ minor: minor + bonus, currency }),
    amount: money({ minor, currency }),
    bonus: moneyExact({ minor: bonus, currency }),
  });
}

export function finished(progress: PaymentProgress): boolean {
  return ['fulfilled', 'review', 'canceled', 'refunded'].includes(progress.order_status);
}
