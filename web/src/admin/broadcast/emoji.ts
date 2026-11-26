const TAG = /<tg-emoji\s+emoji-id="\d{1,32}">[^<]*<\/tg-emoji>/;

export type ButtonStyle = 'primary' | 'success' | 'danger';

export const BUTTON_STYLES: Array<{ value: ButtonStyle | ''; label: string }> = [
  { value: '', label: 'Обычная' },
  { value: 'primary', label: 'Синяя' },
  { value: 'success', label: 'Зелёная' },
  { value: 'danger', label: 'Красная' },
];

function unescape(text: string): string {
  return text.replace(/&lt;/g, '<').replace(/&gt;/g, '>').replace(/&quot;/g, '"').replace(/&amp;/g, '&');
}

export type Pasted = { kind: 'text'; value: string } | { kind: 'code'; value: string } | { kind: 'none' };

export function readPasted(raw: string): Pasted {
  const lines = raw
    .split(/\r?\n/)
    .map((line) => line.trim())
    .filter(Boolean);
  for (const line of lines) {
    if (TAG.test(line)) {
      return { kind: 'text', value: line };
    }
    const decoded = unescape(line);
    if (TAG.test(decoded)) {
      return { kind: 'text', value: decoded };
    }
  }
  const code = raw.trim();
  if (/^\d{1,32}$/.test(code)) {
    return { kind: 'code', value: code };
  }
  return { kind: 'none' };
}

export function insertAt(text: string, start: number, end: number, value: string): { text: string; caret: number } {
  const from = Math.max(0, Math.min(start, text.length));
  const to = Math.max(from, Math.min(end, text.length));
  return { text: text.slice(0, from) + value + text.slice(to), caret: from + value.length };
}

export function premiumCount(html: string): number {
  return (html.match(/<tg-emoji\b/g) ?? []).length;
}

export function emojiIdProblem(raw: string): string | null {
  const value = raw.trim();
  return value && !/^\d{1,32}$/.test(value) ? 'Код премиум-эмодзи — только цифры, его присылает бот по команде /emoji' : null;
}
