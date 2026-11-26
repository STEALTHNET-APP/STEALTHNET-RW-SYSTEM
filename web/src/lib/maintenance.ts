import { useEffect } from 'react';
import { api } from './api';
import { lang, locale, t } from './i18n';
import type { Maintenance, Site } from './types';

const WATCH_MS = 60_000;

export function maintenanceOf(site: Site | null): Maintenance | null {
  return site?.maintenance ?? null;
}

export function maintenanceText(mode: Maintenance): string {
  return lang() === 'en' ? mode.message.en : mode.message.ru;
}

export function untilText(until: string | null, now: number = Date.now()): string | null {
  if (!until) {
    return null;
  }
  const at = new Date(until);
  if (Number.isNaN(at.getTime()) || at.getTime() <= now) {
    return null;
  }
  const sameYear = at.getFullYear() === new Date(now).getFullYear();
  const when = new Intl.DateTimeFormat(locale(), {
    day: 'numeric',
    month: 'long',
    year: sameYear ? undefined : 'numeric',
    hour: '2-digit',
    minute: '2-digit',
  })
    .format(at)
    .replace(/\s/g, '\u00a0');
  return t('Закончим примерно {when}', { when });
}

export function useMaintenanceWatch(on: boolean): void {
  useEffect(() => {
    if (!on) {
      return;
    }
    const timer = window.setInterval(() => {
      api<Site>('/api/public/site')
        .then((site) => {
          if (!site.maintenance) {
            window.location.reload();
          }
        })
        .catch(() => undefined);
    }, WATCH_MS);
    return () => window.clearInterval(timer);
  }, [on]);
}
