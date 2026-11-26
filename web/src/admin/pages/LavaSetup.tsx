import { Fragment, useCallback, useEffect, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { dateLong, money, plural } from '../../lib/format';
import type { Currency } from '../../lib/types';
import type { Tariff, View } from '../tariffs/model';

export type LavaPlan = {
  product: string | null;
  offer: string;
  periodicity: string;
  name: string;
  amount: number;
  currency: Currency;
  tariff: number;
  tariff_period: number;
};

type CatalogPrice = { periodicity: string; every: string; min_days: number; max_days: number; amount: number; currency: Currency };
type CatalogOffer = { id: string; name: string; prices: CatalogPrice[] };
type CatalogProduct = { product: string; name: string; offers: CatalogOffer[] };
type Catalog = { subscriptions: CatalogProduct[] };
type Stats = { active: number; cancelling: number; next_charge_at: string | null; old_price: number };

const EVERY: Record<string, string> = {
  MONTHLY: 'каждый месяц',
  PERIOD_90_DAYS: 'раз в 90 дней',
  PERIOD_180_DAYS: 'раз в 180 дней',
  PERIOD_YEAR: 'раз в год',
};

function record(item: unknown): Record<string, unknown> | null {
  return item && typeof item === 'object' ? (item as Record<string, unknown>) : null;
}

function isPlan(item: unknown): item is LavaPlan {
  const p = record(item);
  return (
    !!p &&
    typeof p.offer === 'string' &&
    typeof p.periodicity === 'string' &&
    typeof p.amount === 'number' &&
    (p.currency === 'USD' || p.currency === 'RUB') &&
    typeof p.tariff === 'number' &&
    typeof p.tariff_period === 'number'
  );
}

export function lavaPlans(settings: Record<string, unknown>): LavaPlan[] {
  const list = Array.isArray(settings.plans) ? (settings.plans as unknown[]) : [];
  return list.filter(isPlan).map((p) => ({ ...p, product: typeof p.product === 'string' ? p.product : null, name: typeof p.name === 'string' ? p.name : '' }));
}

function capital(text: string): string {
  return text.charAt(0).toUpperCase() + text.slice(1);
}

function daysText(days: number): string {
  return `${days} ${plural(days, 'день', 'дня', 'дней')}`;
}

function unboundText(count: number): string {
  return `Ещё ${count} ${plural(count, 'подписка Lava.top не привязана', 'подписки Lava.top не привязаны', 'подписок Lava.top не привязаны')}`;
}

function needText(price: CatalogPrice): string {
  return price.min_days === price.max_days
    ? daysText(price.min_days)
    : `${price.min_days}–${price.max_days} ${plural(price.max_days, 'день', 'дня', 'дней')}`;
}

function priceOf(tariff: Tariff | undefined, period: number, currency: Currency): number | null {
  const found = tariff?.periods.find((p) => p.id === period);
  if (!found) {
    return null;
  }
  return currency === 'RUB' ? found.prices.rub : found.prices.usd;
}

type Props = {
  value: LavaPlan[];
  onChange: (next: LavaPlan[]) => void;
  busy: boolean;
  connected: boolean;
  currency: Currency;
  switchedOff: boolean;
};

type Row = { product: CatalogProduct; offer: CatalogOffer; price: CatalogPrice };

export function LavaSetup({ value, onChange, busy, connected, currency, switchedOff }: Props) {
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [tariffs, setTariffs] = useState<Tariff[] | null>(null);
  const [stats, setStats] = useState<Stats | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let alive = true;
    api<View>('/api/admin/tariffs')
      .then((view) => alive && setTariffs(view.tariffs))
      .catch(() => alive && setTariffs([]));
    api<Stats>('/api/admin/payments/providers/lava/renewals')
      .then((found) => alive && setStats(found))
      .catch(() => alive && setStats(null));
    return () => {
      alive = false;
    };
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      setCatalog(await api<Catalog>('/api/admin/payments/providers/lava/catalog'));
    } catch (err) {
      setError(err instanceof ApiError ? err.message : 'Список из Lava.top не загрузился');
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    if (connected) {
      void load();
    }
  }, [connected, load]);

  const byId = useMemo(() => new Map((tariffs ?? []).map((t) => [t.id, t])), [tariffs]);
  const options = (price: CatalogPrice) =>
    (tariffs ?? []).flatMap((t) =>
      t.periods
        .filter((p) => p.active && p.days >= price.min_days && p.days <= price.max_days)
        .map((p) => ({ value: `${t.id}:${p.id}`, label: `${t.name} · ${daysText(p.days)}` })),
    );
  const same = (plan: LavaPlan, offer: string, price: CatalogPrice) => plan.offer === offer && plan.periodicity === price.periodicity && plan.currency === price.currency;
  const bind = (product: CatalogProduct, offer: CatalogOffer, price: CatalogPrice, choice: string) => {
    const rest = value.filter((p) => !same(p, offer.id, price));
    const [tariff, tariffPeriod] = choice.split(':').map(Number);
    if (!choice || !tariff || !tariffPeriod) {
      onChange(rest);
      return;
    }
    const name = offer.name && offer.name !== product.name ? `${product.name} · ${offer.name}` : product.name;
    onChange([
      ...rest.filter((p) => !(p.tariff === tariff && p.tariff_period === tariffPeriod && p.currency === price.currency)),
      { product: product.product, offer: offer.id, periodicity: price.periodicity, name, amount: price.amount, currency: price.currency, tariff, tariff_period: tariffPeriod },
    ]);
  };
  const drop = (plan: LavaPlan) => onChange(value.filter((p) => p !== plan));
  const status = (plan: LavaPlan | undefined, amount: number) => {
    if (!plan) {
      return null;
    }
    const panel = priceOf(byId.get(plan.tariff), plan.tariff_period, plan.currency);
    if (panel === null) {
      return <span className="warn-line"> · срока больше нет в тарифах</span>;
    }
    return panel === amount ? (
      <span style={{ color: 'var(--ok)' }}> · совпадает с ценой срока</span>
    ) : (
      <span className="warn-line"> · в панели {money({ minor: panel, currency: plan.currency })}, кнопки нет</span>
    );
  };
  const rows: Row[] = (catalog?.subscriptions ?? []).flatMap((product) =>
    product.offers.flatMap((offer) => offer.prices.filter((price) => price.currency === currency).map((price) => ({ product, offer, price }))),
  );
  const isBound = ({ offer, price }: Row) => value.some((p) => same(p, offer.id, price));
  const boundRows = rows.filter(isBound);
  const freeRows = rows.filter((row) => !isBound(row));
  const line = ({ product, offer, price }: Row) => {
    const plan = value.find((p) => same(p, offer.id, price));
    const name = offer.name && offer.name !== product.name ? `${product.name} · ${offer.name}` : product.name;
    const choices = options(price);
    return (
      <Fragment key={`${offer.id}:${price.periodicity}`}>
        <div className="pay-mrow">
          <span className="pay-mname">{name || 'Подписка'}</span>
          <span className="pay-mhint muted">
            {capital(EVERY[price.periodicity] ?? price.every)} · {money({ minor: price.amount, currency: price.currency })}
            {status(plan, price.amount)}
          </span>
          <select
            className="field pay-mtitle"
            value={plan ? `${plan.tariff}:${plan.tariff_period}` : ''}
            onChange={(e) => bind(product, offer, price, e.target.value)}
            disabled={busy || tariffs === null}
            aria-label={`Какой срок продлевает «${name}», ${EVERY[price.periodicity] ?? price.every}`}
          >
            <option value="">Не привязана</option>
            {choices.map((o) => (
              <option key={o.value} value={o.value}>
                {o.label}
              </option>
            ))}
          </select>
        </div>
        {tariffs !== null && choices.length === 0 ? <span className="cap">Нужен срок {needText(price)}</span> : null}
      </Fragment>
    );
  };
  const known = new Set(rows.map(({ offer, price }) => `${offer.id}:${price.periodicity}:${price.currency}`));
  const stale = catalog ? value.filter((p) => p.currency === currency && !known.has(`${p.offer}:${p.periodicity}:${p.currency}`)) : [];
  const label = (plan: LavaPlan) => {
    const t = byId.get(plan.tariff);
    const p = t?.periods.find((x) => x.id === plan.tariff_period);
    return t && p ? `${t.name} · ${daysText(p.days)}` : 'срок удалён';
  };

  return (
    <div className="sf-field" style={{ maxWidth: 'none' }} role="group" aria-labelledby="lava-plans">
      <span className="sf-label" id="lava-plans">
        Подписки Lava.top
      </span>
      <span className="cap">
        Привяжите подписку Lava.top к сроку тарифа. Цена в Lava.top должна совпадать с ценой срока, иначе у этого срока кнопки автопродления нет. Lava.top не меняет цену уже
        оформленных подписок: новая цена действует только для новых.
      </span>
      {switchedOff && value.length > 0 ? (
        <span className="cap warn-line">«Автопродление через Lava.top» выключено: кнопки «Карта с автопродлением» у клиентов не будет, пока переключатель выключен.</span>
      ) : null}
      {!connected ? <span className="cap">Сохраните API-ключ, и панель загрузит список подписок из Lava.top.</span> : null}
      {catalog ? (
        boundRows.length === 0 ? (
          rows.map(line)
        ) : (
          <>
            {boundRows.map(line)}
            {freeRows.length > 0 ? (
              <details className="pay-more">
                <summary>
                  {unboundText(freeRows.length)}
                  <span className="muted"> · ни на что не влияют</span>
                </summary>
                {freeRows.map(line)}
              </details>
            ) : null}
          </>
        )
      ) : (
        value
            .filter((p) => p.currency === currency)
            .map((p) => (
              <div key={`${p.offer}:${p.periodicity}`} className="pay-mrow">
                <span className="pay-mname">{p.name || 'Подписка'}</span>
                <span className="pay-mhint muted">
                  {capital(EVERY[p.periodicity] ?? p.periodicity)} · {money({ minor: p.amount, currency: p.currency })} · {label(p)}
                </span>
              </div>
            ))
      )}
      {catalog && rows.length === 0 ? <span className="cap">В Lava.top пока нет подписок с ценой в этой валюте.</span> : null}
      {stale.map((p) => (
        <div key={`${p.offer}:${p.periodicity}`} className="pay-mrow">
          <span className="pay-mname">{p.name || 'Подписка'}</span>
          <span className="pay-mhint muted">
            {label(p)} · этой подписки больше нет в Lava.top
          </span>
          <button type="button" className="b34 hov" onClick={() => drop(p)} disabled={busy}>
            Убрать
          </button>
        </div>
      ))}
      {connected ? (
        <span className="pay-lrow">
          <button type="button" className="btn-line hov" onClick={() => void load()} disabled={busy || loading} style={{ padding: '0 16px' }}>
            {loading ? 'Загружаем…' : 'Обновить список из Lava.top'}
          </button>
        </span>
      ) : null}
      {stats && stats.active + stats.cancelling > 0 ? (
        <span className="cap">
          Подписок с автопродлением Lava.top сейчас: {stats.active + stats.cancelling}
          {stats.next_charge_at ? `, ближайшее списание около ${dateLong(stats.next_charge_at)}` : ''}.
          {stats.old_price > 0 ? ` Списываются не по цене срока: ${stats.old_price}. Lava.top не меняет цену уже оформленным подпискам.` : ''}
        </span>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
