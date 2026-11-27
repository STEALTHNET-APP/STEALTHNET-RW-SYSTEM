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

export type LevelRow = { label: string; percent: number };

export function levelRows(levels: readonly number[]): LevelRow[] {
  const [first = 0, second = 0, third = 0] = levels;
  return [
    { label: t('С оплат друзей'), percent: first },
    { label: t('С оплат их друзей'), percent: second },
    { label: t('С оплат друзей их друзей'), percent: third },
  ].filter((row) => row.percent > 0);
}
