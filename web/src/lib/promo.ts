import type { Money } from './format';
import { t } from './i18n';

export type OfferPromotion = { id: number; name: string; percent: number; ends_at: string; full_price: Money; full_stars: number | null; until: string };
export type QuotePromotion = { id: number; name: string; percent: number; ends_at: string; until: string; applied: boolean; code_skipped: boolean };

export function promoOf(periods: ReadonlyArray<{ promotion?: OfferPromotion | null }> | undefined): OfferPromotion | null {
  return periods?.find((p) => p.promotion)?.promotion ?? null;
}

export function promoTitle(promotion: { name: string; until: string }): string {
  return `${promotion.name} ${promotion.until}`;
}

export function promoCut(percent: number): string {
  return `−${percent}%`;
}

export function promoQuoteNote(promotion: QuotePromotion | null | undefined): string | null {
  if (!promotion) {
    return null;
  }
  if (promotion.code_skipped) {
    return t('Промокод не применён: скидка по акции «{name}» больше.', { name: promotion.name });
  }
  if (!promotion.applied) {
    return t('Применён промокод: он выгоднее акции «{name}».', { name: promotion.name });
  }
  return null;
}
