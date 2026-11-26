import { useEffect, useMemo, useRef, useState } from 'react';
import { useLocation, useNavigate } from 'react-router';
import { api, ApiError } from '../lib/api';
import { count, days as dayCount, money, MINUS, type Money } from '../lib/format';
import { t } from '../lib/i18n';
import { gbText, ladderText, linesOf, resetPlan, stars, transitionText, type Catalog, type CheckoutMethod, type Quote, type TariffOffer } from '../cabinet/model';
import { checkPromo, handedCode, promoFailText } from '../cabinet/promo';
import { fullCategory, fullNote, NEWEST, newTargetNote, payTargets, remember, resolveTarget, subName, subState, targetView, type PayTarget } from '../cabinet/subs';
import { BackHead, MINI_SUB_KEY, useMini } from './MiniApp';
import { openExternal, webApp } from './telegram';
import { methodKey } from '../lib/methods';
import { promoCut, promoOf, promoQuoteNote, promoTitle } from '../lib/promo';
import { categoryOffers, categoryPick, firstPick, flatFirst, flatOffers, flatTerm, offerById, offerCategories, planTerms, renewsCurrent, SHORT_PERIOD_DAYS, tariffPick, targetPick, type OfferPick } from '../lib/offers';
import { ReceiptEmail, useReceiptEmail } from '../lib/receiptEmail';

const PAY_STEP = 'pay';
const TAP_GUARD_MS = 450;

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function termName(months: number, daysCount: number): string {
  return daysCount >= 28 ? count(months, 'месяц', 'месяца', 'месяцев') : count(daysCount, 'день', 'дня', 'дней');
}

function payLabel(transition: boolean, total: string, telegram: boolean): string {
  if (transition) {
    return telegram ? t('Перейти и оплатить {amount} в Telegram', { amount: total }) : t('Перейти и оплатить {amount}', { amount: total });
  }
  return telegram ? t('Оплатить {amount} в Telegram', { amount: total }) : t('Оплатить {amount}', { amount: total });
}

function fromTerms(state: unknown): boolean {
  return typeof state === 'object' && state !== null && (state as { terms?: unknown }).terms === true;
}

function methodStars(m: CheckoutMethod): number | null {
  const a = m.amount;
  if (typeof a === 'number') {
    return a;
  }
  return a && typeof a === 'object' && typeof a.stars === 'number' ? a.stars : null;
}

function methodNote(m: CheckoutMethod, balance: Money): string {
  if (m.unavailable?.type === 'insufficient_balance') {
    return t('Не хватает: на балансе {amount}', { amount: money(m.unavailable.balance) });
  }
  if (m.unavailable?.type === 'no_stars_price') {
    return t('У этого срока нет цены в звёздах');
  }
  if (m.kind === 'balance') {
    return t('На балансе {amount}', { amount: money(balance) });
  }
  const amount = m.kind === 'stars' ? methodStars(m) : null;
  return amount !== null ? t('{stars}, оплата в Telegram', { stars: stars(amount) }) : m.hint;
}

export function MiniPlans() {
  const { me, subs, view: shown, choose, reload } = useMini();
  const receipt = useReceiptEmail(me);
  const location = useLocation();
  const navigate = useNavigate();
  const paying = new URLSearchParams(location.search).get('step') === PAY_STEP;
  const multi = me.multi_subscriptions === true;
  const [picked, setPicked] = useState<PayTarget | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [tariffId, setTariffId] = useState<number | null>(null);
  const chosenOffer = offerById(catalog?.tariffs ?? [], tariffId);
  const full = useMemo(() => fullCategory(subs, multi, chosenOffer?.category), [subs, multi, chosenOffer]);
  const targets = useMemo(() => payTargets(subs, multi, full), [subs, multi, full]);
  const wanted = picked ?? shown?.subscription.id ?? null;
  const target = full ? (wanted !== null && targets.includes(wanted) ? wanted : (targets[0] ?? 'new')) : resolveTarget(targets, subs, multi, wanted);
  const view = targetView(subs, target, multi);
  const [period, setPeriod] = useState<number | null>(null);
  const [devices, setDevices] = useState(0);
  const promoOn = me.promo_codes === true;
  const [handed] = useState(() => (promoOn ? handedCode(location.state) : null));
  const [promoOpen, setPromoOpen] = useState(handed !== null);
  const [promoInput, setPromoInput] = useState(handed ?? '');
  const [promo, setPromo] = useState<string | null>(handed);
  const [promoError, setPromoError] = useState<string | null>(null);
  const [promoDays, setPromoDays] = useState<{ code: string; days: number } | null>(null);
  const [promoBusy, setPromoBusy] = useState(false);
  const [quote, setQuote] = useState<Quote | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [otherWallet, setOtherWallet] = useState<string | null>(null);
  const seq = useRef(0);
  const armed = useRef(0);
  const reopened = useRef(paying);
  const scrolled = useRef(false);

  useEffect(() => {
    api<Catalog>('/api/client/catalog')
      .then(setCatalog)
      .catch((err) => setError(failText(err, t('Тарифы не загрузились'))));
  }, []);

  const offers = catalog?.tariffs ?? [];
  const categories = useMemo(() => offerCategories(offers), [offers]);
  const siblings = useMemo(() => categoryOffers(offers, chosenOffer?.category.id), [offers, chosenOffer]);
  const flat = useMemo(() => (catalog ? flatOffers(catalog.tariffs, (o: TariffOffer) => planTerms(o, catalog.currency)) : null), [catalog]);
  const current = view ? offerById(offers, view.subscription.tariff_id) : null;
  const own = view?.subscription ?? null;

  useEffect(() => {
    if (!catalog || tariffId !== null) {
      return;
    }
    const held = view?.subscription ?? null;
    const start = flat ? flatFirst(flat, held) : firstPick(catalog.tariffs, held);
    if (!start) {
      return;
    }
    setTariffId(start.tariffId);
    setPeriod(start.periodId);
    setDevices(start.devices);
  }, [catalog, flat, tariffId, view]);

  const toTerms = () => {
    if (fromTerms(location.state)) {
      navigate(-1);
      return;
    }
    navigate({ pathname: location.pathname, search: '' }, { replace: true });
  };

  const toPay = () => {
    setError(null);
    setNote(null);
    setOtherWallet(null);
    armed.current = performance.now() + TAP_GUARD_MS;
    navigate({ pathname: location.pathname, search: `?step=${PAY_STEP}` }, { state: { terms: true } });
  };

  useEffect(() => {
    if (reopened.current) {
      reopened.current = false;
      toTerms();
    }
  }, []);

  useEffect(() => {
    if (!scrolled.current) {
      scrolled.current = true;
      return;
    }
    window.scrollTo(0, 0);
  }, [paying]);

  const offer: TariffOffer | null = chosenOffer;
  const included = offer?.tariff.devices.included ?? 0;
  const extra = offer?.tariff.devices.extra ?? null;
  const maxDevices = included + (extra?.max ?? 0);
  const periodOffer = offer?.periods.find((p) => p.period_id === period) ?? null;
  const subscriptionId = typeof target === 'number' ? target : null;
  const another = !view && subs.length > 0;
  const choice: OfferPick | null = tariffId === null ? null : { tariffId, periodId: period, devices };

  const apply = (next: OfferPick | null) => {
    if (!next) {
      return;
    }
    setTariffId(next.tariffId);
    setPeriod(next.periodId);
    setDevices(next.devices);
  };

  const forced = full && typeof target === 'number' && target !== wanted ? target : null;
  useEffect(() => {
    if (forced === null || !chosenOffer) {
      return;
    }
    const held = subs.find((v) => v.subscription.id === forced);
    if (held && held.subscription.tariff_id === chosenOffer.tariff.id) {
      setDevices(chosenOffer.tariff.devices.included + held.subscription.extra_devices);
    }
  }, [forced, chosenOffer, subs]);

  const pickTarget = (next: PayTarget) => {
    if (next === target) {
      return;
    }
    setPicked(next);
    apply(targetPick(offers, targetView(subs, next, multi)?.subscription ?? null, choice));
    if (typeof next === 'number') {
      choose(next);
    }
  };

  useEffect(() => {
    if (!offer || !periodOffer) {
      return;
    }
    const mine = ++seq.current;
    const timer = window.setTimeout(async () => {
      try {
        const q = await api<Quote>('/api/client/quote', {
          method: 'POST',
          body: { tariff_id: offer.tariff.id, period_id: periodOffer.period_id, extra_devices: Math.max(0, devices - included), subscription_id: subscriptionId, promo_code: promo },
        });
        if (mine === seq.current) {
          setQuote(q);
          setPromoError(null);
        }
      } catch (err) {
        if (mine !== seq.current) {
          return;
        }
        if (promo) {
          setPromoError(failText(err, t('Промокод не подошёл')));
          setPromo(null);
          return;
        }
        setError(failText(err, t('Сумма не посчиталась')));
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [offer, periodOffer, devices, included, subscriptionId, promo]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => methodKey(m) === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';
  const inTelegramWallet = chosen?.provider === 'gram' && webApp() !== null;
  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? money(q.total) : '';
  const fromTrial = !!view && view.subscription.kind === 'trial';
  const sameCategory = renewsCurrent(current, offer);
  const switching = sameCategory === true && !fromTrial && !!current && !!offer && current.tariff.id !== offer.tariff.id;
  const change = !!view && !!offer && (fromTrial || (sameCategory === null ? quote?.kind === 'change' : !sameCategory));
  const transition = change && !fromTrial;
  const currentName = view && subs.length > 1 ? subName(view, subs) : (current?.tariff.name ?? view?.tariff?.name ?? null);
  const limitBytes = offer?.tariff.traffic.limit_bytes ?? 0;
  const extraMonthly: Money | null = extra && catalog ? { minor: catalog.currency === 'RUB' ? (extra.monthly.rub ?? 0) : (extra.monthly.usd ?? 0), currency: catalog.currency } : null;
  const limitNote = fullNote(full, offer?.category);
  const ladder = ladderText(offer?.tariff.device_discounts);
  const deviceCut = q ? (linesOf(q.lines, 'device_discount')[0] ?? null) : null;
  const promoLines = q ? linesOf(q.lines, 'adjustment') : [];
  const saleLines = q ? linesOf(q.lines, 'promotion') : [];
  const quoteNote = promoQuoteNote(quote?.promotion);
  const rows = flat ? flat.flatMap((o) => o.periods.slice(0, 1).map((p) => ({ offer: o, period: p }))) : offer ? offer.periods.map((p) => ({ offer, period: p })) : [];
  const offerPromo = promoOf(rows.map((row) => row.period));
  const promoMinor = promoLines.reduce((sum, line) => sum - line.amount.minor, 0);
  const promoStars = promoLines.reduce((sum, line) => sum - (line.stars ?? 0), 0);
  const promoCutShown = !!q && (starsMode ? promoStars > 0 : promoMinor > 0);
  const empty = !!catalog && offers.length === 0;
  const anyDevices = offers.some((o) => (o.tariff.devices.extra?.max ?? 0) > 0);
  const named = !flat && (categories.length > 1 || siblings.length > 1);
  const renewing = typeof target === 'number' ? targetView(subs, target, multi) : null;
  const summary = [named && offer ? offer.tariff.name : null, periodOffer ? termName(periodOffer.months, periodOffer.days) : null, count(devices, 'устройство', 'устройства', 'устройств'), totalText || null]
    .filter((part): part is string => !!part)
    .join(' · ');
  const title = paying ? t('Оплата') : empty ? t('Тарифы') : !catalog || anyDevices ? t('Срок и устройства') : t('Срок подписки');

  const applyPromo = async () => {
    const code = promoInput.trim().toUpperCase();
    if (!code || promoBusy) {
      return;
    }
    setPromoBusy(true);
    setPromoError(null);
    setPromoDays(null);
    try {
      const found = await checkPromo(code, offer?.tariff.id ?? null);
      if (found.type === 'days') {
        setPromoDays({ code: found.code, days: found.days });
        return;
      }
      setPromoInput(found.code);
      setPromo(found.code);
    } catch (err) {
      setPromoError(promoFailText(err, true));
    } finally {
      setPromoBusy(false);
    }
  };

  const pay = async () => {
    if (!offer || !periodOffer || !chosen || performance.now() < armed.current) {
      return;
    }
    const email = receipt.take(chosen);
    if (email === false) {
      return;
    }
    setBusy(true);
    setError(null);
    setOtherWallet(null);
    if (another) {
      remember(MINI_SUB_KEY, NEWEST);
    }
    try {
      const res = await api<{ type: 'redirect'; payment_id: string; pay_url: string } | { type: 'completed'; order_id: string }>('/api/client/checkout', {
        method: 'POST',
        body: {
          email: email ?? undefined,
          cart: { tariff_id: offer.tariff.id, period_id: periodOffer.period_id, extra_devices: Math.max(0, devices - included), subscription_id: subscriptionId, promo_code: promo },
          provider: chosen.provider,
          method: chosen.method ?? undefined,
        },
      });
      if (res.type === 'redirect') {
        if (inTelegramWallet) {
          try {
            const sent = await api<{ chat_url: string }>(`/api/client/payments/${res.payment_id}/telegram`, { method: 'POST' });
            setOtherWallet(res.pay_url);
            setNote(t('Счёт отправлен в чат с ботом. Нажмите там «Оплатить в Telegram» — кошелёк откроется с готовой суммой.'));
            openExternal(sent.chat_url);
            return;
          } catch (err) {
            setError(failText(err, t('Счёт в чат не отправился, открываем другой кошелёк')));
          }
        }
        openExternal(res.pay_url);
        setNote(t('Оплата открылась в новом окне. Когда она пройдёт, подписка обновится сама.'));
        return;
      }
      setNote(t('Оплачено. Обновляем подписку…'));
      window.setTimeout(() => void reload(), 2000);
    } catch (err) {
      if (!receipt.refused(err)) {
        setError(failText(err, t('Оплата не началась')));
      }
    } finally {
      setBusy(false);
    }
  };

  if (paying) {
    return (
      <main className="mn-page plans">
        <BackHead title={title} sub={t('Шаг {n} из {total}', { n: 2, total: 2 })} onBack={toTerms} />
        {offer && periodOffer ? (
          <button type="button" className="mn-card mn-picked press" onClick={toTerms}>
            <span className="mn-picked-text">
              <strong className="num">{summary}</strong>
              {targets.length > 1 ? <span>{renewing ? t('Продлить «{name}»', { name: subName(renewing, subs) }) : t('Новая подписка')}</span> : null}
            </span>
            <span className="mn-picked-edit">{t('Изменить')}</span>
          </button>
        ) : null}
        {saleLines.length > 0 || quoteNote || promoCutShown ? (
          <div className="mn-notes">
            {saleLines.map((line) => (
              <span key={line.label}>
                {line.label}: {MINUS}
                {starsMode && line.stars !== null ? stars(Math.abs(line.stars)) : money({ minor: Math.abs(line.amount.minor), currency: line.amount.currency })}
              </span>
            ))}
            {quoteNote ? <span>{quoteNote}</span> : null}
            {promoCutShown && q ? (
              <span>{t('Скидка по промокоду: {amount}', { amount: `${MINUS}${starsMode ? stars(promoStars) : money({ minor: promoMinor, currency: q.currency })}` })}</span>
            ) : null}
          </div>
        ) : null}
        <div className="mn-pay">
          <div className="mn-pay-head">
            <h2>{t('Способ оплаты')}</h2>
          </div>
          {q && methods.length === 0 ? (
            <p className="mn-muted small">
              {t('В мини-приложении оплатить сейчас нельзя.')}{' '}
              <button type="button" className="mn-link press" onClick={() => openExternal(`${window.location.origin}/cabinet/pay`)}>
                {t('Оплатить в кабинете на сайте')}
              </button>
            </p>
          ) : null}
          {methods.length > 0 ? (
            <div role="radiogroup" aria-label={t('Способ оплаты')} className="mn-terms">
              {methods.map((m) => {
                const hint = methodNote(m, me.balance);
                return (
                  <button key={methodKey(m)} type="button" role="radio" aria-checked={chosen === m} className="mn-term mn-method press" disabled={!!m.unavailable} onClick={() => setMethod(methodKey(m))}>
                    <span className="mn-dot" aria-hidden="true" />
                    <span className="mn-term-text">
                      <strong className="mn-method-name">{m.title}</strong>
                      {hint ? <span className="mn-term-sub">{hint}</span> : null}
                    </span>
                  </button>
                );
              })}
            </div>
          ) : null}
        </div>
        {promoOn || promo || promoError || promoDays ? (
          <div className="mn-pay">
            {promoOn || promo ? (
              <button type="button" className="mn-link mn-promo-toggle" aria-expanded={promoOpen} onClick={() => setPromoOpen(!promoOpen)}>
                {promo ? t('Промокод {code}', { code: promo }) : t('Есть промокод?')}
              </button>
            ) : null}
            {promoOpen && (promoOn || promo) ? (
              <form
                className="mn-promo"
                onSubmit={(e) => {
                  e.preventDefault();
                  if (promo) {
                    setPromo(null);
                    setPromoInput('');
                    return;
                  }
                  void applyPromo();
                }}
              >
                <input
                  value={promoInput}
                  onChange={(e) => {
                    setPromoInput(e.target.value.toUpperCase());
                    setPromoDays(null);
                  }}
                  placeholder={t('Промокод')}
                  aria-label={t('Промокод')}
                  disabled={!!promo || promoBusy}
                />
                <button type="submit" className="mn-btn2 press" disabled={promoBusy}>
                  {promo ? t('Убрать') : promoBusy ? t('Проверяем…') : t('Применить')}
                </button>
              </form>
            ) : null}
            {promoError ? <p className="mn-error">{promoError}</p> : null}
            {promoDays ? (
              <p className="mn-note" role="status">
                {t('Промокод {code} добавляет {days} к подписке, при оплате он не нужен.', { code: promoDays.code, days: dayCount(promoDays.days) })}{' '}
                <button type="button" className="mn-link press" onClick={() => navigate('/app/promo', { state: { promo: promoDays.code } })}>
                  {t('Добавить дни')}
                </button>
              </p>
            ) : null}
          </div>
        ) : null}
        {receipt.asks(chosen) ? <ReceiptEmail value={receipt.email} onChange={receipt.change} error={receipt.error} disabled={busy} mini /> : null}
        {error && catalog ? <p className="mn-error">{error}</p> : null}
        {note ? (
          <p role="status" className="mn-note">
            {note}
            {otherWallet ? (
              <>
                {' '}
                <button type="button" className="mn-link press" onClick={() => openExternal(otherWallet)}>
                  {t('Оплатить другим кошельком')}
                </button>
              </>
            ) : null}
          </p>
        ) : null}
        <button type="button" className="mn-main mn-paybtn press" onClick={() => void pay()} disabled={busy || !chosen || !q}>
          {busy ? t('Открываем оплату…') : payLabel(transition, totalText, inTelegramWallet)}
        </button>
      </main>
    );
  }

  return (
    <main className="mn-page plans">
      <BackHead title={title} sub={empty ? undefined : t('Шаг {n} из {total}', { n: 1, total: 2 })} />
      {targets.length > 1 ? (
        <section aria-label={t('Что оплачиваем')} className="mn-pay">
          <div className="mn-pay-head">
            <h2>{t('Что оплачиваем')}</h2>
          </div>
          <div role="radiogroup" aria-label={t('Что оплачиваем')} className="mn-terms">
            {targets.map((item) => {
              const v = typeof item === 'number' ? targetView(subs, item, multi) : null;
              const state = v ? subState(v, me.client.status === 'blocked') : null;
              return (
                <button key={String(item)} type="button" role="radio" aria-checked={target === item} className="mn-term press" onClick={() => pickTarget(item)}>
                  <span className="mn-dot" aria-hidden="true" />
                  <span className="mn-term-text">
                    <strong className="mn-target-name">{v ? t('Продлить «{name}»', { name: subName(v, subs) }) : t('Новая подписка')}</strong>
                    <span className={`mn-term-sub ${state?.tone ?? ''}`}>{state ? state.text : newTargetNote(subs, multi)}</span>
                  </span>
                </button>
              );
            })}
          </div>
        </section>
      ) : null}
      {!catalog ? <p className="mn-muted">{error ?? t('Загружаем тарифы…')}</p> : null}
      {empty ? <p className="mn-muted">{t('Тарифов пока нет.')}</p> : null}
      {!flat && categories.length > 1 ? (
        <div role="group" aria-label={t('Тип тарифа')} className={categories.length > 3 ? 'mn-seg flow many' : 'mn-seg flow'}>
          {categories.map((c) => (
            <button key={c.id} type="button" aria-pressed={offer?.category.id === c.id} onClick={() => apply(categoryPick(offers, c.id, own, choice))}>
              {c.name}
            </button>
          ))}
        </div>
      ) : null}
      {!flat && siblings.length > 1 ? (
        <div role="group" aria-label={t('Тариф')} className={siblings.length > 3 ? 'mn-seg flow many' : 'mn-seg flow'}>
          {siblings.map((o) => (
            <button key={o.tariff.id} type="button" aria-pressed={offer?.tariff.id === o.tariff.id} onClick={() => apply(tariffPick(offers, o.tariff.id, own, choice))}>
              {o.tariff.name}
            </button>
          ))}
        </div>
      ) : null}
      {transition && offer ? (
        <div className="mn-card mn-move" role="note">
          <strong>{t('Будет переход на тариф «{name}»', { name: offer.tariff.name })}</strong>
          <span>{transitionText(currentName)}</span>
        </div>
      ) : null}
      {(fromTrial && offer) || another || switching || limitBytes || limitNote ? (
        <div className="mn-notes">
          {limitNote ? <span>{limitNote}</span> : null}
          {switching && offer ? <span>{t('Тариф сменится на «{name}»', { name: offer.tariff.name })}</span> : null}
          {fromTrial && offer ? <span>{t('Пробный период закончится сразу после оплаты, тариф «{name}» начнёт действовать с даты оплаты', { name: offer.tariff.name })}</span> : null}
          {another ? <span>{t('У новой подписки будет своя ссылка, текущие подписки останутся как есть')}</span> : null}
          {limitBytes ? (
            <span>
              {t('Трафик: {traffic} {reset}', { traffic: gbText(limitBytes), reset: resetPlan(offer?.tariff.traffic.reset ?? 'no_reset') })}
            </span>
          ) : null}
        </div>
      ) : null}
      {offerPromo ? <p className="mn-sale">{promoTitle(offerPromo)}</p> : null}
      {rows.length > 0 ? (
        <div role="radiogroup" aria-label={t('Срок')} className="mn-terms">
          {rows.map(({ offer: o, period: p }) => {
            const save = starsMode ? p.stars_savings_percent : p.savings_percent;
            return (
              <button
                key={`${o.tariff.id}:${p.period_id}`}
                type="button"
                role="radio"
                aria-checked={offer?.tariff.id === o.tariff.id && period === p.period_id}
                className="mn-term press"
                onClick={() => (flat ? apply(flatTerm(flat, o.tariff.id, choice)) : setPeriod(p.period_id))}
              >
                <span className="mn-dot" aria-hidden="true" />
                <span className="mn-term-text">
                  <span className="mn-term-top">
                    <strong>{termName(p.months, p.days)}</strong>
                    <span className="mn-term-price">
                      {p.promotion ? <s className="mn-old num">{starsMode && p.promotion.full_stars ? stars(p.promotion.full_stars) : money(p.promotion.full_price)}</s> : null}
                      <strong className="num">{starsMode && p.stars ? stars(p.stars) : money(p.price)}</strong>
                    </span>
                  </span>
                  <span className="mn-term-sub">
                    {p.days < SHORT_PERIOD_DAYS
                      ? null
                      : starsMode && p.stars_per_month
                        ? t('{price} в месяц', { price: stars(p.stars_per_month) })
                        : t('{price} в месяц', { price: money(p.per_month) })}
                    {p.promotion ? (
                      <span className="mn-save">
                        {p.days < SHORT_PERIOD_DAYS ? '' : ' · '}
                        {promoCut(p.promotion.percent)}
                      </span>
                    ) : null}
                    {save && !p.promotion ? <span className="mn-save"> · {t('выгоднее на {percent}%', { percent: save })}</span> : null}
                  </span>
                </span>
              </button>
            );
          })}
        </div>
      ) : null}
      {extra && extra.max > 0 ? (
        <div className="mn-card mn-devs">
          <span className="mn-devs-text">
            <strong>{t('Устройства')}</strong>
            <span>
              {t('{included} в тарифе, ещё до {max}', { included, max: extra.max })}{' '}
              <span style={{ whiteSpace: 'nowrap' }}>
                {t('по {price} в месяц', { price: starsMode && extra.monthly_stars ? stars(extra.monthly_stars) : extraMonthly ? money(extraMonthly) : '' })}
              </span>
            </span>
            {ladder && !deviceCut ? <span>{ladder}</span> : null}
            {deviceCut ? (
              <span className="mn-devs-cut" aria-live="polite">
                {deviceCut.label}
              </span>
            ) : null}
          </span>
          <span className="mn-stepper">
            <button type="button" className="mn-circ press" aria-label={t('Меньше устройств')} aria-disabled={devices <= included} onClick={() => devices > included && setDevices(devices - 1)}>
              {MINUS}
            </button>
            <span aria-live="polite">{devices}</span>
            <button type="button" className="mn-circ press" aria-label={t('Больше устройств')} aria-disabled={devices >= maxDevices} onClick={() => devices < maxDevices && setDevices(devices + 1)}>
              +
            </button>
          </span>
        </div>
      ) : null}
      {error && catalog ? <p className="mn-error">{error}</p> : null}
      {offer ? (
        <button type="button" className="mn-main mn-paybtn press" onClick={toPay} disabled={!q || !periodOffer}>
          {totalText ? t('Далее · {amount}', { amount: totalText }) : t('Далее')}
        </button>
      ) : null}
    </main>
  );
}
