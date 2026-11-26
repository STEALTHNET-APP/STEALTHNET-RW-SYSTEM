import { useEffect, useRef, useState } from 'react';
import { Link, useLocation, useNavigate } from 'react-router';
import { api, ApiError } from '../lib/api';
import { Captcha } from '../lib/captcha';
import { dateLong, days, money, type Money } from '../lib/format';
import { countryName } from '../lib/geo';
import { answerOf, captchaOf, startWork, type CheckAnswer } from '../lib/humanCheck';
import { t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { siteInviteLink } from '../lib/referral';
import { supportLink, supportTickets } from '../lib/support';
import { Card, useCabinet } from './CabinetApp';
import { ConnectSteps } from './ConnectSteps';
import { DevicesCard } from './DevicesCard';
import { PromoBox } from './PromoBox';
import { SubSwitch, useSubDetail, useSubs } from './SubSwitch';
import { explanation, heldBalance, periodTitle, statusWord, trafficLine, type Catalog, type Referral } from './model';
import { loadTrial, NO_TRIAL, requestTrial, trialFacts, trialPlaces, trialTitle, type TrialCheck, type TrialOffer } from './trial';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function SubscriptionPage() {
  const { me, site, phone, theme } = useCabinet();
  const location = useLocation();
  const navigate = useNavigate();
  const subs = useSubs();
  const detail = useSubDetail(subs.chosen);
  const view = subs.list === undefined ? undefined : detail.view;
  const devices = detail.devices;
  const chosenId = subs.chosen?.subscription.id ?? null;
  const [referral, setReferral] = useState<Referral | null>(null);
  const [catalog, setCatalog] = useState<Catalog | null>(null);
  const [pageError, setError] = useState<string | null>(null);
  const [ask, setAsk] = useState<string>('');
  const [reissued, setReissued] = useState(false);
  const [busy, setBusy] = useState(false);
  const [copied, setCopied] = useState<string | null>(null);
  const [qr, setQr] = useState(false);
  const [trial, setTrial] = useState<TrialOffer>(NO_TRIAL);
  const [trialOpen, setTrialOpen] = useState(false);
  const [trialPick, setTrialPick] = useState<number | null>(null);
  const [trialVariant, setTrialVariant] = useState<number | null>(null);
  const [trialBusy, setTrialBusy] = useState(false);
  const [trialCheck, setTrialCheck] = useState<TrialCheck | null>(null);
  const [trialNote, setTrialNote] = useState<string | null>(null);
  const [trialPaused, setTrialPaused] = useState(false);
  const [trialRound, setTrialRound] = useState(0);
  const trialWork = useRef<Promise<string | null> | null>(null);
  const trialCaptcha = captchaOf(trialCheck);
  const error = pageError ?? subs.error;
  const trialOn = !!site.trial?.enabled;
  const noSub = view === null;

  useEffect(() => {
    if (!trialOn || !noSub) {
      setTrial(NO_TRIAL);
      return;
    }
    let live = true;
    loadTrial()
      .then((offer) => live && setTrial(offer))
      .catch(() => live && setTrial(NO_TRIAL));
    return () => {
      live = false;
    };
  }, [trialOn, noSub]);

  useEffect(() => {
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
    api<Catalog>('/api/client/catalog')
      .then(setCatalog)
      .catch(() => setCatalog(null));
  }, []);

  useEffect(() => {
    setAsk('');
    setReissued(false);
    setQr(false);
    setCopied(null);
    setError(null);
  }, [chosenId]);

  useEffect(() => {
    const id = location.hash.replace('#', '');
    if (id && view !== undefined) {
      window.setTimeout(() => document.getElementById(id)?.scrollIntoView({ behavior: 'smooth', block: 'start' }), 50);
    }
  }, [location.hash, view]);

  const copy = async (text: string, key: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      setError(t('Браузер не дал скопировать: выделите ссылку вручную'));
    }
  };

  const reissue = async () => {
    if (!view) {
      return;
    }
    setBusy(true);
    try {
      const res = await api<{ subscription_url: string }>(`/api/client/subscriptions/${view.subscription.id}/reissue`, { method: 'POST', body: {} });
      detail.setView({ ...view, subscription_url: res.subscription_url });
      detail.setDevices([]);
      setReissued(true);
      setAsk('');
    } catch (err) {
      setError(failText(err, t('Новая ссылка не выпущена')));
    } finally {
      setBusy(false);
    }
  };

  const renewalOff = async () => {
    if (!view) {
      return;
    }
    setBusy(true);
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/autorenew`, {
        method: 'PUT',
        body: { enabled: false, period_id: view.subscription.autorenew.period_id },
      });
      setAsk('');
      const fresh = await api<typeof view>(`/api/client/subscriptions/${view.subscription.id}`).catch(() => null);
      if (fresh) {
        detail.setView(fresh);
      }
    } catch (err) {
      setError(failText(err, t('Автопродление не отключилось')));
    } finally {
      setBusy(false);
    }
  };

  const toggleAuto = async () => {
    if (!view) {
      return;
    }
    if (view.renewal?.status === 'active') {
      setAsk(ask === 'renewal' ? '' : 'renewal');
      return;
    }
    const enabled = !view.subscription.autorenew.enabled;
    const prev = view;
    detail.setView({ ...view, subscription: { ...view.subscription, autorenew: { ...view.subscription.autorenew, enabled } } });
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/autorenew`, {
        method: 'PUT',
        body: { enabled, period_id: view.subscription.autorenew.period_id },
      });
    } catch (err) {
      detail.setView(prev);
      setError(failText(err, t('Автопродление не переключилось')));
    }
  };

  const startTrial = async (variant: number | null, answer?: CheckAnswer) => {
    setTrialBusy(true);
    setError(null);
    try {
      const check = await requestTrial(variant, answer);
      if (check) {
        const failed = check.again === 'failed';
        setTrialVariant(variant);
        setTrialCheck(check);
        trialWork.current = startWork(check);
        setTrialNote(failed ? t('Проверка не пройдена. Попробуйте ещё раз.') : null);
        setTrialPaused(failed && check.check?.kind === 'captcha');
        return;
      }
      setTrialCheck(null);
      setTrialOpen(false);
      for (let i = 0; i < 10; i += 1) {
        await new Promise((r) => window.setTimeout(r, 1500));
        if ((await subs.reload()).length > 0) {
          break;
        }
      }
    } catch (err) {
      setError(failText(err, t('Пробный период не включился')));
      if (answer?.captcha) {
        setTrialPaused(true);
      }
      loadTrial()
        .then(setTrial)
        .catch(() => setTrial(NO_TRIAL));
    } finally {
      setTrialBusy(false);
    }
  };

  const confirmTrial = async () => {
    if (!trialCheck) {
      return;
    }
    setTrialBusy(true);
    await startTrial(trialVariant, await answerOf(trialCheck, trialWork.current));
  };

  const againTrial = () => {
    setTrialPaused(false);
    setTrialNote(null);
    setError(null);
    setTrialRound((n) => n + 1);
  };

  if (view === undefined) {
    return <p className="cb-muted cb-pad">{t('Загружаем подписку…')}</p>;
  }

  const blocked = me.client.status === 'blocked';
  const trialChoices = trial.available ? trial.variants : [];
  const trialSingle = trialChoices.length === 1 ? trialChoices[0] : null;
  const trialPicked = trialChoices.find((choice) => choice.id === trialPick) ?? null;
  const status = statusWord(view, blocked);
  const note = explanation(view, blocked);
  const traffic = view ? trafficLine(view) : null;
  const expire = view?.subscription.mirror.expire_at ?? null;
  const expired = view?.status === 'expired';
  const left = view?.progress?.days_left ?? 0;
  const total = view?.progress?.days_total ?? 0;
  const share = expired || !total ? 0 : Math.min(1, left / total);
  const mode = site.connect?.mode ?? 'ours';
  const url = view?.subscription_url ?? null;
  const offer = view && catalog ? catalog.tariffs.find((t) => t.tariff.id === view.subscription.tariff_id) : null;
  const renewPeriod = offer?.periods.find((p) => p.period_id === view?.subscription.autorenew.period_id) ?? offer?.periods.find((p) => p.days === view?.subscription.period_days) ?? null;
  const chargeDate = expire ? new Date(Date.parse(expire) - (site.autorenew_days_before ?? 1) * 86_400_000).toISOString() : null;
  const renewPromo = renewPeriod?.promotion ?? null;
  const promoOver = renewPromo !== null && Math.max(Date.now(), chargeDate ? Date.parse(chargeDate) : 0) >= Date.parse(renewPromo.ends_at);
  const renewPrice: Money | null = renewPeriod ? (renewPromo && promoOver ? renewPromo.full_price : renewPeriod.price) : null;
  const renewal = view?.renewal && (view.renewal.status === 'active' || view.renewal.status === 'cancelling') ? view.renewal : null;
  const autoOn = renewal ? renewal.status === 'active' : (view?.subscription.autorenew.enabled ?? false);
  const balanceOff = me.balance_enabled === false;
  const held = heldBalance(me);
  const showBalance = !balanceOff || me.balance.minor > 0 || held !== null;
  const canCharge = !balanceOff || me.autorenew_card === true || (renewPrice !== null && me.balance.minor >= renewPrice.minor);
  const autoText = !view
    ? ''
    : renewal
      ? renewal.status === 'cancelling'
        ? t('Отключаем автопродление в {provider}', { provider: renewal.title })
        : renewal.amount && renewal.next_charge_at
          ? t('{provider} спишет {amount} около {date}', {
              provider: renewal.title,
              amount: money(renewal.amount),
              date: dateLong(renewal.next_charge_at, false),
            })
          : t('Продлевает {provider}', { provider: renewal.title })
    : blocked
      ? autoOn
        ? t('На время блокировки не списываем')
        : t('Выключено')
      : !autoOn
        ? expired
          ? t('Выключено')
          : t('Выключено, напомним за 3 дня')
        : !canCharge
          ? expired
            ? t('Продлите подписку вручную')
            : chargeDate
              ? t('Напомним продлить {date}', { date: dateLong(chargeDate, false) })
              : t('Напомним продлить перед окончанием')
          : expired
            ? balanceOff
              ? renewPrice
                ? t('Спишем {amount} при следующей попытке', { amount: money(renewPrice) })
                : t('Спишем при следующей попытке')
              : renewPrice
                ? t('Спишем {amount}, когда хватит баланса', { amount: money(renewPrice) })
                : t('Спишем, когда хватит баланса')
            : renewPrice && chargeDate
              ? phone
                ? t('Спишем {amount} {date}', { amount: money(renewPrice), date: dateLong(chargeDate, false) })
                : t('{date} спишем {amount}', { date: dateLong(chargeDate, false), amount: money(renewPrice) })
              : balanceOff
                ? t('Спишем перед окончанием')
                : t('Спишем с баланса перед окончанием');

  const mayReissue = site.client_access?.reissue !== false;
  const reissueBlock = !mayReissue ? null : reissued ? (
    <p role="status" className="cb-result">
      <strong>{t('Новая ссылка выпущена.')}</strong> {t('Подключите нужные устройства заново.')}
    </p>
  ) : ask === 'link' ? (
    <div className="cb-ask">
      <p>{t('Старая ссылка перестанет работать, а с ней все устройства. Нужные устройства подключите заново по новой ссылке.')}</p>
      <span className={phone ? 'cb-ask-btns col' : 'cb-ask-btns'}>
        <button type="button" className="cb-main sm" onClick={() => void reissue()} disabled={busy}>
          {t('Выпустить новую ссылку')}
        </button>
        <button type="button" className="cb-cancel" onClick={() => setAsk('')} disabled={busy}>
          {t('Отменить')}
        </button>
      </span>
    </div>
  ) : (
    <button type="button" className="cb-text" onClick={() => setAsk('link')} disabled={!view || busy}>
      {t('Выпустить новую ссылку')}
    </button>
  );

  const subCard = !view ? (
    <Card className="cb-sub" label={t('Подписка')}>
      <h1 className="cb-h1">{t('Подписки пока нет')}</h1>
      <p className="cb-ink2">{t('Выберите тариф — подписка начнёт работать сразу после оплаты.')}</p>
      <div className={phone ? 'cb-btns col' : 'cb-btns'}>
        <Link to="/cabinet/pay" className="cb-main">
          {t('Выбрать тариф')}
        </Link>
        {trialSingle && !trialCheck ? (
          <button type="button" className="cb-second" onClick={() => void startTrial(trialSingle.id)} disabled={trialBusy}>
            {trialBusy ? t('Включаем…') : t('Попробовать бесплатно {days}', { days: days(trialSingle.days) })}
          </button>
        ) : null}
        {trialChoices.length > 1 && !trialCheck && !trialOpen ? (
          <button type="button" className="cb-second" onClick={() => setTrialOpen(true)}>
            {t('Попробовать бесплатно')}
          </button>
        ) : null}
      </div>
      {trialChoices.length > 1 && trialOpen && !trialCheck ? (
        <div className="cb-trial" role="group" aria-labelledby="cb-trial-h">
          <h2 id="cb-trial-h" className="cb-trial-h">
            {t('Выберите пробный период')}
          </h2>
          <div role="radiogroup" aria-labelledby="cb-trial-h" className="cb-method-grid">
            {trialChoices.map((choice, index) => {
              const places = trialPlaces(choice);
              return (
                <button key={choice.id} type="button" role="radio" aria-checked={trialPick === choice.id} className="cb-method" onClick={() => setTrialPick(choice.id)} disabled={trialBusy}>
                  <span className="cb-radio" aria-hidden="true" />
                  <span className="cb-method-text">
                    <strong>{trialTitle(choice, index)}</strong>
                    <span>{trialFacts(choice)}</span>
                    {places ? <span>{places}</span> : null}
                  </span>
                </button>
              );
            })}
          </div>
          <span className={phone ? 'cb-ask-btns col' : 'cb-ask-btns'}>
            <button type="button" className="cb-main sm" onClick={() => trialPicked && void startTrial(trialPicked.id)} disabled={trialBusy || !trialPicked}>
              {trialBusy ? t('Включаем…') : trialPicked ? t('Включить «{name}»', { name: trialTitle(trialPicked, trialChoices.indexOf(trialPicked)) }) : t('Выберите вариант')}
            </button>
            <button
              type="button"
              className="cb-cancel"
              onClick={() => {
                setTrialOpen(false);
                setTrialPick(null);
              }}
              disabled={trialBusy}
            >
              {t('Отменить')}
            </button>
          </span>
        </div>
      ) : null}
      {trialCheck ? (
        <div className={trialCaptcha ? 'cb-human col' : 'cb-human'} role="group" aria-label={t('Проверка перед пробным периодом')}>
          <p className="cb-ink2">{trialCheck.text}</p>
          {trialNote ? (
            <p className="cb-warn" role="alert">
              {trialNote}
            </p>
          ) : null}
          {!trialCaptcha ? (
            <button type="button" className="cb-second" onClick={() => void confirmTrial()} disabled={trialBusy}>
              {trialBusy ? t('Проверяем…') : t('Я не робот')}
            </button>
          ) : trialPaused ? (
            <button type="button" className="cb-second" onClick={againTrial}>
              {t('Пройти проверку ещё раз')}
            </button>
          ) : (
            <Captcha widget={trialCaptcha} theme={theme} round={trialRound} onToken={(token) => void startTrial(trialVariant, { confirm: trialCheck.token, captcha: token })} />
          )}
          {trialBusy && trialCaptcha ? (
            <p className="cb-muted" role="status">
              {t('Проверяем…')}
            </p>
          ) : null}
        </div>
      ) : null}
      <PromoBox subs={subs} />
    </Card>
  ) : (
    <Card className="cb-sub" label={t('Подписка')}>
      <div className="cb-row">
        <h1 className="cb-h1">{periodTitle(view)}</h1>
        <span className={`cb-status ${status.tone}`}>{status.text}</span>
      </div>
      <div className="cb-term">
        <span className="cb-days">{expired ? days(0) : days(left)}</span>
        <span className="cb-muted">{expire ? (expired ? t('закончилась {date}', { date: dateLong(expire, false) }) : t('до {date}', { date: dateLong(expire, true) })) : ''}</span>
      </div>
      <div className="cb-bar" role="img" aria-label={expired && expire ? t('Подписка закончилась {date}', { date: dateLong(expire, false) }) : t('Осталось {left} из {total}', { left: days(left), total })}>
        <span style={{ width: `${Math.round(share * 100)}%` }} />
      </div>
      {phone ? (
        traffic || note ? (
          <div className="cb-meta">
            {traffic ? (
              <span>
                {t('Трафик:')} <span className={traffic.exhausted ? 'cb-warn' : undefined}>{traffic.used}</span>
                {traffic.tail}
              </span>
            ) : null}
            {note ? <span>{note}</span> : null}
          </div>
        ) : null
      ) : (
        <>
          <div className="cb-facts">
            <span>
              {t('Трафик:')}{' '}
              {traffic ? (
                <>
                  <strong className={traffic.exhausted ? 'cb-warn' : undefined}>{traffic.used}</strong>
                  {traffic.tail}
                </>
              ) : (
                <strong>{t('без лимита')}</strong>
              )}
            </span>
            {view.countries.length ? (
              <span>
                {t('Серверы:')} <strong>{view.countries.map(countryName).filter(Boolean).join(', ')}</strong>
              </span>
            ) : null}
          </div>
          {note ? <p className="cb-note">{note}</p> : null}
        </>
      )}
      <div className={phone ? 'cb-btns col' : 'cb-btns'}>
        {blocked ? (
          site.support_url ? (
            <a className="cb-main" href={site.support_url}>
              {t('Написать в поддержку')}
            </a>
          ) : null
        ) : expired ? (
          <Link to={subs.link('/cabinet/pay')} className="cb-main">
            {t('Продлить')}
          </Link>
        ) : (
          <>
            {mode === 'remnawave' && url ? (
              <a className="cb-main" href={url} target="_blank" rel="noreferrer">
                {t('Открыть страницу подключения')}
                <Icon name="external" size={18} />
              </a>
            ) : (
              <button type="button" className="cb-main" onClick={() => document.getElementById('connect')?.scrollIntoView({ behavior: 'smooth' })}>
                {t('Подключить устройство')}
              </button>
            )}
            <Link to={subs.link('/cabinet/pay')} className="cb-second">
              {t('Продлить')}
            </Link>
            {me.options !== false ? (
              <Link to={subs.link('/cabinet/addons')} className="cb-second">
                {t('Докупить')}
              </Link>
            ) : null}
          </>
        )}
      </div>
      <PromoBox subs={subs} />
    </Card>
  );

  const connectCard = view ? (
    <Card id="connect" part="connect" className="cb-connect" label={t('Подключение')}>
      <h2 className="cb-h2">{t('Подключение')}</h2>
      {mode === 'remnawave' ? (
        <>
          <p className="cb-ink2">
            {t('На странице подключения есть приложения для вашего устройства и кнопки, которые сами добавят подписку.')}
            {phone ? '' : ` ${t('Чтобы подключить телефон, отсканируйте QR-код.')}`}
          </p>
          <div className={phone ? 'cb-linkrow col' : 'cb-linkrow'}>
            <span className="cb-link mono">{url ?? (detail.ready ? t('ссылка появится, когда подписка заработает') : t('загружаем ссылку…'))}</span>
            {phone && url && (blocked || expired) ? (
              <a className="cb-main" href={url} target="_blank" rel="noreferrer">
                {t('Открыть страницу подключения')}
                <Icon name="external" size={18} />
              </a>
            ) : null}
            <button type="button" className="cb-second" onClick={() => url && void copy(url, 'url')} disabled={!url}>
              {copied === 'url' ? t('Скопировано') : t('Скопировать')}
            </button>
            {!phone ? (
              <button type="button" className="cb-second" aria-expanded={qr} onClick={() => setQr(!qr)} disabled={!url}>
                {t('QR-код')}
              </button>
            ) : null}
          </div>
          {qr && url ? <QrCode text={url} size={200} label={t('QR-код ссылки подписки')} /> : null}
          {reissueBlock}
        </>
      ) : (
        <ConnectSteps
          url={url}
          phone={phone}
          brand={site.brand_name}
          copied={copied === 'url'}
          onCopy={() => url && void copy(url, 'url')}
          qr={qr}
          onQr={() => setQr(!qr)}
          reissue={reissueBlock}
          waiting={detail.ready ? t('ссылка появится, когда подписка заработает') : t('загружаем ссылку…')}
        />
      )}
    </Card>
  ) : null;

  const devicesCard = <DevicesCard view={view} devices={devices} loading={!detail.ready} onDevices={detail.setDevices} onError={setError} link={subs.link} addLink />;

  const autoSwitch = view ? (
    <button
      type="button"
      role="switch"
      className={phone ? 'cb-switch big' : 'cb-switch'}
      aria-checked={autoOn}
      aria-label={t('Автопродление')}
      onClick={() => void toggleAuto()}
      disabled={blocked || busy || renewal?.status === 'cancelling'}
    >
      <span />
    </button>
  ) : null;

  const renewalAsk =
    renewal && ask === 'renewal' ? (
      <div className="cb-ask">
        <p>
          <strong>{t('Отключить автопродление?')}</strong>{' '}
          {view?.subscription.mirror.expire_at
            ? t('{provider} больше не будет списывать деньги. Подписка доработает до {date}, продлить её можно будет вручную.', {
                provider: renewal.title,
                date: dateLong(view.subscription.mirror.expire_at, false),
              })
            : t('{provider} больше не будет списывать деньги. Продлить подписку можно будет вручную.', { provider: renewal.title })}
        </p>
        <span className={phone ? 'cb-ask-btns col' : 'cb-ask-btns'}>
          <button type="button" className="cb-main sm" onClick={() => void renewalOff()} disabled={busy}>
            {busy ? t('Отключаем…') : t('Отключить автопродление')}
          </button>
          <button type="button" className="cb-cancel" onClick={() => setAsk('')} disabled={busy}>
            {t('Оставить')}
          </button>
        </span>
      </div>
    ) : null;

  const errorLine = error ? (
    <p className="cb-error" role="alert">
      {error}
    </p>
  ) : null;

  if (phone) {
    return (
      <div className="cb-stack">
        <SubSwitch subs={subs} />
        {errorLine}
        {subCard}
        {connectCard}
        {devicesCard}
        {view ? (
          <Card className="cb-auto" label={t('Автопродление')}>
            <span className="cb-auto-text">
              <strong>{t('Автопродление')}</strong>
              <span className="cb-small cb-muted">{autoText}</span>
            </span>
            {autoSwitch}
            {renewalAsk}
          </Card>
        ) : null}
      </div>
    );
  }

  return (
    <div className="cb-cols">
      <SubSwitch subs={subs} />
      <div className="cb-col left">
        {errorLine}
        {subCard}
        {connectCard}
        {devicesCard}
      </div>
      <div className="cb-col right">
        {showBalance || view ? (
          <Card className="cb-balance-card" label={showBalance ? t('Баланс') : t('Автопродление')}>
            {showBalance ? (
              <>
                <span className="cb-row">
                  <span className="cb-label">{balanceOff ? t('Остаток на балансе') : t('Баланс')}</span>
                  <Link to="/cabinet/payments" className="cb-accent-link cb-small">
                    {t('История оплат')}
                  </Link>
                </span>
                <span className="cb-sum">{money(me.balance)}</span>
                {held ? <span className="cb-small cb-muted">{t('Удержано до ответа платёжной системы: {amount}', { amount: money(held) })}</span> : null}
                {balanceOff ? (
                  <span className="cb-small cb-muted">{t('Пополнить баланс нельзя. Остатком можно оплатить подписку, если его хватает на всю сумму')}</span>
                ) : (
                  <button type="button" className="cb-main wide" onClick={() => navigate('/cabinet/topup')}>
                    {t('Пополнить')}
                  </button>
                )}
              </>
            ) : null}
            {view ? (
              <div className={showBalance ? 'cb-auto-row' : 'cb-auto-row solo'}>
                <span className="cb-auto-text">
                  <strong>{t('Автопродление')}</strong>
                  <span className="cb-small cb-muted">{autoText}</span>
                </span>
                {autoSwitch}
              </div>
            ) : null}
            {view && renewalAsk ? (
              renewalAsk
            ) : null}
          </Card>
        ) : null}
        {referral ? (
          <Card id="friends" className="cb-friends" label={t('Друзья')}>
            <div className="cb-row">
              <h2 className="cb-h2">{t('Друзья')}</h2>
              <Link to="/cabinet/friends" className="cb-accent-link cb-small">
                {t('Подробнее')}
              </Link>
            </div>
            <p className="cb-ink2">{t('Пригласите друга и получайте {percent}% от его оплат на баланс.', { percent: referral.levels[0] ?? 0 })}</p>
            <span className="cb-small cb-muted">{t('Ссылка на сайт')}</span>
            <div className="cb-linkrow">
              <span className="cb-link mono">{siteInviteLink(referral.code).replace(/^https?:\/\//, '')}</span>
              <button type="button" className="cb-second" onClick={() => void copy(siteInviteLink(referral.code), 'ref-site')}>
                {copied === 'ref-site' ? t('Скопировано') : t('Скопировать')}
              </button>
            </div>
            {referral.bot_link ? (
              <>
                <span className="cb-small cb-muted">{t('Ссылка на Telegram-бота')}</span>
                <div className="cb-linkrow">
                  <span className="cb-link mono">{referral.bot_link.replace(/^https:\/\//, '')}</span>
                  <button type="button" className="cb-second" onClick={() => void copy(referral.bot_link ?? '', 'ref')}>
                    {copied === 'ref' ? t('Скопировано') : t('Скопировать')}
                  </button>
                </div>
              </>
            ) : null}
            <span className="cb-small cb-muted">{t('Пришли по ссылке: {count} · заработано {amount}', { count: referral.invited, amount: money(referral.earned) })}</span>
          </Card>
        ) : null}
        {supportTickets(site) ? (
          <Link id="help" className="cb-card cb-help hov" to="/cabinet/help">
            <span>
              <strong>{t('Нужна помощь?')}</strong>
              <span className="cb-small cb-muted">{t('Напишите нам — ответ появится здесь, в кабинете')}</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </Link>
        ) : supportLink(site) ? (
          <a id="help" className="cb-card cb-help hov" href={supportLink(site) ?? undefined} target="_blank" rel="noreferrer">
            <span>
              <strong>{t('Нужна помощь?')}</strong>
              <span className="cb-small cb-muted">{t('Откроется чат поддержки')}</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </a>
        ) : site.bot_username ? (
          <a id="help" className="cb-card cb-help hov" href={`https://t.me/${site.bot_username}`} target="_blank" rel="noreferrer">
            <span>
              <strong>{t('Нужна помощь?')}</strong>
              <span className="cb-small cb-muted">{t('Напишите в Telegram-бота: раздел «Поддержка»')}</span>
            </span>
            <Icon name="chevron-right" size={20} />
          </a>
        ) : null}
      </div>
    </div>
  );
}

