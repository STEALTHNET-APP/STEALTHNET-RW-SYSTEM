import { useCallback, useEffect, useState } from 'react';
import { api, MAINTENANCE_EVENT } from './api';
import { servedBrand, servedLanguages, tabTitle } from './tabTitle';
import type { Site } from './types';

const served = typeof document === 'undefined' ? null : document;

export const DEFAULT_SITE: Site = {
  brand_name: servedBrand(served),
  currency: 'USD',
  support_url: null,
  bot_username: null,
  appearance: { admin: 'side', admin_theme: 'dark', client: 'accent', client_theme: 'client' },
  languages: servedLanguages(served),
};

export type SiteData = { site: Site | null; failed: boolean; loading: boolean; retry: () => void };

export function showTitle(site: Site | null, make: (site: Site) => string): void {
  const title = tabTitle(site, DEFAULT_SITE, make);
  if (title !== null) {
    document.title = title;
  }
}

export function useSiteData(): SiteData {
  const [site, setSite] = useState<Site | null>(null);
  const [loading, setLoading] = useState(true);
  const [turn, setTurn] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    setLoading(true);
    api<Site>('/api/public/site', { signal: controller.signal })
      .then(setSite)
      .catch((err: unknown) => {
        if (!(err instanceof DOMException)) {
          setSite((known) => known ?? DEFAULT_SITE);
        }
      })
      .finally(() => {
        if (!controller.signal.aborted) {
          setLoading(false);
        }
      });
    return () => controller.abort();
  }, [turn]);
  const retry = useCallback(() => setTurn((value) => value + 1), []);
  useEffect(() => {
    window.addEventListener(MAINTENANCE_EVENT, retry);
    return () => window.removeEventListener(MAINTENANCE_EVENT, retry);
  }, [retry]);
  return { site, failed: site === DEFAULT_SITE, loading, retry };
}

export function useSite(): Site | null {
  return useSiteData().site;
}
