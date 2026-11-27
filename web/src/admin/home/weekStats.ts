import type { Money } from '../../lib/format';

export type DayStat = {
  day: string;
  new_clients: number;
  new_clients_paid: number | null;
  revenue: Money | null;
  payments: number | null;
};

export type Totals = {
  revenue: Money | null;
  payments: number | null;
  newClients: number;
  newPaid: number | null;
};

export type WeekRow = { day: string; back: number; stat: DayStat };

export type Week = { rows: WeekRow[]; totals: Totals };

export const WEEK_DAYS = 7;

const WEEKDAYS = ['пн', 'вт', 'ср', 'чт', 'пт', 'сб', 'вс'];

export function totals(days: DayStat[]): Totals {
  const known = <T,>(pick: (d: DayStat) => T | null): T[] | null => {
    const values = days.map(pick);
    return values.every((v) => v !== null) ? (values as T[]) : null;
  };
  const revenue = known((d) => d.revenue);
  const payments = known((d) => d.payments);
  const paid = known((d) => d.new_clients_paid);
  return {
    revenue: revenue && revenue[0] ? { minor: revenue.reduce((sum, m) => sum + m.minor, 0), currency: revenue[0].currency } : null,
    payments: payments && days.length ? payments.reduce((sum, n) => sum + n, 0) : null,
    newClients: days.reduce((sum, d) => sum + d.new_clients, 0),
    newPaid: paid && days.length ? paid.reduce((sum, n) => sum + n, 0) : null,
  };
}

export function lastWeek(days: DayStat[]): Week | null {
  if (!days.length) {
    return null;
  }
  const current = days.slice(-WEEK_DAYS);
  return {
    rows: current.map((stat, i) => ({ day: stat.day, back: current.length - 1 - i, stat })),
    totals: totals(current),
  };
}

export function changePercent(value: Money | null, prev: Money | null | undefined): number | null {
  if (!value || !prev || prev.minor <= 0) {
    return null;
  }
  return Math.round(((value.minor - prev.minor) / prev.minor) * 100);
}

export function weekdayIndex(day: string): number {
  const [y = 1970, m = 1, d = 1] = day.split('-').map(Number);
  return (new Date(Date.UTC(y, m - 1, d)).getUTCDay() + 6) % 7;
}

export function dayLabel(day: string, back: number): string {
  if (back === 0) {
    return 'сегодня';
  }
  if (back === 1) {
    return 'вчера';
  }
  return `${WEEKDAYS[weekdayIndex(day)]} ${Number(day.slice(8, 10))}`;
}
