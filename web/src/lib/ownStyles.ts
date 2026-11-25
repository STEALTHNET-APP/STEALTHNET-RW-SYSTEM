import { useLayoutEffect, useState } from 'react';

const WAIT_MS = 2500;

export function styleFile(value: unknown, folder: 'landing' | 'miniapp' | 'cabinet'): string | null {
  return typeof value === 'string' && new RegExp(`^/api/public/${folder}/[0-9a-f]{16}\\.css$`).test(value) ? value : null;
}

export function useOwnStyles(href: string | null): boolean {
  const [settled, setSettled] = useState<string | null>(null);
  useLayoutEffect(() => {
    if (!href) {
      return;
    }
    const link = document.createElement('link');
    const settle = () => setSettled(href);
    link.rel = 'stylesheet';
    link.href = href;
    link.addEventListener('load', settle);
    link.addEventListener('error', settle);
    const timer = window.setTimeout(settle, WAIT_MS);
    document.head.append(link);
    return () => {
      window.clearTimeout(timer);
      link.remove();
    };
  }, [href]);
  return href === null || settled === href;
}

export function usePreviewStyles(css: string): void {
  useLayoutEffect(() => {
    if (!css || !('adoptedStyleSheets' in document)) {
      return;
    }
    const sheet = new CSSStyleSheet();
    sheet.replaceSync(css);
    document.adoptedStyleSheets = [...document.adoptedStyleSheets, sheet];
    return () => {
      document.adoptedStyleSheets = document.adoptedStyleSheets.filter((other) => other !== sheet);
    };
  }, [css]);
}
