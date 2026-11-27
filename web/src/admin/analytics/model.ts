import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { count, groupDigits, money, NBSP, traffic } from '../../lib/format';
import type { Currency } from '../../lib/types';
import { useSession } from '../session';
import { localToday, periodText, presetDates, rollingPreset, type Preset } from './period';

export { axisLabel, dayText, localToday, monthTitle, periodText, PRESETS, presetDates, presetFromParam, type Preset } from './period';

export type Unit = 'money' | 'stars' | 'count' | 'percent' | 'seconds' | 'bytes' | 'days';
export type Step = 'day' | 'week' | 'month';
export type Compare = 'prev' | 'year' | 'none';

export type ListRef = { kind: 'payments' | 'clients'; set: string; slice?: string; from?: string; to?: string };
export type Point = { at: string; value: number };
export type BRow = { key: string; label: string; value: number; count?: number; prev?: number; list?: ListRef };
export type Breakdown = { key: string; title: string; rows: BRow[] };
export type Column = { title: string; unit: Unit; share_of?: number };
export type Row = { label: string; sub?: string; cells: Array<number | null>; list?: ListRef; children?: Row[] };
export type Bar = { label: string; value: number; list?: ListRef };
export type CohortRow = { month: string; size: number; kept: Array<number | null>; money: Array<number | null> };

export type Detail =
  | { type: 'none' }
  | { type: 'series'; points: Point[]; prev: Point[]; breakdowns: Breakdown[]; list?: ListRef }
  | { type: 'table'; columns: Column[]; rows: Row[]; note?: string }
  | { type: 'bars'; bars: Bar[]; note?: string }
  | { type: 'cohorts'; columns: number; rows: CohortRow[] };

export type Metric = {
  key: string;
  title: string;
  unit: Unit;
  value: number | null;
  prev?: number | null;
  note?: string;
  hint?: string;
  sub: boolean;
  detail: Detail;
};

export type Group = { title?: string; metrics: Metric[] };

export type TabReport = {
  tab: string;
  currency: Currency;
  range: { from: string; to: string; step: Step; prev?: { from: string; to: string }; today: string };
  groups: Group[];
  notes?: string[];
  preparing: boolean;
};

export type Choice = { value: string; label: string };
export type Options = {
  tariffs: Choice[];
  categories: Choice[];
  channels: Choice[];
  sources: Choice[];
  providers: Choice[];
  methods: Choice[];
};

export type ListItem = { client_id: number; name: string; what: string; amount?: number; stars: boolean; at?: string };
export type ListPage = { kind: 'payments' | 'clients'; total: number; page: number; more: boolean; items: ListItem[] };

export const TABS: Array<[string, string]> = [
  ['money', 'Деньги'],
  ['clients', 'Клиенты'],
  ['retention', 'Удержание'],
  ['payments', 'Платёжки'],
  ['sources', 'Источники'],
  ['tariffs', 'Тарифы'],
  ['economy', 'Экономика'],
  ['support', 'Поддержка'],
];

export type FilterKey = 'tariff' | 'category' | 'channel' | 'source' | 'provider' | 'method';
export type Filters = Partial<Record<FilterKey, string>>;

export const FILTERS: Array<[FilterKey, string, keyof Options]> = [
  ['tariff', 'Тариф', 'tariffs'],
  ['category', 'Категория', 'categories'],
  ['channel', 'Канал', 'channels'],
  ['source', 'Источник', 'sources'],
  ['provider', 'Платёжка', 'providers'],
  ['method', 'Способ', 'methods'],
];

export type Query = {
  preset: Preset;
  from: string;
  to: string;
  step: Step | null;
  compare: Compare;
  filters: Filters;
};

export function compareText(r: TabReport['range']): string | null {
  if (!r.prev) {
    return null;
  }
  return periodText(r.prev.from, r.prev.to);
}

export function stars(n: number): string {
  return count(Math.round(n), 'звезда', 'звезды', 'звёзд');
}

export function shareOf(cells: Array<number | null>, col: Column, i: number): string | null {
  if (col.share_of === undefined) {
    return null;
  }
  const value = cells[i];
  const base = cells[col.share_of];
  if (value === null || value === undefined || !base) {
    return null;
  }
  return percentText((value / base) * 100);
}

export function linkColumn(columns: Column[]): number {
  return columns.find((col) => col.share_of !== undefined)?.share_of ?? 0;
}

export function percentText(v: number): string {
  const r = Math.round(v * 10) / 10;
  return `${Number.isInteger(r) ? r : r.toFixed(1).replace('.', ',')}${NBSP}%`;
}

export function duration(seconds: number): string {
  const s = Math.max(0, Math.round(seconds));
  if (s < 60) {
    return `${s}${NBSP}с`;
  }
  if (s < 3600) {
    return `${Math.round(s / 60)}${NBSP}мин`;
  }
  if (s < 86_400) {
    const h = Math.floor(s / 3600);
    const m = Math.round((s % 3600) / 60);
    return m ? `${h}${NBSP}ч ${m}${NBSP}мин` : `${h}${NBSP}ч`;
  }
  const d = Math.floor(s / 86_400);
  const h = Math.round((s % 86_400) / 3600);
  return h ? `${d}${NBSP}дн ${h}${NBSP}ч` : count(d, 'день', 'дня', 'дней');
}

export function compactMoney(minor: number, currency: Currency): string {
  const major = minor / 100;
  const abs = Math.abs(major);
  const tail = currency === 'RUB' ? `${NBSP}₽` : '';
  const head = currency === 'USD' ? '$' : '';
  if (abs >= 1_000_000) {
    return `${head}${(major / 1_000_000).toFixed(abs >= 10_000_000 ? 0 : 1).replace('.', ',')}${NBSP}млн${tail}`;
  }
  if (abs >= 1_000) {
    return `${head}${(major / 1_000).toFixed(abs >= 10_000 ? 0 : 1).replace('.', ',')}${NBSP}тыс${tail}`;
  }
  return `${head}${Math.round(major)}${tail}`;
}

export function format(value: number | null | undefined, unit: Unit, currency: Currency, short = false): string {
  if (value === null || value === undefined || Number.isNaN(value)) {
    return '—';
  }
  switch (unit) {
    case 'money':
      return short ? compactMoney(value, currency) : money({ minor: Math.round(value), currency });
    case 'stars':
      return short && Math.abs(value) >= 10_000 ? `${(value / 1000).toFixed(0)}${NBSP}тыс` : groupDigits(value);
    case 'percent':
      return percentText(value);
    case 'seconds':
      return duration(value);
    case 'bytes':
      return traffic(value);
    case 'days':
      return groupDigits(value);
    default:
      return short && Math.abs(value) >= 100_000 ? `${(value / 1000).toFixed(0)}${NBSP}тыс` : groupDigits(value);
  }
}

export function change(value: number | null | undefined, prev: number | null | undefined, unit: Unit): string | null {
  if (value === null || value === undefined || prev === null || prev === undefined) {
    return null;
  }
  if (unit === 'percent') {
    const d = Math.round((value - prev) * 10) / 10;
    if (d === 0) {
      return `0${NBSP}п.`;
    }
    return `${d > 0 ? '↑' : '↓'}${NBSP}${String(Math.abs(d)).replace('.', ',')}${NBSP}п.`;
  }
  if (prev === 0) {
    return null;
  }
  const r = ((value - prev) / Math.abs(prev)) * 100;
  if (Math.abs(r) < 0.05) {
    return `0${NBSP}%`;
  }
  const digits = Math.abs(r) >= 10 ? 0 : 1;
  return `${r > 0 ? '↑' : '↓'}${NBSP}${Math.abs(r).toFixed(digits).replace('.', ',')}${NBSP}%`;
}

export function queryString(q: Query, extra: Record<string, string | undefined> = {}): string {
  const p = new URLSearchParams();
  p.set('from', q.from);
  p.set('to', q.to);
  if (q.step) {
    p.set('step', q.step);
  }
  p.set('compare', q.compare);
  if (rollingPreset(q.preset)) {
    p.set('rolling', 'true');
  }
  for (const [k, v] of Object.entries(q.filters)) {
    if (v) {
      if (k === 'method') {
        const [prov = '', code] = v.split('/');
        p.set('provider', prov);
        p.set('method', code ?? '');
      } else {
        p.set(k, v);
      }
    }
  }
  for (const [k, v] of Object.entries(extra)) {
    if (v !== undefined) {
      p.set(k, v);
    }
  }
  return p.toString();
}

export function useQueryState(initial: Filters = {}, preset: Preset = 'month'): [Query, (next: Partial<Query>) => void, (today: string) => void] {
  const today = useRef(localToday());
  const [q, setQ] = useState<Query>(() => ({ preset, ...presetDates(preset, today.current), step: null, compare: 'prev', filters: initial }));
  const update = (next: Partial<Query>) =>
    setQ((cur) => {
      const merged = { ...cur, ...next };
      if (next.preset && next.preset !== 'custom') {
        Object.assign(merged, presetDates(next.preset, today.current), { step: next.step ?? null });
      }
      return merged;
    });
  const sync = useCallback((panel: string) => {
    if (!panel || panel === today.current) {
      return;
    }
    today.current = panel;
    setQ((cur) => {
      if (cur.preset === 'custom') {
        return cur;
      }
      const dates = presetDates(cur.preset, panel);
      return dates.from === cur.from && dates.to === cur.to ? cur : { ...cur, ...dates };
    });
  }, []);
  return [q, update, sync];
}

export type Tab = {
  allowed: boolean;
  report: TabReport | null;
  loading: boolean;
  error: string | null;
  off: boolean;
  reload: () => void;
};

export function useTab(tab: string, q: Query): Tab {
  const { can } = useSession();
  const allowed = can('money');
  const [report, setReport] = useState<TabReport | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [off, setOff] = useState(false);
  const [round, setRound] = useState(0);
  const key = `${tab}?${queryString(q)}`;
  useEffect(() => {
    if (!allowed) {
      return;
    }
    const abort = new AbortController();
    let timer: number | undefined;
    setLoading(true);
    setError(null);
    const load = () =>
      api<TabReport>(`/api/admin/m/analytics/tab/${key}`, { signal: abort.signal })
        .then((data) => {
          setReport(data);
          setOff(false);
          if (data.preparing) {
            timer = window.setTimeout(load, 3000);
          } else {
            setLoading(false);
          }
        })
        .catch((err: unknown) => {
          if (abort.signal.aborted) {
            return;
          }
          setLoading(false);
          if (err instanceof ApiError && err.code === 'module.disabled') {
            setOff(true);
            return;
          }
          setError(err instanceof ApiError ? err.message : 'Не получилось загрузить аналитику');
        });
    void load();
    return () => {
      abort.abort();
      if (timer) {
        window.clearTimeout(timer);
      }
    };
  }, [allowed, key, round]);
  return { allowed, report, loading, error, off, reload: () => setRound((n) => n + 1) };
}

export function useOptions(enabled: boolean): Options | null {
  const [options, setOptions] = useState<Options | null>(null);
  useEffect(() => {
    if (!enabled) {
      return;
    }
    api<Options>('/api/admin/m/analytics/options')
      .then(setOptions)
      .catch(() => setOptions(null));
  }, [enabled]);
  return options;
}

export function useSelected(report: TabReport | null): [Metric | null, (key: string) => void] {
  const [key, setKey] = useState<string | null>(null);
  const metric = useMemo(() => {
    if (!report) {
      return null;
    }
    const all = report.groups.flatMap((g) => g.metrics);
    return all.find((m) => m.key === key) ?? all[0] ?? null;
  }, [report, key]);
  return [metric, setKey];
}

export function filterLabel(options: Options | null, key: FilterKey, value: string): string {
  const field = FILTERS.find((f) => f[0] === key);
  const list = field && options ? options[field[2]] : [];
  return list.find((c) => c.value === value)?.label ?? value;
}

export async function loadList(ref: ListRef, q: Query, page: number): Promise<ListPage> {
  const extra: Record<string, string | undefined> = { set: ref.set, slice: ref.slice, page: String(page) };
  const base: Query = { ...q, from: ref.from ?? q.from, to: ref.to ?? q.to };
  return api<ListPage>(`/api/admin/m/analytics/list?${queryString(base, extra)}`);
}

export function amountText(item: ListItem, currency: Currency): string {
  if (item.amount === undefined) {
    return '';
  }
  return item.stars ? stars(item.amount) : money({ minor: item.amount, currency });
}
