import { useEffect, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api, ApiError } from '../lib/api';
import { dateLong, money, MINUS, traffic } from '../lib/format';
import { t } from '../lib/i18n';
import { linesOf, stars } from '../cabinet/model';
import { addonItem, addonLadder, daysText, doneText, firstKind, leftoverText, offerUrl, type AddonChoice, type AddonKind, type AddonsOffer, type ItemQuote, type PayResult } from '../cabinet/extras';
import { BackHead, MiniSubSwitch, useMini } from './MiniApp';
import { openExternal, webApp } from './telegram';
import { methodKey } from '../lib/methods';
import { ReceiptEmail, useReceiptEmail } from '../lib/receiptEmail';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function MiniAddons() {
  const { me, view, reload } = useMini();
  const receipt = useReceiptEmail(me);
  const [params] = useSearchParams();
  const [offer, setOffer] = useState<AddonsOffer | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [kind, setKind] = useState<AddonKind>('devices');
  const [count, setCount] = useState(1);
  const [pack, setPack] = useState<number | null>(null);
  const [quote, setQuote] = useState<ItemQuote | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [otherWallet, setOtherWallet] = useState<string | null>(null);
  const seq = useRef(0);
  const subscriptionId = view && !view.subscription.ended_at ? view.subscription.id : null;
  const asked = params.get('kind');

  const load = async (id: number, keep: boolean) => {
    try {
      const next = await api<AddonsOffer>(offerUrl(id));
      setOffer(next);
      setLoadError(null);
      if (!keep) {
        setKind(firstKind(next, asked));
        setCount(1);
        setPack(next.traffic.offer?.packs[0]?.gb ?? null);
      }
    } catch (err) {
      setLoadError(failText(err, t('Не удалось загрузить предложения')));
    }
  };

  useEffect(() => {
    setOffer(null);
    setQuote(null);
    setError(null);
    setNote(null);
    setOtherWallet(null);
    if (subscriptionId !== null) {
      void load(subscriptionId, false);
    }
  }, [subscriptionId]);

  const devices = offer?.devices.offer ?? null;
  const trafficOffer = offer?.traffic.offer ?? null;
  const choice: AddonChoice | null =
    kind === 'devices' ? (devices && devices.can_add > 0 ? { kind: 'devices', count: Math.min(count, devices.can_add) } : null) : trafficOffer && pack !== null ? { kind: 'traffic', gb: pack } : null;
  const choiceKey = choice ? JSON.stringify(choice) : '';

  useEffect(() => {
    if (subscriptionId === null || !choice) {
      setQuote(null);
      return;
    }
    const mine = ++seq.current;
    const timer = window.setTimeout(async () => {
      try {
        const next = await api<ItemQuote>('/api/client/items/quote', { method: 'POST', body: addonItem(subscriptionId, choice) });
        if (mine === seq.current) {
          setQuote(next);
          setError(null);
        }
      } catch (err) {
        if (mine === seq.current) {
          setQuote(null);
          setError(failText(err, t('Сумма не посчиталась')));
        }
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [subscriptionId, choiceKey]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => methodKey(m) === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';
  const inTelegramWallet = chosen?.provider === 'gram' && webApp() !== null;
  const balanceShort = methods.find((m) => m.kind === 'balance' && m.unavailable?.type === 'insufficient_balance');
  const q = quote?.quote ?? null;
  const addonCut = q ? (linesOf(q.lines, 'device_discount')[0] ?? null) : null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? money(q.total) : '';

  const pay = async () => {
    if (subscriptionId === null || !choice || !chosen) {
      return;
    }
    const email = receipt.take(chosen);
    if (email === false) {
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    setOtherWallet(null);
    try {
      const res = await api<PayResult>('/api/client/items/checkout', {
        method: 'POST',
        body: { email: email ?? undefined, item: addonItem(subscriptionId, choice), provider: chosen.provider, method: chosen.method ?? undefined },
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
        setNote(t('Оплата открылась в новом окне. Когда она пройдёт, лимит обновится сам.'));
        return;
      }
      setNote(doneText(choice));
      window.setTimeout(() => {
        void reload();
        void load(subscriptionId, true);
      }, 1500);
    } catch (err) {
      if (!receipt.refused(err)) {
        setError(failText(err, t('Оплата не началась')));
      }
    } finally {
      setBusy(false);
    }
  };

  if (subscriptionId === null || view?.status === 'expired') {
    return (
      <main className="mn-page">
        <BackHead title={t('Докупить')} />
        <MiniSubSwitch />
        <div className="mn-card mn-pad">
          <strong>{t('Нужна действующая подписка')}</strong>
          <span className="mn-ink2">{t('Устройства и трафик докупаются к подписке, пока она действует.')}</span>
          <Link to="/app/plans" className="mn-main sm press">
            {view ? t('Продлить') : t('Выбрать тариф')}
          </Link>
        </div>
      </main>
    );
  }

  if (!offer) {
    return (
      <main className="mn-page">
        <BackHead title={t('Докупить')} />
        <MiniSubSwitch />
        <p className={loadError ? 'mn-error' : 'mn-muted'}>{loadError ?? t('Загружаем…')}</p>
      </main>
    );
  }

  if (!devices && !trafficOffer) {
    const reason = offer.devices.unavailable?.message ?? offer.traffic.unavailable?.message ?? t('Для этой подписки докупить нечего.');
    return (
      <main className="mn-page">
        <BackHead title={t('Докупить')} />
        <MiniSubSwitch />
        <div className="mn-card mn-pad">
          <strong>{t('Докупить нечего')}</strong>
          <span className="mn-ink2">{reason}</span>
        </div>
      </main>
    );
  }

  return (
    <main className="mn-page plans">
      <BackHead title={t('Докупить')} />
      <MiniSubSwitch />
      {devices && trafficOffer ? (
        <div role="group" aria-label={t('Что докупить')} className="mn-seg" style={{ gridTemplateColumns: 'repeat(2, minmax(0, 1fr))' }}>
          <button type="button" aria-pressed={kind === 'devices'} onClick={() => setKind('devices')}>
            {t('Устройства')}
          </button>
          <button type="button" aria-pressed={kind === 'traffic'} onClick={() => setKind('traffic')}>
            {t('Трафик')}
          </button>
        </div>
      ) : null}

      {kind === 'devices' && devices ? (
        <>
          <div className="mn-card mn-devs">
            <span className="mn-devs-text">
              <strong>{t('Устройства')}</strong>
              <span>
                {devices.extra_now > 0 ? t('{included} в тарифе, {extra} докуплено', { included: devices.included, extra: devices.extra_now }) : t('{included} в тарифе', { included: devices.included })}
              </span>
            </span>
            {devices.can_add > 0 ? (
              <span className="mn-stepper">
                <button type="button" className="mn-circ press" aria-label={t('Меньше устройств')} aria-disabled={count <= 1} onClick={() => count > 1 && setCount(count - 1)}>
                  {MINUS}
                </button>
                <span aria-live="polite">+{Math.min(count, devices.can_add)}</span>
                <button type="button" className="mn-circ press" aria-label={t('Больше устройств')} aria-disabled={count >= devices.can_add} onClick={() => count < devices.can_add && setCount(count + 1)}>
                  +
                </button>
              </span>
            ) : null}
          </div>
          <p className="mn-notes">
            {devices.can_add > 0 ? (
              <span>
                {t('До конца подписки {days}, до {date}. Одно устройство на этот срок стоит {price}. При продлении устройство стоит {monthly} в месяц.', {
                  days: daysText(devices.days),
                  date: dateLong(devices.expire_at, false),
                  price: starsMode && devices.per_device_stars ? stars(devices.per_device_stars) : money(devices.per_device),
                  monthly: starsMode && devices.monthly_stars ? stars(devices.monthly_stars) : money(devices.monthly),
                })}
              </span>
            ) : (
              <span>{t('К этой подписке больше устройств не докупить.')}</span>
            )}
            {devices.can_add > 0 && devices.discounts?.length ? <span>{addonLadder(devices.discounts)}</span> : null}
            {addonCut ? <span className="mn-notes-strong">{t('{label}: {amount}', { label: addonCut.label, amount: starsMode && addonCut.stars !== null ? stars(addonCut.stars) : money(addonCut.amount) })}</span> : null}
          </p>
        </>
      ) : null}

      {kind === 'traffic' && trafficOffer ? (
        <>
          <p className="mn-notes">
            <span>
              {t('Израсходовано {used} из {limit}. Докупленный трафик расходуется после основного.', { used: traffic(trafficOffer.used_bytes ?? 0), limit: traffic(trafficOffer.limit_bytes ?? 0) })}
            </span>
            <span>{leftoverText(trafficOffer)}</span>
          </p>
          <div role="radiogroup" aria-label={t('Пакет трафика')} className="mn-terms">
            {trafficOffer.packs.map((p) => (
              <button key={p.gb} type="button" role="radio" aria-checked={pack === p.gb} className="mn-term press" onClick={() => setPack(p.gb)}>
                <span className="mn-dot" aria-hidden="true" />
                <span className="mn-term-text">
                  <span className="mn-term-top">
                    <strong>{t('+{n} ГБ', { n: p.gb })}</strong>
                    <strong className="num">{starsMode && p.stars ? stars(p.stars) : money(p.price)}</strong>
                  </span>
                </span>
              </button>
            ))}
          </div>
        </>
      ) : null}

      <div className="mn-pay">
        <div className="mn-pay-head">
          <h2>{t('Оплата')}</h2>
        </div>
        {q && methods.length === 0 ? <p className="mn-muted small">{t('В мини-приложении оплатить сейчас нельзя. Докупить можно в боте или в кабинете на сайте.')}</p> : null}
        <div role="radiogroup" aria-label={t('Способ оплаты')} className="mn-chips">
          {methods.map((m) => (
            <button key={methodKey(m)} type="button" role="radio" aria-checked={chosen === m} className="mn-chip press" disabled={!!m.unavailable} onClick={() => setMethod(methodKey(m))}>
              {m.title}
            </button>
          ))}
        </div>
        {balanceShort?.unavailable?.type === 'insufficient_balance' ? <p className="mn-muted small">{t('Не хватает: на балансе {amount}', { amount: money(me.balance) })}</p> : null}
        {error ? <p className="mn-error">{error}</p> : null}
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
      </div>
      {receipt.asks(chosen) ? <ReceiptEmail value={receipt.email} onChange={receipt.change} error={receipt.error} disabled={busy} mini /> : null}
      <button type="button" className="mn-main mn-paybtn press" onClick={() => void pay()} disabled={busy || !chosen || !q || !choice}>
        {busy ? t('Открываем оплату…') : inTelegramWallet ? t('Оплатить {amount} в Telegram', { amount: totalText }) : t('Оплатить {amount}', { amount: totalText })}
      </button>
    </main>
  );
}
