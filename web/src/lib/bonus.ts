export type BonusTerms = { percent: number; min?: { minor: number } | null; max?: { minor: number } | null };

export function bonusReached(minor: number, terms: BonusTerms | null | undefined): boolean {
  return !!terms && minor > 0 && minor >= (terms.min?.minor ?? 0);
}

export function bonusOf(minor: number, terms: BonusTerms | null | undefined): number {
  if (!terms || !bonusReached(minor, terms)) {
    return 0;
  }
  const share = Math.floor((minor * terms.percent) / 100);
  return Math.max(0, terms.max ? Math.min(share, terms.max.minor) : share);
}

export function bonusTaken(left: number, remaining: number, refund: number): number {
  if (left <= 0 || refund <= 0) {
    return 0;
  }
  return refund >= remaining ? left : left - Math.floor((left * (remaining - refund)) / remaining);
}
