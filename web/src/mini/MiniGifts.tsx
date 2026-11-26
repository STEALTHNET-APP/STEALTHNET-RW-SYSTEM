import { useCallback, useEffect, useRef, useState } from 'react';
import { useSearchParams } from 'react-router';
import { api } from '../lib/api';
import { money } from '../lib/format';
import { t } from '../lib/i18n';
import { methodKey } from '../lib/methods';
import { promoCut, promoOf, promoTitle } from '../lib/promo';
import type { ItemQuote, PayResult } from '../cabinet/extras';
import { codeDaysText, failText, firstTarget, giftItem, giftState, GIFTS, limitNote, planText, targetBody, term, type GiftOffers, type GiftTarget, type MyGift, type RedeemPreview, type Redeemed } from '../cabinet/gifting';
import { stars } from '../cabinet/model';
import { BackHead, useMini } from './MiniApp';
import { openExternal, webApp } from './telegram';

function Redeem({ code, onClose }: { code: string; onClose: () => void }) {
  const { reload } = useMini();
  const [preview, setPreview] = useState<RedeemPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [target, setTarget] = useState<GiftTarget>('auto');
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<Redeemed | null>(null);

  useEffect(() => {
    let alive = true;
    api<RedeemPreview>(`${GIFTS}/preview`, { method: 'POST', body: targetBody(code, 'auto') })
      .then((p) => {
        if (alive) {
          setPreview(p);
          setTarget(firstTarget(p));
        }
      })
      .catch((err) => alive && setError(failText(err, t('Подарок не нашёлся'))));
    return () => {
      alive = false;
    };
  }, [code]);

  const activate = async () => {
    setBusy(true);
    setError(null);
    try {
      setDone(await api<Redeemed>(`${GIFTS}/redeem`, { method: 'POST', body: targetBody(code, target) }));
      window.setTimeout(() => void reload(), 2000);
    } catch (err) {
      setError(failText(err, t('Подарок не активировался')));
    } finally {
      setBusy(false);
    }
  };

  if (done) {
    return (
      <div className="mn-card mn-pad">
        <strong>{t('Подарок активирован')}</strong>
        <span className="mn-ink2">
          {t('Подписка «{name}» на {term} будет готова через минуту.', { name: done.gift.tariff, term: done.gift.term })}
        </span>
      </div>
    );
  }

  const choose = preview ? preview.multi && preview.choices.length > 0 && !preview.current : false;
  const plan = preview ? planText(preview, target) : null;

  return (
    <div className="mn-card mn-pad">
      <strong>{preview ? t('Подарок для вас') : t('Подарок')}</strong>
      {!preview && !error ? <span className="mn-muted">{t('Проверяем код…')}</span> : null}
      {preview ? (
        <span className="mn-ink2">
          {t('Подписка «{name}» на {term}.', { name: preview.gift.tariff, term: preview.gift.term })}
        </span>
      ) : null}
      {preview && limitNote(preview) ? <span className="mn-muted small">{limitNote(preview)}</span> : null}
      {preview && choose ? (
        <div role="radiogroup" aria-label={t('Куда зачесть подарок')} className="mn-terms">
          {preview.choices.map((c) => (
            <button key={c.subscription_id} type="button" role="radio" aria-checked={target === c.subscription_id} className="mn-term press" onClick={() => setTarget(c.subscription_id)}>
              <span className="mn-dot" aria-hidden="true" />
              <span className="mn-term-text">
                <strong>{c.tariff ? t('Продлить «{name}»', { name: c.tariff }) : t('Продлить «подписку»')}</strong>
              </span>
            </button>
          ))}
          {preview.new_allowed !== false ? (
            <button type="button" role="radio" aria-checked={target === 'new'} className="mn-term press" onClick={() => setTarget('new')}>
              <span className="mn-dot" aria-hidden="true" />
              <span className="mn-term-text">
                <strong>{t('Новая подписка')}</strong>
              </span>
            </button>
          ) : null}
        </div>
      ) : null}
      {plan && !choose ? <span className={`mn-tone ${plan.tone === 'warn' ? 'warn' : 'muted'}`}>{plan.text}</span> : null}
      {preview?.gift.own ? <span className="mn-muted small">{t('Это ваш подарок: после активации код другу уже не пригодится.')}</span> : null}
      {error ? <p className="mn-error">{error}</p> : null}
      {preview ? (
        <button type="button" className="mn-main sm press" onClick={() => void activate()} disabled={busy}>
          {busy ? t('Активируем…') : t('Активировать подарок')}
        </button>
      ) : null}
      <button type="button" className="mn-link press" onClick={onClose} disabled={busy}>
        {preview ? t('Не сейчас') : t('Ввести другой код')}
      </button>
    </div>
  );
}

function Mine({ mine }: { mine: MyGift[] }) {
  const [copied, setCopied] = useState<number | null>(null);
  if (mine.length === 0) {
    return null;
  }
  const copy = async (g: MyGift) => {
    try {
      await navigator.clipboard.writeText(g.links.bot ?? g.code);
      setCopied(g.id);
    } catch {
      setCopied(null);
    }
  };
  return (
    <section aria-label={t('Мои подарки')} className="mn-gifts">
      <div className="mn-pay-head">
        <h2>{t('Мои подарки')}</h2>
      </div>
      <div className="mn-card">
        {mine.map((g) => {
          const state = giftState(g);
          return (
            <div key={g.id} className="mn-gift">
              <span className="mn-gift-top">
                <strong>
                  {t('«{name}» на {term}', { name: g.tariff_name, term: g.term })}
                </strong>
                <span className="mono mn-gift-code">{g.code}</span>
              </span>
              <span className={`mn-tone ${state.tone}`}>{state.text}</span>
              {g.status === 'active' ? (
                <button type="button" className="mn-link press" onClick={() => void copy(g)}>
                  {copied === g.id ? t('Скопировано') : g.links.bot ? t('Скопировать ссылку для друга') : t('Скопировать код')}
                </button>
              ) : null}
            </div>
          );
        })}
      </div>
    </section>
  );
}

export function MiniGifts() {
  const { me } = useMini();
  const [params, setParams] = useSearchParams();
  const code = params.get('code');
  const [offers, setOffers] = useState<GiftOffers | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [mine, setMine] = useState<MyGift[]>([]);
  const [tariff, setTariff] = useState<number | null>(null);
  const [period, setPeriod] = useState<number | null>(null);
  const [quote, setQuote] = useState<ItemQuote | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [codeInput, setCodeInput] = useState('');
  const [codeOpen, setCodeOpen] = useState(false);
  const seq = useRef(0);

  const loadMine = useCallback(async () => {
    try {
      setMine(await api<MyGift[]>(`${GIFTS}/mine`));
    } catch {
      setMine([]);
    }
  }, []);

  useEffect(() => {
    api<GiftOffers>(`${GIFTS}/offers`)
      .then((o) => {
        setOffers(o);
        const first = o.offers[0];
        if (first) {
          setTariff(first.tariff.id);
          setPeriod(first.periods[Math.min(1, first.periods.length - 1)]?.period_id ?? null);
        }
      })
      .catch((err) => setLoadError(failText(err, t('Подарки сейчас недоступны'))));
    void loadMine();
  }, [loadMine]);

  const list = offers?.offers ?? [];
  const offer = list.find((o) => o.tariff.id === tariff) ?? null;
  const giftPromo = promoOf(offer?.periods);
  const periodOffer = offer?.periods.find((p) => p.period_id === period) ?? null;

  useEffect(() => {
    if (!offer || !periodOffer) {
      setQuote(null);
      return;
    }
    const mine = ++seq.current;
    const timer = window.setTimeout(async () => {
      try {
        const q = await api<ItemQuote>('/api/client/items/quote', { method: 'POST', body: giftItem(offer.tariff.id, periodOffer.period_id) });
        if (mine === seq.current) {
          setQuote(q);
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
  }, [offer, periodOffer]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => methodKey(m) === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';
  const inTelegramWallet = chosen?.provider === 'gram' && webApp() !== null;
  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? money(q.total) : '';
  const balanceShort = methods.find((m) => m.kind === 'balance' && m.unavailable?.type === 'insufficient_balance');

  const pay = async () => {
    if (!offer || !periodOffer || !chosen) {
      return;
    }
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      const res = await api<PayResult>('/api/client/items/checkout', {
        method: 'POST',
        body: { item: giftItem(offer.tariff.id, periodOffer.period_id), provider: chosen.provider, method: chosen.method ?? undefined },
      });
      if (res.type === 'redirect') {
        if (inTelegramWallet) {
          try {
            const sent = await api<{ chat_url: string }>(`/api/client/payments/${res.payment_id}/telegram`, { method: 'POST' });
            setNote(t('Счёт отправлен в чат с ботом. Нажмите там «Оплатить в Telegram». Код подарка придёт туда же.'));
            openExternal(sent.chat_url);
            return;
          } catch (err) {
            setError(failText(err, t('Счёт в чат не отправился, открываем другой кошелёк')));
          }
        }
        openExternal(res.pay_url);
        setNote(t('Оплата открылась в новом окне. Когда она пройдёт, код подарка придёт в чат с ботом и появится ниже.'));
        return;
      }
      setNote(t('Оплачено с баланса. Код подарка появится ниже через несколько секунд и придёт в чат с ботом.'));
      window.setTimeout(() => void loadMine(), 2500);
    } catch (err) {
      setError(failText(err, t('Оплата не началась')));
    } finally {
      setBusy(false);
    }
  };

  if (loadError) {
    return (
      <main className="mn-page">
        <BackHead title={t('Подарки')} />
        <p className="mn-error">{loadError}</p>
      </main>
    );
  }

  return (
    <main className="mn-page plans">
      <BackHead title={t('Подарки')} />
      {code ? (
        <Redeem code={code} onClose={() => setParams({})} />
      ) : codeOpen ? (
        <form
          className="mn-promo"
          onSubmit={(e) => {
            e.preventDefault();
            if (codeInput.trim()) {
              setParams({ code: codeInput.trim() });
              setCodeOpen(false);
            }
          }}
        >
          <input value={codeInput} onChange={(e) => setCodeInput(e.target.value.toUpperCase())} placeholder="ABCD-EFGH-JKLM" aria-label={t('Код подарка')} autoComplete="off" spellCheck={false} />
          <button type="submit" className="mn-btn2 press">
            {t('Активировать')}
          </button>
        </form>
      ) : (
        <button type="button" className="mn-link press mn-gift-has" onClick={() => setCodeOpen(true)}>
          {t('Есть код подарка?')}
        </button>
      )}

      <p className="mn-notes">
        <span>{t('Выберите тариф и срок, оплатите — и получите код. Друг активирует его в боте или в кабинете.')}</span>
        {offers ? <span>{codeDaysText(offers.code_days)}</span> : null}
      </p>

      {offers === null ? <p className="mn-muted">{t('Загружаем…')}</p> : null}
      {offers && list.length === 0 ? <p className="mn-muted">{t('Сейчас подарить подписку нельзя: нет тарифов для подарка.')}</p> : null}

      {list.length > 1 ? (
        <div role="group" aria-label={t('Тариф')} className={list.length > 3 ? 'mn-seg flow many' : 'mn-seg flow'}>
          {list.map((o) => (
            <button
              key={o.tariff.id}
              type="button"
              aria-pressed={tariff === o.tariff.id}
              onClick={() => {
                setTariff(o.tariff.id);
                const keep = o.periods.find((p) => p.months === periodOffer?.months) ?? o.periods[Math.min(1, o.periods.length - 1)];
                setPeriod(keep?.period_id ?? null);
              }}
            >
              {o.tariff.name}
            </button>
          ))}
        </div>
      ) : null}

      {giftPromo ? <p className="mn-sale">{promoTitle(giftPromo)}</p> : null}
      {offer ? (
        <div role="radiogroup" aria-label={t('Срок')} className="mn-terms">
          {offer.periods.map((p) => (
            <button key={p.period_id} type="button" role="radio" aria-checked={period === p.period_id} className="mn-term press" onClick={() => setPeriod(p.period_id)}>
              <span className="mn-dot" aria-hidden="true" />
              <span className="mn-term-text">
                <span className="mn-term-top">
                  <strong>
                    {list.length === 1 ? `${offer.tariff.name} · ` : ''}
                    {term(p.days)}
                  </strong>
                  <span className="mn-term-price">
                    {p.promotion ? <s className="mn-old num">{starsMode && p.promotion.full_stars ? stars(p.promotion.full_stars) : money(p.promotion.full_price)}</s> : null}
                    <strong className="num">{starsMode && p.stars ? stars(p.stars) : money(p.price)}</strong>
                  </span>
                </span>
                {p.promotion ? <span className="mn-term-sub">{promoCut(p.promotion.percent)}</span> : null}
              </span>
            </button>
          ))}
        </div>
      ) : null}

      {offer ? (
        <>
          <div className="mn-pay">
            <div className="mn-pay-head">
              <h2>{t('Оплата')}</h2>
            </div>
            {q && methods.length === 0 ? <p className="mn-muted small">{t('В мини-приложении оплатить сейчас нельзя. Подарок можно купить в боте или в кабинете на сайте.')}</p> : null}
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
              </p>
            ) : null}
          </div>
          <button type="button" className="mn-main mn-paybtn press" onClick={() => void pay()} disabled={busy || !chosen || !q}>
            {busy ? t('Открываем оплату…') : inTelegramWallet ? t('Подарить за {amount} в Telegram', { amount: totalText }) : t('Подарить за {amount}', { amount: totalText })}
          </button>
        </>
      ) : null}

      <Mine mine={mine} />
    </main>
  );
}
