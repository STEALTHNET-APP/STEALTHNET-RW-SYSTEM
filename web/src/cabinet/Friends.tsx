import { useCallback, useEffect, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { dateLong, money } from '../lib/format';
import { t } from '../lib/i18n';
import { deeperLevels, siteInviteLink } from '../lib/referral';
import { Card, useCabinet } from './CabinetApp';
import type { Referral, Withdrawal } from './model';
import { firstPayout, payoutDetails, payoutLabel as methodLabel, WITHDRAWAL_STATUS as STATUS } from './withdrawals';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function shareUrl(link: string, brand: string): string {
  const text = t('Пользуюсь {brand} — подключайся по моей ссылке', { brand });
  return `https://t.me/share/url?url=${encodeURIComponent(link)}&text=${encodeURIComponent(text)}`;
}

export function FriendsPage() {
  const { site, me } = useCabinet();
  const [data, setData] = useState<Referral | null | undefined>(undefined);
  const [error, setError] = useState<string | null>(null);
  const [copied, setCopied] = useState<string | null>(null);
  const [amount, setAmount] = useState('');
  const [method, setMethod] = useState('');
  const [details, setDetails] = useState('');
  const [busy, setBusy] = useState(false);
  const [status, setStatus] = useState<{ text: string; bad: boolean } | null>(null);

  const load = useCallback(async () => {
    try {
      setData(await api<Referral>('/api/client/m/referral/me'));
      setError(null);
    } catch (err) {
      if (err instanceof ApiError && (err.status === 404 || err.status === 403)) {
        setData(null);
      } else {
        setError(failText(err, t('Программа приглашений не загрузилась')));
        setData((d) => (d === undefined ? null : d));
      }
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const copy = async (text: string, key: string) => {
    try {
      await navigator.clipboard.writeText(text);
      setCopied(key);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      setError(t('Браузер не дал скопировать: выделите ссылку вручную'));
    }
  };

  if (data === undefined) {
    return <p className="cb-muted cb-pad">{t('Загружаем…')}</p>;
  }

  if (data === null) {
    return (
      <div className="cb-history">
        <header className="cb-pay-head">
          <h1 className="cb-title">{t('Друзья')}</h1>
        </header>
        {error ? (
          <p className="cb-error" role="alert">
            {error}
          </p>
        ) : (
          <Card label={t('Друзья')}>
            <p className="cb-ink2">{t('Программа приглашений сейчас не работает.')}</p>
          </Card>
        )}
      </div>
    );
  }

  const first = data.levels[0] ?? 0;
  const deeper = deeperLevels(data.levels);
  const siteLink = siteInviteLink(data.code);
  const balance = data.balance ?? me.referral_balance;
  const methods = data.payout_methods ?? [];
  const chosen = methods.find((m) => m.id === method) ?? methods.find((m) => m.id === firstPayout(methods, me.currency)) ?? null;
  const canWithdraw = data.withdrawals_enabled === true && methods.length > 0;
  const withdrawals = data.withdrawals ?? [];

  const submit = async () => {
    if (busy) {
      return;
    }
    if (!chosen) {
      setStatus({ text: t('Выберите, куда вывести'), bad: true });
      return;
    }
    if (!amount.trim() || !details.trim()) {
      setStatus({ text: t('Укажите сумму и реквизиты'), bad: true });
      return;
    }
    setBusy(true);
    setStatus(null);
    try {
      await api('/api/client/m/referral/withdrawals', { method: 'POST', body: { amount: amount.trim().replace(',', '.'), method: chosen.id, details: details.trim() } });
      setAmount('');
      setDetails('');
      setStatus({ text: t('Заявка отправлена. Деньги придут после проверки, статус виден ниже.'), bad: false });
      await load();
    } catch (err) {
      setStatus({ text: failText(err, t('Заявка не отправилась')), bad: true });
    } finally {
      setBusy(false);
    }
  };

  const cancel = async (item: Withdrawal) => {
    setBusy(true);
    try {
      await api(`/api/client/m/referral/withdrawals/${item.id}/cancel`, { method: 'POST', body: {} });
      await load();
    } catch (err) {
      setError(failText(err, t('Заявка не отменилась')));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <h1 className="cb-title">{t('Друзья')}</h1>
        <p className="cb-lead muted">
          {first > 0 ? t('Пригласите друга и получайте {percent}% от его оплат.', { percent: first }) : t('Пригласите друга по своей ссылке.')}
          {deeper.length ? ` ${deeper.join(' ')}` : ''}
        </p>
      </header>
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}

      <Card className="cb-friends" label={t('Ссылки для приглашения')}>
        <h2 className="cb-h2">{t('Ваша ссылка')}</h2>
        <span className="cb-small cb-muted">{t('На сайт')}</span>
        <div className="cb-linkrow">
          <span className="cb-link mono">{siteLink.replace(/^https?:\/\//, '')}</span>
          <button type="button" className="cb-second" onClick={() => void copy(siteLink, 'site')}>
            {copied === 'site' ? t('Скопировано') : t('Скопировать')}
          </button>
        </div>
        {data.bot_link ? (
          <>
            <span className="cb-small cb-muted">{t('В Telegram-бота')}</span>
            <div className="cb-linkrow">
              <span className="cb-link mono">{data.bot_link.replace(/^https:\/\//, '')}</span>
              <button type="button" className="cb-second" onClick={() => void copy(data.bot_link ?? '', 'bot')}>
                {copied === 'bot' ? t('Скопировано') : t('Скопировать')}
              </button>
            </div>
          </>
        ) : null}
        <div className="cb-btns">
          <a className="cb-second" href={shareUrl(data.bot_link ?? siteLink, site.brand_name)} target="_blank" rel="noreferrer">
            {t('Отправить в Telegram')}
          </a>
        </div>
      </Card>

      <Card className="cb-refstats" label={t('Итоги')}>
        <dl className="cb-facts">
          <div>
            <dt>{t('Пришли по ссылке')}</dt>
            <dd className="num">{data.invited}</dd>
          </div>
          <div>
            <dt>{t('Заработано')}</dt>
            <dd className="num">{money(data.earned)}</dd>
          </div>
          <div>
            <dt>{t('Можно вывести')}</dt>
            <dd className="num">{money(balance)}</dd>
          </div>
        </dl>
      </Card>

      {canWithdraw && balance.minor > 0 ? (
        <Card className="cb-withdraw" label={t('Вывод')}>
          <h2 className="cb-h2">{t('Вывести заработанное')}</h2>
          {chosen?.min ? <p className="cb-small cb-muted">{t('От {amount}. Заявку проверяют вручную, обычно за день-два.', { amount: money(chosen.min) })}</p> : null}
          <label className="cb-field">
            {t('Сумма')}
            <span className="cb-wamount">
              <input inputMode="decimal" value={amount} onChange={(e) => setAmount(e.target.value)} placeholder="0" disabled={busy} />
              <button type="button" className="cb-text" onClick={() => setAmount(String(balance.minor / 100))} disabled={busy || balance.minor <= 0}>
                {t('Всё')}
              </button>
            </span>
          </label>
          <span className="cb-field">
            {t('Куда')}
            <span role="group" aria-label={t('Способ вывода')} className="cb-seg cb-seg-wrap">
              {methods.map((m) => (
                <button key={m.id} type="button" aria-pressed={chosen?.id === m.id} onClick={() => setMethod(m.id)} disabled={busy}>
                  {m.title}
                </button>
              ))}
            </span>
          </span>
          <label className="cb-field">
            {t('Реквизиты')}
            <input value={details} onChange={(e) => setDetails(e.target.value)} placeholder={chosen?.hint ?? ''} maxLength={200} disabled={busy} />
          </label>
          {status ? (
            <p className={status.bad ? 'cb-error' : 'cb-small cb-muted'} role={status.bad ? 'alert' : 'status'}>
              {status.text}
            </p>
          ) : null}
          <div className="cb-btns">
            <button type="button" className="cb-main" onClick={() => void submit()} disabled={busy}>
              {busy ? t('Отправляем…') : t('Отправить заявку')}
            </button>
          </div>
        </Card>
      ) : balance.minor > 0 ? (
        <p className="cb-small cb-muted">{t('Вывод заработанного сейчас не принимается.')}</p>
      ) : null}

      {withdrawals.length > 0 ? (
        <Card className="cb-paylist" label={t('Заявки на вывод')}>
          <ul className="cb-payrows">
            {withdrawals.map((w) => {
              const st = STATUS[w.status];
              const open = w.status === 'holding' || w.status === 'pending';
              return (
                <li key={w.id} className="cb-payrow">
                  <span className="cb-payrow-main">
                    <strong>
                      {methodLabel(w)} · {payoutDetails(w.details)}
                    </strong>
                    <span className="cb-small cb-muted">
                      {dateLong(w.created_at, true)}
                      {w.comment ? ` · ${w.comment}` : ''}
                    </span>
                  </span>
                  <span className="cb-payrow-side">
                    <strong className="num">{money(w.amount)}</strong>
                    <span className={`cb-small cb-status ${st.tone}`}>{t(st.text)}</span>
                    {open ? (
                      <button type="button" className="cb-text" onClick={() => void cancel(w)} disabled={busy}>
                        {t('Отменить')}
                      </button>
                    ) : null}
                  </span>
                </li>
              );
            })}
          </ul>
        </Card>
      ) : null}
    </div>
  );
}
