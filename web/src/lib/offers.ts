type Period = { period_id: number; months: number; days: number };

export const SHORT_PERIOD_DAYS = 28;

export type OfferLike = {
  category: { id: number; sort: number };
  tariff: { id: number; devices: { included: number; extra: { max: number } | null } };
  periods: Period[];
};

export type OwnTariff = { tariff_id: number | null; period_days: number | null; extra_devices: number };

export type OfferPick = { tariffId: number; periodId: number | null; devices: number };

export type OfferLink = { tariffId?: number | null; periodId?: number | null };

export function offerCategories<T extends OfferLike>(offers: T[]): Array<T['category']> {
  const seen = new Map<number, T['category']>();
  for (const offer of offers) {
    if (!seen.has(offer.category.id)) {
      seen.set(offer.category.id, offer.category);
    }
  }
  return [...seen.values()].sort((a, b) => a.sort - b.sort);
}

export function categoryOffers<T extends OfferLike>(offers: T[], categoryId: number | null | undefined): T[] {
  return offers.filter((offer) => offer.category.id === categoryId);
}

export function flatOffers<T extends OfferLike>(offers: T[], terms: (offer: T) => string): T[] | null {
  const [first] = offers;
  if (!first || offers.length < 2 || offers.some((offer) => offer.periods.length !== 1)) {
    return null;
  }
  if (new Set(offers.map((offer) => offer.periods[0]?.days)).size !== offers.length) {
    return null;
  }
  const same = terms(first);
  if (offers.some((offer) => terms(offer) !== same)) {
    return null;
  }
  return offerCategories(offers).flatMap((category) => categoryOffers(offers, category.id));
}

export type TermsLike = {
  tariff: {
    devices: { included: number; extra: { max: number; monthly: { rub?: number; usd?: number }; monthly_stars: number | null } | null };
    traffic: { limit_bytes: number | null; reset: string };
    device_discounts?: Array<{ from: number; percent: number }>;
  };
};

export function planTerms(offer: TermsLike, currency: string): string {
  const { devices, traffic, device_discounts: discounts } = offer.tariff;
  const extra = devices.extra && devices.extra.max > 0 ? devices.extra : null;
  return JSON.stringify([
    devices.included,
    extra ? [extra.max, currency === 'RUB' ? (extra.monthly.rub ?? 0) : (extra.monthly.usd ?? 0), extra.monthly_stars, (discounts ?? []).map((d) => [d.from, d.percent])] : null,
    traffic.limit_bytes ? [traffic.limit_bytes, traffic.reset] : null,
  ]);
}

export function flatPlans<T extends OfferLike & TermsLike>(offers: T[], currency: string): T[] | null {
  return flatOffers(offers, (offer) => planTerms(offer, currency));
}

export function offerById<T extends OfferLike>(offers: T[], tariffId: number | null | undefined): T | null {
  return offers.find((offer) => offer.tariff.id === tariffId) ?? null;
}

export function renewsCurrent(current: OfferLike | null, offer: OfferLike | null): boolean | null {
  return current && offer ? current.category.id === offer.category.id : null;
}

function standard(offer: OfferLike): Period | undefined {
  return offer.periods[Math.min(1, offer.periods.length - 1)];
}

function owned(offer: OfferLike, own: OwnTariff | null): OwnTariff | null {
  return own && own.tariff_id === offer.tariff.id ? own : null;
}

function ownPeriod(offer: OfferLike, own: OwnTariff | null): Period | undefined {
  const held = owned(offer, own);
  return held ? offer.periods.find((p) => p.days === held.period_days) : undefined;
}

function sameMonths(offer: OfferLike, months: number | null): Period | undefined {
  return months === null ? undefined : offer.periods.find((p) => p.months === months);
}

function made(offer: OfferLike, period: Period | undefined, own: OwnTariff | null): OfferPick {
  const held = owned(offer, own);
  const extra = held ? Math.max(0, Math.min(held.extra_devices, offer.tariff.devices.extra?.max ?? 0)) : 0;
  return { tariffId: offer.tariff.id, periodId: period?.period_id ?? null, devices: offer.tariff.devices.included + extra };
}

export function pickedMonths(offers: OfferLike[], pick: OfferPick | null): number | null {
  const offer = offerById(offers, pick?.tariffId);
  return offer?.periods.find((p) => p.period_id === pick?.periodId)?.months ?? null;
}

export function firstPick<T extends OfferLike>(offers: T[], own: OwnTariff | null, link: OfferLink = {}): OfferPick | null {
  const offer = offerById(offers, link.tariffId) ?? offerById(offers, own?.tariff_id) ?? offers[0] ?? null;
  if (!offer) {
    return null;
  }
  const linked = offer.periods.find((p) => p.period_id === link.periodId);
  return made(offer, linked ?? ownPeriod(offer, own) ?? standard(offer), own);
}

export function flatFirst<T extends OfferLike>(flat: T[], own: OwnTariff | null, link: OfferLink = {}): OfferPick | null {
  const offer = offerById(flat, link.tariffId) ?? offerById(flat, own?.tariff_id) ?? flat[Math.min(1, flat.length - 1)];
  return offer ? made(offer, offer.periods[0], own) : null;
}

export function flatTerm<T extends OfferLike>(flat: T[], tariffId: number, current: OfferPick | null): OfferPick | null {
  const offer = offerById(flat, tariffId);
  if (!offer) {
    return current;
  }
  const { included, extra } = offer.tariff.devices;
  const devices = Math.min(Math.max(current?.devices ?? included, included), included + (extra?.max ?? 0));
  return { tariffId: offer.tariff.id, periodId: offer.periods[0]?.period_id ?? null, devices };
}

export function targetPick<T extends OfferLike>(offers: T[], own: OwnTariff | null, current: OfferPick | null): OfferPick | null {
  const offer = offerById(offers, own?.tariff_id);
  if (offer) {
    return made(offer, ownPeriod(offer, own) ?? sameMonths(offer, pickedMonths(offers, current)) ?? standard(offer), own);
  }
  const kept = offerById(offers, current?.tariffId);
  if (!kept) {
    return firstPick(offers, own);
  }
  return made(kept, kept.periods.find((p) => p.period_id === current?.periodId) ?? standard(kept), own);
}

function switchTo(offer: OfferLike, own: OwnTariff | null, months: number | null): OfferPick {
  return made(offer, sameMonths(offer, months) ?? ownPeriod(offer, own) ?? standard(offer), own);
}

export function categoryPick<T extends OfferLike>(offers: T[], categoryId: number, own: OwnTariff | null, current: OfferPick | null): OfferPick | null {
  if (current && offerById(offers, current.tariffId)?.category.id === categoryId) {
    return current;
  }
  const inside = categoryOffers(offers, categoryId);
  const offer = inside.find((o) => o.tariff.id === own?.tariff_id) ?? inside[0];
  return offer ? switchTo(offer, own, pickedMonths(offers, current)) : current;
}

export function tariffPick<T extends OfferLike>(offers: T[], tariffId: number, own: OwnTariff | null, current: OfferPick | null): OfferPick | null {
  if (current?.tariffId === tariffId) {
    return current;
  }
  const offer = offerById(offers, tariffId);
  return offer ? switchTo(offer, own, pickedMonths(offers, current)) : current;
}
