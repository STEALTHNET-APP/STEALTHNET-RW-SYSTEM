import type { Step } from './model';

export type Preset = 'today' | 'yesterday' | 'month' | 'last_month' | '7' | '30' | '90' | 'year' | 'all' | 'custom';

export const PRESETS: Array<[Preset, string]> = [
  ['today', 'Сегодня'],
  ['yesterday', 'Вчера'],
  ['month', 'Этот месяц'],
  ['last_month', 'Прошлый месяц'],
  ['7', '7 дней'],
  ['30', '30 дней'],
  ['90', '90 дней'],
  ['year', '12 месяцев'],
  ['all', 'Всё время'],
  ['custom', 'Свой период'],
];

const MONTHS = ['январь', 'февраль', 'март', 'апрель', 'май', 'июнь', 'июль', 'август', 'сентябрь', 'октябрь', 'ноябрь', 'декабрь'];
const MONTHS_GEN = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];
const MONTHS_SHORT = ['янв', 'фев', 'мар', 'апр', 'май', 'июн', 'июл', 'авг', 'сен', 'окт', 'ноя', 'дек'];

function iso(d: Date): string {
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

function parse(day: string): Date {
  const [y = 1970, m = 1, d = 1] = day.split('-').map(Number);
  return new Date(y, m - 1, d);
}

export function presetDates(preset: Preset, today: string): { from: string; to: string } {
  const t = parse(today);
  const back = (days: number) => iso(new Date(t.getFullYear(), t.getMonth(), t.getDate() - days + 1));
  switch (preset) {
    case 'yesterday':
      return { from: back(2), to: back(2) };
    case 'month':
      return { from: iso(new Date(t.getFullYear(), t.getMonth(), 1)), to: today };
    case 'last_month':
      return {
        from: iso(new Date(t.getFullYear(), t.getMonth() - 1, 1)),
        to: iso(new Date(t.getFullYear(), t.getMonth(), 0)),
      };
    case '7':
      return { from: back(7), to: today };
    case '30':
      return { from: back(30), to: today };
    case '90':
      return { from: back(90), to: today };
    case 'year':
      return { from: iso(new Date(t.getFullYear() - 1, t.getMonth() + 1, 1)), to: today };
    case 'all':
      return { from: '2015-01-01', to: today };
    default:
      return { from: today, to: today };
  }
}

export function rollingPreset(preset: Preset): boolean {
  return preset === '7' || preset === '30' || preset === '90';
}

export function presetFromParam(value: string | null): Preset | null {
  return PRESETS.some(([p]) => p === value && p !== 'custom') ? (value as Preset) : null;
}

export function localToday(): string {
  return iso(new Date());
}

export function dayText(day: string, withYear = true): string {
  const d = parse(day);
  return `${d.getDate()} ${MONTHS_GEN[d.getMonth()]}${withYear ? ` ${d.getFullYear()}` : ''}`;
}

export function monthTitle(day: string): string {
  const d = parse(day);
  const name = MONTHS[d.getMonth()] ?? '';
  return `${name.charAt(0).toUpperCase()}${name.slice(1)} ${d.getFullYear()}`;
}

export function periodText(from: string, to: string): string {
  if (from === to) {
    return dayText(from);
  }
  const a = parse(from);
  const b = parse(to);
  const lastDay = new Date(b.getFullYear(), b.getMonth() + 1, 0).getDate();
  if (a.getDate() === 1 && a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth()) {
    return b.getDate() === lastDay ? monthTitle(from) : `${monthTitle(from)}, по ${b.getDate()}`;
  }
  const sameYear = a.getFullYear() === b.getFullYear();
  return `${dayText(from, !sameYear)} — ${dayText(to)}`;
}

export function axisLabel(day: string, step: Step, first: boolean, prevDay: string | null): string {
  const d = parse(day);
  if (step === 'month') {
    const year = prevDay ? parse(prevDay).getFullYear() !== d.getFullYear() : first;
    const short = MONTHS_SHORT[d.getMonth()] ?? '';
    return year ? `${short} ${String(d.getFullYear()).slice(2)}` : short;
  }
  if (step === 'week') {
    return `${d.getDate()} ${MONTHS_SHORT[d.getMonth()]}`;
  }
  const monthChanged = prevDay ? parse(prevDay).getMonth() !== d.getMonth() : first;
  return monthChanged ? `${d.getDate()} ${MONTHS_SHORT[d.getMonth()]}` : String(d.getDate());
}
