import { t } from './i18n';

const KEY = 'sn.ref';
const TTL_MS = 30 * 24 * 3600 * 1000;
const CODE = /^[A-Z0-9]{4,16}$/;

type Saved = { code: string; at: number };

function read(): Saved | null {
  try {
    const raw = window.localStorage.getItem(KEY);
    if (!raw) {
      return null;
    }
    const saved = JSON.parse(raw) as Saved;
    if (typeof saved.code !== 'string' || !CODE.test(saved.code) || typeof saved.at !== 'number' || Date.now() - saved.at > TTL_MS) {
      return null;
    }
    return saved;
  } catch {
    return null;
  }
}

export function captureReferral(search: string = window.location.search) {
  const code = (new URLSearchParams(search).get('ref') ?? '').trim().toUpperCase();
  if (!CODE.test(code) || read()) {
    return;
  }
  try {
    window.localStorage.setItem(KEY, JSON.stringify({ code, at: Date.now() }));
  } catch {
    return;
  }
}

export function storedReferral(): string | null {
  return read()?.code ?? null;
}

export function siteInviteLink(code: string): string {
  return `${window.location.origin}/?ref=${encodeURIComponent(code)}`;
}

export function botLink(username: string, start?: string | null): string {
  const base = `https://t.me/${username.replace(/^@/, '')}`;
  return start ? `${base}?start=${encodeURIComponent(start)}` : base;
}

export function deeperLevels(levels: readonly number[]): string[] {
  const [, second = 0, third = 0] = levels;
  const lines: string[] = [];
  if (second > 0) {
    lines.push(t('{percent}% — их друзья', { percent: second }));
  }
  if (third > 0) {
    lines.push(t('{percent}% — друзья их друзей', { percent: third }));
  }
  return lines;
}

export function levelsText(levels: readonly number[]): string[] {
  const [first = 0] = levels;
  const lines = [first > 0 ? t('{percent}% — друзья', { percent: first }) : '', ...deeperLevels(levels)].filter(Boolean);
  return lines.length ? [t('Ваш процент с каждой оплаты:'), ...lines] : [];
}
