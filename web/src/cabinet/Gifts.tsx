import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useSearchParams } from 'react-router';
import { api } from '../lib/api';
import { money, MINUS } from '../lib/format';
import { t } from '../lib/i18n';
import { methodKey } from '../lib/methods';
import { promoCut, promoOf, promoTitle } from '../lib/promo';
import { Card, useCabinet } from './CabinetApp';
import type { ItemQuote, PayResult } from './extras';
import { codeDaysText, failText, firstTarget, giftItem, giftState, GIFTS, limitNote, planText, shareLink, targetBody, term, type GiftOffers, type GiftTarget, type MyGift, type RedeemPreview, type Redeemed } from './gifting';
import { stars, type CheckoutMethod, type QuoteLineView } from './model';

const PAYMENT_KEY = 'sn.cabinet.gift';

type Payment = {
  payment_id: string;
  status: string;
  order_status: string;
  pay_url: string | null;
};

function methodAmount(method: CheckoutMethod): number | null {
  const a = method.amount;
  if (typeof a === 'number') {
    return a;
  }
  if (a && typeof a === 'object' && typeof a.stars === 'number') {
    return a.stars;
  }
  return null;
}

function readKey(): string | null {
  try {
    return window.sessionStorage.getItem(PAYMENT_KEY);
  } catch {
    return null;
  }
}

function dropKey() {
  try {
    window.sessionStorage.removeItem(PAYMENT_KEY);
  } catch {
    return;
  }
}

function useMine() {
  const [mine, setMine] = useState<MyGift[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const load = useCallback(async () => {
    try {
      setMine(await api<MyGift[]>(`${GIFTS}/mine`));
      setError(null);
    } catch (err) {
      setError(failText(err, t('Подарки не загрузились')));
    }
  }, []);
  useEffect(() => {
    void load();
  }, [load]);
  return { mine, error, load };
}

function ShareButtons({ gift }: { gift: MyGift }) {
  const [note, setNote] = useState<string | null>(null);
  const link = gift.links.bot ?? gift.links.cabinet;
  const copy = async (text: string, done: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setNote(done);
    } catch {
      setNote(t('Не получилось скопировать, выделите код вручную'));
    }
  };
  return (
    <span className="cb-gift-acts">
      <button type="button" className="cb-second sm" onClick={() => void copy(gift.code, t('Код скопирован'))}>
        {t('Скопировать код')}
      </button>
      {link ? (
        <button
          type="button"
          className="cb-second sm"
          onClick={() =>
            void shareLink(link, t('Дарю подписку «{name}» на {term}. Код: {code}', { name: gift.tariff_name, term: gift.term, code: gift.code })).then((r) =>
              setNote(r === 'copied' ? t('Ссылка скопирована') : r === 'failed' ? t('Не получилось поделиться, скопируйте код') : null),
            )
          }
        >
          {t('Поделиться ссылкой')}
        </button>
      ) : null}
      {note ? (
        <span className="cb-small cb-muted" role="status">
          {note}
        </span>
      ) : null}
    </span>
  );
}

function MineCard({ mine, error }: { mine: MyGift[] | null; error: string | null }) {
  if (error) {
    return (
      <p className="cb-error" role="alert">
        {error}
      </p>
    );
  }
  if (!mine || mine.length === 0) {
    return null;
  }
  return (
    <Card className="cb-paylist" label={t('Мои подарки')}>
      <h2 className="cb-h2 cb-gift-h">{t('Мои подарки')}</h2>
      <ul className="cb-payrows">
        {mine.map((g) => {
          const state = giftState(g);
          return (
            <li key={g.id} className="cb-payrow cb-giftrow">
              <span className="cb-payrow-main">
                <strong>
                  {t('«{name}» на {term}', { name: g.tariff_name, term: g.term })}
                </strong>
                <span className="cb-small cb-muted">
                  {t('Код')} <span className="mono cb-gift-code">{g.code}</span>
                </span>
              </span>
              <span className="cb-payrow-side">
                <span className={`cb-small cb-status ${state.tone}`}>{state.text}</span>
              </span>
              {g.status === 'active' ? <ShareButtons gift={g} /> : null}
            </li>
          );
        })}
      </ul>
    </Card>
  );
}

function RedeemCard({ code, onClose, onDone }: { code: string; onClose: () => void; onDone: () => void }) {
  const { reload } = useCabinet();
  const [preview, setPreview] = useState<RedeemPreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [target, setTarget] = useState<GiftTarget>('auto');
  const [busy, setBusy] = useState(false);
  const [done, setDone] = useState<Redeemed | null>(null);

  useEffect(() => {
    let alive = true;
    setPreview(null);
    setError(null);
    api<RedeemPreview>(`${GIFTS}/preview`, {
      method: 'POST',
      body: targetBody(code, 'auto'),
    })
      .then((p) => {
        if (!alive) {
          return;
        }
        setPreview(p);
        setTarget(firstTarget(p));
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
      setDone(
        await api<Redeemed>(`${GIFTS}/redeem`, {
          method: 'POST',
          body: targetBody(code, target),
        }),
      );
      await reload();
      onDone();
    } catch (err) {
      setError(failText(err, t('Подарок не активировался')));
    } finally {
      setBusy(false);
    }
  };

  if (done) {
    return (
      <Card className="cb-wait cb-gift-card" label={t('Подарок активирован')}>
        <h1 className="cb-h1">{t('Подарок активирован')}</h1>
        <p className="cb-ink2">
          {t('Подписка «{name}» на {term} будет готова через минуту.', { name: done.gift.tariff, term: done.gift.term })}
        </p>
        <div className="cb-btns">
          <Link to="/cabinet" className="cb-main">
            {t('К подписке')}
          </Link>
        </div>
      </Card>
    );
  }

  const choose = preview ? preview.multi && preview.choices.length > 0 && !preview.current : false;
  const plan = preview ? planText(preview, target) : null;

  return (
    <Card className="cb-wait cb-gift-card" label={t('Подарок для вас')}>
      <h1 className="cb-h1">{preview ? t('Подарок для вас') : t('Подарок')}</h1>
      {!preview && !error ? <p className="cb-muted">{t('Проверяем код…')}</p> : null}
      {preview ? (
        <>
          <p className="cb-ink2">
            {t('Подписка «{name}» на {term}.', { name: preview.gift.tariff, term: preview.gift.term })}
          </p>
          {limitNote(preview) ? <p className="cb-muted">{limitNote(preview)}</p> : null}
          {choose ? (
            <div role="radiogroup" aria-label={t('Куда зачесть подарок')} className="cb-method-grid">
              {preview.choices.map((c) => (
                <button key={c.subscription_id} type="button" role="radio" aria-checked={target === c.subscription_id} className="cb-method cb-target" onClick={() => setTarget(c.subscription_id)}>
                  <span className="cb-radio" aria-hidden="true" />
                  <span className="cb-method-text">
                    <strong>{t('Продлить «{name}»', { name: c.tariff ?? t('подписку') })}</strong>
                    <span className="cb-target-note muted">{t('Срок добавится к этой подписке')}</span>
                  </span>
                </button>
              ))}
              {preview.new_allowed !== false ? (
                <button type="button" role="radio" aria-checked={target === 'new'} className="cb-method cb-target" onClick={() => setTarget('new')}>
                  <span className="cb-radio" aria-hidden="true" />
                  <span className="cb-method-text">
                    <strong>{t('Новая подписка')}</strong>
                    <span className="cb-target-note muted">{t('Со своей ссылкой, текущие останутся как есть')}</span>
                  </span>
                </button>
              ) : null}
            </div>
          ) : plan ? (
            <p className={`cb-lead ${plan.tone}`}>{plan.text}</p>
          ) : null}
          {preview.gift.own ? <p className="cb-small cb-muted">{t('Это ваш подарок: после активации код другу уже не пригодится.')}</p> : null}
        </>
      ) : null}
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      <div className="cb-btns">
        {preview ? (
          <button type="button" className="cb-main" onClick={() => void activate()} disabled={busy}>
            {busy ? t('Активируем…') : t('Активировать подарок')}
          </button>
        ) : null}
        <button type="button" className="cb-second" onClick={onClose} disabled={busy}>
          {preview ? t('Не сейчас') : t('Ввести другой код')}
        </button>
      </div>
    </Card>
  );
}

function CodeForm({ onCode }: { onCode: (code: string) => void }) {
  const [value, setValue] = useState('');
  return (
    <Card className="cb-total cb-gift-code-card" label={t('Есть код подарка')}>
      <h2 className="cb-h2">{t('Есть код подарка?')}</h2>
      <form
        className="cb-promo"
        onSubmit={(e) => {
          e.preventDefault();
          const code = value.trim();
          if (code) {
            onCode(code);
          }
        }}
      >
        <label className="cb-promo-field">
          <span className="sr-only">{t('Код подарка')}</span>
          <input value={value} onChange={(e) => setValue(e.target.value.toUpperCase())} placeholder="ABCD-EFGH-JKLM" maxLength={64} autoComplete="off" spellCheck={false} />
        </label>
        <button type="submit" className="cb-second">
          {t('Активировать')}
        </button>
      </form>
    </Card>
  );
}

function PaidCard({ payment, newest, onAgain }: { payment: Payment; newest: MyGift | null; onAgain: () => void }) {
  const done = payment.order_status === 'fulfilled';
  const review = payment.order_status === 'review';
  const failed = payment.order_status === 'canceled' || payment.order_status === 'refunded';
  return (
    <Card className="cb-wait cb-gift-card" label={t('Оплата подарка')}>
      <h1 className="cb-h1">{done ? t('Подарок оплачен') : review ? t('Оплата принята') : failed ? t('Оплата не прошла') : t('Ждём оплату')}</h1>
      {done && newest ? (
        <>
          <p className="cb-ink2">
            {t('«{name}» на {term}. Отправьте другу код или ссылку — он активирует подарок в боте или здесь, в кабинете.', { name: newest.tariff_name, term: newest.term })}
          </p>
          <p className="cb-gift-big mono">{newest.code}</p>
          <ShareButtons gift={newest} />
        </>
      ) : (
        <p className="cb-ink2">
          {done
            ? t('Код подарка появится в списке ниже через несколько секунд.')
            : review
              ? t('Проверяем платёж. Код подарка появится в списке ниже, как только проверка закончится.')
              : failed
                ? t('Деньги не списаны или возвращены. Попробуйте ещё раз или выберите другой способ.')
                : t('Как только платёжная система подтвердит оплату, код появится здесь.')}
        </p>
      )}
      <div className="cb-btns">
        {!done && !review && !failed && payment.pay_url ? (
          <a className="cb-second" href={payment.pay_url}>
            {t('Вернуться к оплате')}
          </a>
        ) : null}
        <button type="button" className="cb-second" onClick={onAgain}>
          {done ? t('Подарить ещё') : t('Выбрать заново')}
        </button>
      </div>
    </Card>
  );
}

export function GiftsPage() {
  const { me, reload } = useCabinet();
  const [params, setParams] = useSearchParams();
  const code = params.get('code');
  const { mine, error: mineError, load: loadMine } = useMine();
  const [offers, setOffers] = useState<GiftOffers | null>(null);
  const [offersError, setOffersError] = useState<string | null>(null);
  const [tariff, setTariff] = useState<number | null>(null);
  const [period, setPeriod] = useState<number | null>(null);
  const [quote, setQuote] = useState<ItemQuote | null>(null);
  const [quoteError, setQuoteError] = useState<string | null>(null);
  const [method, setMethod] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [waiting, setWaiting] = useState<Payment | null>(null);
  const seq = useRef(0);

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
      .catch((err) => setOffersError(failText(err, t('Тарифы для подарка не загрузились'))));
  }, []);

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
        const q = await api<ItemQuote>('/api/client/items/quote', {
          method: 'POST',
          body: giftItem(offer.tariff.id, periodOffer.period_id),
        });
        if (mine === seq.current) {
          setQuote(q);
          setQuoteError(null);
        }
      } catch (err) {
        if (mine === seq.current) {
          setQuote(null);
          setQuoteError(failText(err, t('Сумма не посчиталась')));
        }
      }
    }, 150);
    return () => window.clearTimeout(timer);
  }, [offer, periodOffer]);

  useEffect(() => {
    const id = params.get('payment') ?? readKey();
    if (!id) {
      return;
    }
    let alive = true;
    const tick = async (attempt: number) => {
      try {
        const p = await api<Payment>(`/api/client/payments/${id}`);
        if (!alive) {
          return;
        }
        setWaiting(p);
        if (['fulfilled', 'review', 'canceled', 'refunded'].includes(p.order_status)) {
          dropKey();
          if (p.order_status === 'fulfilled') {
            await Promise.all([loadMine(), reload()]);
          }
          return;
        }
        if (attempt < 60) {
          window.setTimeout(() => void tick(attempt + 1), 3000);
        }
      } catch {
        setWaiting(null);
      }
    };
    void tick(0);
    return () => {
      alive = false;
    };
  }, [params, loadMine, reload]);

  const methods = quote?.methods ?? [];
  const chosen = methods.find((m) => methodKey(m) === method && !m.unavailable) ?? methods.find((m) => !m.unavailable) ?? null;
  const starsMode = chosen?.kind === 'stars';
  const q = quote?.quote ?? null;
  const totalText = starsMode && q?.stars ? stars(q.stars.total) : q ? money(q.total) : '';
  const lineText = (line: QuoteLineView) => (starsMode && line.stars !== null ? `${line.stars < 0 ? MINUS : ''}${stars(Math.abs(line.stars))}` : money(line.amount));
  const giftFull = q?.lines.some((line) => line.kind.type === 'promotion') ? (q.lines.find((line) => line.kind.type === 'period') ?? null) : null;
  const newest = useMemo<MyGift | null>(() => mine?.[0] ?? null, [mine]);

  const pay = async () => {
    if (!offer || !periodOffer || !chosen) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await api<PayResult>('/api/client/items/checkout', {
        method: 'POST',
        body: {
          item: giftItem(offer.tariff.id, periodOffer.period_id),
          provider: chosen.provider,
          method: chosen.method ?? undefined,
          return_url: `${window.location.origin}/cabinet/gifts`,
        },
      });
      if (res.type === 'redirect') {
        try {
          window.sessionStorage.setItem(PAYMENT_KEY, res.payment_id);
        } catch {
          setParams({ payment: res.payment_id });
        }
        window.location.href = res.pay_url;
        return;
      }
      setWaiting({
        payment_id: '',
        status: 'paid',
        order_status: 'paid',
        pay_url: null,
      });
      void reload();
      window.setTimeout(() => {
        void loadMine().then(() =>
          setWaiting({
            payment_id: '',
            status: 'paid',
            order_status: 'fulfilled',
            pay_url: null,
          }),
        );
      }, 2500);
    } catch (err) {
      setError(failText(err, t('Оплата не началась')));
    } finally {
      setBusy(false);
    }
  };

  const openCode = (value: string) => setParams({ code: value });
  const closeCode = () => setParams({});
  const blocked = me.client.status === 'blocked';

  return (
    <div className="cb-pay cb-gifts">
      {code ? <RedeemCard code={code} onClose={closeCode} onDone={() => void loadMine()} /> : null}
      {waiting && !code ? (
        <PaidCard
          payment={waiting}
          newest={waiting.order_status === 'fulfilled' ? newest : null}
          onAgain={() => {
            setWaiting(null);
            setParams({});
          }}
        />
      ) : null}

      {code ? null : (
        <>
          <header className="cb-pay-head">
            <h1 className="cb-title">{t('Подписка в подарок')}</h1>
            <p className="cb-lead muted">{t('Выберите тариф и срок, оплатите — и получите код. Друг активирует его в боте или в кабинете.')} {offers ? codeDaysText(offers.code_days) : null}</p>
          </header>

          {offersError ? (
            <p className="cb-error" role="alert">
              {offersError}
            </p>
          ) : null}
          {offers && list.length === 0 ? <p className="cb-muted">{t('Сейчас подарить подписку нельзя: нет тарифов для подарка.')}</p> : null}

          {list.length ? (
            <div className="cb-cols">
              <div className="cb-col left">
                <Card className="cb-pick" label={t('Тариф и срок')}>
                  <div className="cb-row wrap">
                    <h2 className="cb-h2">{t('Тариф и срок')}</h2>
                    {list.length > 1 ? (
                      <div role="group" aria-label={t('Тариф')} className={list.length > 3 ? 'cb-cats many' : 'cb-cats'}>
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
                  </div>
                  {offer?.tariff.description ? <p className="cb-muted cb-small">{offer.tariff.description}</p> : null}
                  {giftPromo ? <p className="cb-sale">{promoTitle(giftPromo)}</p> : null}
                  <div role="radiogroup" aria-label={t('Срок')} className={starsMode ? 'cb-terms wide' : 'cb-terms'}>
                    {offer?.periods.map((p) => (
                      <button key={p.period_id} type="button" role="radio" aria-checked={period === p.period_id} className="cb-term-card" onClick={() => setPeriod(p.period_id)}>
                        <span className="cb-term-name">{term(p.days)}</span>
                        {p.promotion ? (
                          <span className="cb-term-old">
                            <s>{starsMode && p.promotion.full_stars ? stars(p.promotion.full_stars) : money(p.promotion.full_price)}</s> <strong>{promoCut(p.promotion.percent)}</strong>
                          </span>
                        ) : null}
                        <span className="cb-term-price">{starsMode && p.stars ? stars(p.stars) : money(p.price)}</span>
                      </button>
                    ))}
                  </div>
                </Card>

                <Card className="cb-methods" label={t('Способ оплаты')}>
                  <h2 className="cb-h2">{t('Способ оплаты')}</h2>
                  {methods.length === 0 && quote ? <p className="cb-muted">{t('Здесь пока нет способов оплаты. Напишите в поддержку — подскажем, как оплатить.')}</p> : null}
                  <div role="radiogroup" aria-label={t('Способ оплаты')} className="cb-method-grid">
                    {methods.map((m) => {
                      const off = !!m.unavailable;
                      const hint =
                        m.unavailable?.type === 'insufficient_balance'
                          ? t('Не хватает: на балансе {amount}', { amount: money(m.unavailable.balance) })
                          : m.unavailable?.type === 'no_stars_price'
                            ? t('У этого срока нет цены в звёздах')
                            : m.kind === 'balance'
                              ? t('На балансе {amount}', { amount: money(me.balance) })
                              : m.kind === 'stars' && methodAmount(m) !== null
                                ? t('{stars}, оплата в Telegram', { stars: stars(methodAmount(m) ?? 0) })
                                : m.hint;
                      return (
                        <button
                          key={methodKey(m)}
                          type="button"
                          role="radio"
                          aria-checked={chosen === m}
                          aria-disabled={off ? true : undefined}
                          className={off ? 'cb-method off' : 'cb-method'}
                          onClick={() => !off && setMethod(methodKey(m))}
                        >
                          <span className="cb-radio" aria-hidden="true" />
                          <span className="cb-method-text">
                            <strong>{m.title}</strong>
                            <span>{hint}</span>
                          </span>
                        </button>
                      );
                    })}
                  </div>
                </Card>
              </div>

              <div className="cb-col right">
                <Card className="cb-total" label={t('Итого')}>
                  <h2 className="cb-h2">{t('Итого')}</h2>
                  {q && offer && periodOffer ? (
                    <div className="cb-line">
                      <span>
                        {t('Подарок: «{name}» на {term}', { name: offer.tariff.name, term: term(periodOffer.days) })}
                      </span>
                      <span className="num">{giftFull ? lineText(giftFull) : totalText}</span>
                    </div>
                  ) : null}
                  {q
                    ? q.lines
                        .filter((line) => line.kind.type === 'promotion')
                        .map((line) => (
                          <div key={line.label} className="cb-line">
                            <span>{line.label}</span>
                            <span className="num">{lineText(line)}</span>
                          </div>
                        ))
                    : null}
                  <div className="cb-due">
                    <strong>{t('К оплате')}</strong>
                    <span className="cb-due-sum">{totalText}</span>
                  </div>
                  {quoteError || error ? (
                    <p className="cb-error" role="alert">
                      {error ?? quoteError}
                    </p>
                  ) : null}
                  <button type="button" className="cb-main big" onClick={() => void pay()} disabled={busy || blocked || !chosen || !q}>
                    {busy ? t('Открываем оплату…') : t('Оплатить подарок {amount}', { amount: totalText })}
                  </button>
                  <p className="cb-small cb-muted">{t('Код придёт сразу после оплаты: здесь, в «Моих подарках», и сообщением в Telegram.')}</p>
                </Card>
                <CodeForm onCode={openCode} />
              </div>
            </div>
          ) : (
            <CodeForm onCode={openCode} />
          )}
        </>
      )}

      <MineCard mine={mine} error={mineError} />
    </div>
  );
}
