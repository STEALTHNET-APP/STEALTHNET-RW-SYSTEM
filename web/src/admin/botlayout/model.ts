export type ClientState = 'new' | 'active' | 'ended';
export type Kind = 'callback' | 'link' | 'web_app' | 'copy';
export type Style = 'primary' | 'success' | 'danger';
export type Audience = 'all' | 'subscribed' | 'unsubscribed';
export type Rows = string[][];
export type ScreenKey = 'menu' | 'subscription';
export type Lang = 'ru' | 'en';
export type MenuMode = 'classic' | 'web';

export type Slot = {
  key: string;
  label: string;
  text: string;
  text_en: string;
  audience: Audience;
  when: string;
  off: string | null;
  hidden: boolean;
  hideable: boolean;
  style: Style | null;
  emoji: boolean;
  kind: Kind;
  web: string | null;
  web_note: string | null;
  shown: ClientState[];
};

export type ScreenView = {
  key: ScreenKey;
  title: string;
  rows: Rows;
  standard: Rows;
  custom: boolean;
  packed: number | null;
  states: ClientState[];
  slots: Slot[];
};

export type LayoutView = {
  screens: ScreenView[];
  menu_mode: MenuMode;
  web_off: string | null;
  row_max: number;
  english: boolean;
  balance: boolean;
  brand: string;
  texts: Record<string, string>;
  texts_en: Record<string, string>;
};

export type Draft = { rows: Rows; custom: boolean };

export const ROW_MAX = 3;

export const STATE_LABEL: Record<ClientState, string> = {
  new: 'Новый клиент',
  active: 'С подпиской',
  ended: 'Подписка закончилась',
};

export function arrange<T>(rows: Rows, present: Array<[string, T]>): T[][] {
  const left = present.map((item) => ({ key: item[0], value: item[1], used: false }));
  const out: T[][] = [];
  for (const row of rows) {
    const line: T[] = [];
    for (const key of row) {
      const found = left.find((item) => !item.used && item.key === key);
      if (found) {
        found.used = true;
        line.push(found.value);
      }
    }
    for (let i = 0; i < line.length; i += ROW_MAX) {
      out.push(line.slice(i, i + ROW_MAX));
    }
  }
  for (const item of left) {
    if (!item.used) {
      out.push([item.value]);
    }
  }
  return out;
}

export function pack<T>(present: Array<[string, T]>, columns: number): T[][] {
  const size = Math.min(Math.max(columns, 1), ROW_MAX);
  const out: T[][] = [];
  for (let i = 0; i < present.length; i += size) {
    out.push(present.slice(i, i + size).map((item) => item[1]));
  }
  return out;
}

export const MENU_MODES: Array<{ mode: MenuMode; label: string; text: string }> = [
  {
    mode: 'classic',
    label: 'Классический',
    text: 'Кнопки главного меню открывают экраны бота прямо в чате.',
  },
  {
    mode: 'web',
    label: 'Веб',
    text: 'Кнопки главного меню открывают свои разделы мини-приложения: «Купить» — тарифы, «Баланс» — пополнение, «Пригласить друга» — профиль. Кнопки, у которых в мини-приложении нет раздела, работают в чате, как раньше.',
  },
];

export function webOn(view: LayoutView, mode: MenuMode): boolean {
  return mode === 'web' && view.web_off === null;
}

export function kindIn(slot: Slot, web: boolean): Kind {
  return web && slot.web !== null ? 'web_app' : slot.kind;
}

export function opensText(slot: Slot, web: boolean): string {
  const kind = kindIn(slot, web);
  if (kind === 'web_app') {
    return slot.web ? `Мини-приложение, раздел «${slot.web}»` : 'Мини-приложение';
  }
  if (kind === 'link') {
    return 'Ссылку';
  }
  if (kind === 'copy') {
    return 'Ничего: копирует ссылку подписки';
  }
  return 'Экран бота в чате';
}

export function visibleIn(slot: Slot, state: ClientState, hidden: Record<string, boolean>): boolean {
  return slot.shown.includes(state) && !(hidden[slot.key] ?? slot.hidden);
}

export function keyboardFor(screen: ScreenView, draft: Draft, state: ClientState, hidden: Record<string, boolean>): Slot[][] {
  const present: Array<[string, Slot]> = screen.slots.filter((slot) => visibleIn(slot, state, hidden)).map((slot) => [slot.key, slot]);
  if (!draft.custom && screen.packed !== null) {
    return pack(present, screen.packed);
  }
  return arrange(draft.rows, present);
}

export function sameRows(a: Rows, b: Rows): boolean {
  return a.length === b.length && a.every((row, i) => row.length === b[i]?.length && row.every((key, j) => key === b[i]?.[j]));
}

export function spread(rows: Rows, columns: number): Rows {
  const flat = rows.flat();
  const size = Math.min(Math.max(columns, 1), ROW_MAX);
  const out: Rows = [];
  for (let i = 0; i < flat.length; i += size) {
    out.push(flat.slice(i, i + size));
  }
  return out;
}

export function locate(rows: Rows, key: string): { row: number; at: number } | null {
  for (let row = 0; row < rows.length; row += 1) {
    const at = rows[row]?.indexOf(key) ?? -1;
    if (at >= 0) {
      return { row, at };
    }
  }
  return null;
}

function cleanRows(rows: Rows): Rows {
  return rows.filter((row) => row.length > 0);
}

export type Target = { row: number; at: number } | { gap: number };

export function place(rows: Rows, key: string, target: Target): Rows {
  const spot = locate(rows, key);
  if (!spot) {
    return rows;
  }
  if ('gap' in target) {
    const out: Rows = rows.map((row) => row.filter((k) => k !== key));
    out.splice(target.gap, 0, [key]);
    return cleanRows(out);
  }
  const same = spot.row === target.row;
  const line = rows[target.row] ?? [];
  if (!same && line.length >= ROW_MAX) {
    return rows;
  }
  const out: Rows = rows.map((row) => [...row]);
  out[spot.row]?.splice(spot.at, 1);
  const at = same && target.at > spot.at ? target.at - 1 : target.at;
  out[target.row]?.splice(Math.min(at, out[target.row]?.length ?? 0), 0, key);
  return cleanRows(out);
}

export type Projection = { rows: Rows; full: number[] };

export function project(rows: Rows, visible: (key: string) => boolean): Projection {
  const out: Projection = { rows: [], full: [] };
  rows.forEach((row, index) => {
    const line = row.filter(visible);
    if (line.length > 0) {
      out.rows.push(line);
      out.full.push(index);
    }
  });
  return out;
}

export type Move = 'left' | 'right' | 'up' | 'down' | 'own';

export function moveTarget(rows: Rows, shown: Projection, key: string, move: Move): Target | null {
  const d = shown.rows.findIndex((row) => row.includes(key));
  const line = shown.rows[d];
  const f = shown.full[d];
  const full = f === undefined ? undefined : rows[f];
  if (!line || f === undefined || !full) {
    return null;
  }
  const i = line.indexOf(key);
  if (move === 'left') {
    const neighbor = line[i - 1];
    return neighbor === undefined ? null : { row: f, at: full.indexOf(neighbor) };
  }
  if (move === 'right') {
    const neighbor = line[i + 1];
    return neighbor === undefined ? null : { row: f, at: full.indexOf(neighbor) + 1 };
  }
  const alone = full.length === 1;
  if (move === 'own') {
    return alone ? null : { gap: f + 1 };
  }
  const dir = move === 'up' ? -1 : 1;
  const td = d + dir;
  const tf = shown.full[td];
  const target = tf === undefined ? undefined : rows[tf];
  if (tf === undefined || !target) {
    return alone ? null : { gap: dir < 0 ? f : f + 1 };
  }
  if (target.length < ROW_MAX) {
    return { row: tf, at: dir < 0 ? target.length : 0 };
  }
  if (alone) {
    return { gap: dir < 0 ? tf : tf + 1 };
  }
  return { gap: dir < 0 ? f : f + 1 };
}

export function applyMove(rows: Rows, shown: Projection, key: string, move: Move): Rows {
  const target = moveTarget(rows, shown, key, move);
  return target ? place(rows, key, target) : rows;
}

export function dropTarget(rows: Rows, key: string, over: string, after: boolean): Target | null {
  const from = locate(rows, key);
  const spot = locate(rows, over);
  const line = spot ? rows[spot.row] : undefined;
  if (!from || !spot || !line || over === key) {
    return null;
  }
  if (from.row !== spot.row && line.length >= ROW_MAX) {
    return null;
  }
  return { row: spot.row, at: spot.at + (after ? 1 : 0) };
}

export function gapTarget(rows: Rows, shown: Rows, gap: number): Target | null {
  const before = shown[gap]?.[0];
  if (before !== undefined) {
    const spot = locate(rows, before);
    return spot ? { gap: spot.row } : null;
  }
  const last = shown[shown.length - 1];
  const tail = last?.[last.length - 1];
  const spot = tail === undefined ? null : locate(rows, tail);
  return spot ? { gap: spot.row + 1 } : { gap: rows.length };
}

const ORDINAL = ['первая', 'вторая', 'третья'];
const OF = ['', '', 'двух', 'трёх'];

export function placeText(shown: Rows, key: string): string | null {
  const row = shown.findIndex((line) => line.includes(key));
  const line = shown[row];
  if (!line) {
    return null;
  }
  if (line.length === 1) {
    return `Ряд ${row + 1}, одна в ряду`;
  }
  return `Ряд ${row + 1}, ${ORDINAL[line.indexOf(key)] ?? ''} из ${OF[line.length] ?? line.length}`;
}

export type Samples = Record<string, string>;

export const SAMPLES: Record<Lang, Samples> = {
  ru: {
    tariff: 'Стандарт',
    date: '14 февраля 2027',
    ended: '2 ноября 2026',
    days: '87 дней',
    used: '12 ГБ',
    used_ended: '48 ГБ',
    limit: '100 ГБ',
    count: '2',
    devices: '3',
    balance: '150 ₽',
    url: 'https://sub.example.com/s/9f3c4a',
  },
  en: {
    tariff: 'Standard',
    date: '14 February 2027',
    ended: '2 November 2026',
    days: '87 days',
    used: '12 GB',
    used_ended: '48 GB',
    limit: '100 GB',
    count: '2',
    devices: '3',
    balance: '$1.50',
    url: 'https://sub.example.com/s/9f3c4a',
  },
};

function escapeHtml(value: string): string {
  return value.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;').replace(/"/g, '&quot;');
}

export function fillTemplate(template: string, text: Record<string, string>, html: Record<string, string> = {}): string {
  return template.replace(/\{([a-z0-9_]{1,32})\}/g, (whole, name: string) => {
    if (name in html) {
      return html[name] ?? '';
    }
    if (name in text) {
      return escapeHtml(text[name] ?? '');
    }
    return whole;
  });
}

export function messageFor(view: LayoutView, screen: ScreenKey, state: ClientState, lang: Lang): string {
  const texts = lang === 'en' ? view.texts_en : view.texts;
  const s = SAMPLES[lang];
  const t = (key: string, vars: Record<string, string> = {}, html: Record<string, string> = {}) => fillTemplate(texts[key] ?? '', vars, html);
  const ended = state === 'ended';
  const status = t(ended ? 'status.expired' : 'status.active');
  const lines: string[] = [];
  if (screen === 'menu') {
    lines.push(t('menu.title', { brand: view.brand }), t('menu.greeting', { brand: view.brand, name: lang === 'en' ? 'Ann' : 'Алексей' }));
    if (state === 'new') {
      lines.push(t('menu.no_subscription'));
    } else {
      lines.push(
        t('menu.single', { tariff: s.tariff ?? '' }, { status }),
        ended ? t('menu.single_ended', { date: s.ended ?? '' }) : t('menu.single_until', { date: s.date ?? '', days: s.days ?? '' }),
        t('menu.single_traffic', { used: (ended ? s.used_ended : s.used) ?? '', limit: s.limit ?? '' }),
        t('menu.single_devices', { count: s.count ?? '', limit: s.devices ?? '' }),
      );
      if (view.balance) {
        lines.push(t('menu.balance', { balance: s.balance ?? '' }));
      }
    }
  } else {
    lines.push(
      t('sub.title', { tariff: s.tariff ?? '' }, { status }),
      ended ? t('sub.ended', { date: s.ended ?? '' }) : t('sub.until', { date: s.date ?? '', days: s.days ?? '' }),
      t('sub.traffic', { used: (ended ? s.used_ended : s.used) ?? '', limit: s.limit ?? '' }, { reset: '' }),
      t('sub.devices', { count: s.count ?? '', limit: s.devices ?? '' }),
      t(ended ? 'sub.autorenew_off' : 'sub.autorenew_on'),
      t('sub.link', { url: s.url ?? '' }),
    );
  }
  return lines.filter((line) => line !== '').join('\n');
}
