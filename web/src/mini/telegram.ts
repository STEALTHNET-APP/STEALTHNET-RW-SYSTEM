import { bridgeApp, launchFrom, saveLaunch, savedLaunch, type BridgeHost, type Launch, type NativeProxy } from './bridge';
import type { InsetBox } from './insets';

export type WebAppListener = (payload?: unknown) => void;

export type WebApp = {
  initData: string;
  platform?: string;
  colorScheme?: 'dark' | 'light';
  isFullscreen?: boolean;
  safeAreaInset?: InsetBox;
  contentSafeAreaInset?: InsetBox;
  isVersionAtLeast?: (version: string) => boolean;
  setHeaderColor?: (color: string) => void;
  setBackgroundColor?: (color: string) => void;
  ready: () => void;
  expand?: () => void;
  requestFullscreen?: () => void;
  disableVerticalSwipes?: () => void;
  close?: () => void;
  openLink?: (url: string) => void;
  openTelegramLink?: (url: string) => void;
  downloadFile?: (params: { url: string; file_name: string }) => void;
  onEvent?: (name: string, cb: WebAppListener) => void;
  offEvent?: (name: string, cb: WebAppListener) => void;
  BackButton?: {
    show: () => void;
    hide: () => void;
    onClick: (cb: () => void) => void;
    offClick: (cb: () => void) => void;
  };
};

declare global {
  interface Window {
    Telegram?: BridgeHost['Telegram'];
    TelegramGameProxy?: BridgeHost['TelegramGameProxy'];
    TelegramWebviewProxy?: NativeProxy;
  }
}

const SDK = 'https://telegram.org/js/telegram-web-app.js';
export const SDK_WAIT_MS = 2500;
export const LATE_MS = 300;

let spare: WebApp | null = null;

export function webApp(): WebApp | null {
  return window.Telegram?.WebApp ?? spare;
}

export function launchData(): string {
  return launchFrom(window.location.hash).tgWebAppData ?? '';
}

function launch(): Launch {
  const fresh = launchFrom(window.location.hash);
  try {
    if (fresh.tgWebAppData) {
      saveLaunch(window.sessionStorage, fresh);
      return fresh;
    }
    return savedLaunch(window.sessionStorage);
  } catch {
    return fresh;
  }
}

function startSpare(params: Launch): WebApp | null {
  const proxy = window.TelegramWebviewProxy;
  if (!proxy) {
    return null;
  }
  try {
    spare = bridgeApp(params, proxy, window);
  } catch {
    spare = null;
  }
  return spare;
}

export function loadTelegram(late?: (app: WebApp) => void): Promise<WebApp | null> {
  const known = webApp();
  if (known) {
    return Promise.resolve(known);
  }
  const params = launch();
  if (!params.tgWebAppData) {
    return Promise.resolve(null);
  }
  return new Promise((resolve) => {
    let waiting = true;
    const done = () => {
      const app = window.Telegram?.WebApp ?? null;
      if (waiting) {
        waiting = false;
        resolve(app ?? startSpare(params));
      } else if (app) {
        window.setTimeout(() => late?.(app), LATE_MS);
      }
    };
    const script = document.createElement('script');
    script.src = SDK;
    script.async = true;
    script.onload = done;
    script.onerror = done;
    window.setTimeout(() => {
      if (waiting) {
        done();
      }
    }, SDK_WAIT_MS);
    document.head.appendChild(script);
  });
}

export function paintTelegram(app: WebApp, color: string): void {
  if (!/^#[0-9a-f]{6}$/i.test(color) || !app.isVersionAtLeast?.('6.9')) {
    return;
  }
  try {
    app.setHeaderColor?.(color);
    app.setBackgroundColor?.(color);
  } catch {
    return;
  }
}

export function initData(): string {
  return webApp()?.initData || launchData();
}

export function openExternal(url: string): void {
  const app = webApp();
  if (url.startsWith('https://t.me/') && app?.openTelegramLink) {
    app.openTelegramLink(url);
    return;
  }
  if (app?.openLink) {
    app.openLink(url);
    return;
  }
  window.location.href = url;
}

export function downloadExternal(url: string, name: string): void {
  const app = webApp();
  if (app?.downloadFile) {
    try {
      app.downloadFile({ url, file_name: name });
      return;
    } catch {
      openExternal(url);
      return;
    }
  }
  openExternal(url);
}

type TelegramUser = { language_code?: unknown; first_name?: unknown; username?: unknown };

function telegramUser(): TelegramUser | null {
  const data = initData();
  if (!data) {
    return null;
  }
  try {
    const user = JSON.parse(new URLSearchParams(data).get('user') ?? '{}') as TelegramUser | null;
    return user && typeof user === 'object' ? user : null;
  } catch {
    return null;
  }
}

export function telegramLanguage(): string | null {
  const code = telegramUser()?.language_code;
  return typeof code === 'string' ? code : null;
}

export function telegramName(): string | null {
  const name = telegramUser()?.first_name;
  return typeof name === 'string' && name.trim() ? name.trim().slice(0, 40) : null;
}

export function telegramUsername(): string | null {
  const username = telegramUser()?.username;
  return typeof username === 'string' && /^\w{1,32}$/.test(username) ? username : null;
}
