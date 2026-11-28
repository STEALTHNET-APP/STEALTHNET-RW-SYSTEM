const DAY_MS = 86_400_000;

export type Removal = { until: string; ends: boolean };

export function removedUntil(until: string | null, days: number, now: number): Removal | null {
  const end = until ? Date.parse(until) : Number.NaN;
  if (!Number.isFinite(end) || end <= now || days <= 0) {
    return null;
  }
  const next = end - days * DAY_MS;
  return next <= now ? { until: new Date(now).toISOString(), ends: true } : { until: new Date(next).toISOString(), ends: false };
}
