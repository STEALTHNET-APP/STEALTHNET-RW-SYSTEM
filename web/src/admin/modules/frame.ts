export const FRAME_SANDBOX = 'allow-scripts allow-forms allow-popups allow-popups-to-escape-sandbox allow-downloads allow-modals';
export const HEIGHT_MESSAGE = 'stealthnet:height';
export const THEME_MESSAGE = 'stealthnet:theme';
export const CHANGED_MESSAGE = 'stealthnet:changed';
export const THEME_PARAM = 'sn_theme';
export const FRAME_MIN = 48;
export const FRAME_MAX = 4000;

export type CardFrame = { module: string; name: string; title: string; url: string };

export function withTheme(url: string, theme: string): string {
  const cut = url.indexOf('#');
  const path = cut === -1 ? url : url.slice(0, cut);
  const hash = cut === -1 ? '' : url.slice(cut);
  return `${path}${path.includes('?') ? '&' : '?'}${THEME_PARAM}=${encodeURIComponent(theme)}${hash}`;
}

export function frameHeight(value: unknown): number | null {
  if (typeof value !== 'number' || !Number.isFinite(value)) {
    return null;
  }
  return Math.min(FRAME_MAX, Math.max(FRAME_MIN, Math.ceil(value)));
}
