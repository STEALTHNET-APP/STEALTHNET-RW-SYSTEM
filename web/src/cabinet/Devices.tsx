import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { count, money } from '../lib/format';
import { t } from '../lib/i18n';
import { Card, useCabinet } from './CabinetApp';
import { DevicesCard, deviceLimit } from './DevicesCard';
import { stars } from './model';
import { SubSwitch, useSubDetail, useSubs } from './SubSwitch';

export function DevicesPage() {
  const { me } = useCabinet();
  const subs = useSubs();
  const detail = useSubDetail(subs.chosen);
  const view = subs.list === undefined ? undefined : detail.view;
  const devices = detail.devices;
  const chosenId = subs.chosen?.subscription.id ?? null;
  const [pageError, setError] = useState<string | null>(null);
  const error = pageError ?? subs.error;

  useEffect(() => {
    setError(null);
  }, [chosenId]);

  if (view === undefined) {
    return <p className="cb-muted cb-pad">{t('Загружаем устройства…')}</p>;
  }

  const limit = deviceLimit(view);
  const extra = view?.tariff?.devices.extra ?? null;
  const included = view?.tariff?.devices.included ?? null;
  const used = devices?.length ?? view?.subscription.mirror.devices_count ?? 0;
  const monthly = extra ? (me.currency === 'RUB' ? extra.monthly.rub : extra.monthly.usd) : undefined;
  const price = monthly !== undefined ? money({ minor: monthly, currency: me.currency }) : extra?.monthly_stars ? stars(extra.monthly_stars) : null;
  const ended = view?.status === 'expired' || view?.status === 'blocked';

  return (
    <div className="cb-history">
      <header className="cb-pay-head">
        <h1 className="cb-title">{t('Устройства')}</h1>
        {view && limit && used >= limit ? <p className="cb-lead muted">{t('Все места заняты: чтобы подключить новое устройство, отключите одно из старых.')}</p> : null}
      </header>
      <SubSwitch subs={subs} />
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      <DevicesCard view={view} devices={devices} loading={!detail.ready} onDevices={detail.setDevices} onError={setError} link={subs.link} />
      {view ? (
        <Card className="cb-devhelp" label={t('Новое устройство')}>
          <h2 className="cb-h2">{t('Новое устройство')}</h2>
          <p className="cb-ink2">{t('Установите приложение и добавьте в него подписку: ссылка, QR-код и шаги — на главной странице кабинета. Устройство появится в списке после первого подключения.')}</p>
          <div className="cb-btns">
            <Link to={subs.link('/cabinet#connect')} className="cb-second">
              {t('Как подключить')}
            </Link>
            {extra && !ended && me.options !== false ? (
              <Link to={subs.link('/cabinet/addons?kind=devices')} className="cb-second">
                {t('Добавить устройства')}
              </Link>
            ) : null}
          </div>
          {included !== null ? (
            <p className="cb-small cb-muted">
              {t('В тариф входит {devices}.', { devices: count(included, 'устройство', 'устройства', 'устройств') })}
              {extra && extra.max > 0 ? ` ${price ? t('Можно добавить ещё до {max}, {price} в месяц за каждое.', { max: extra.max, price }) : t('Можно добавить ещё до {max}.', { max: extra.max })}` : ''}
            </p>
          ) : null}
        </Card>
      ) : null}
    </div>
  );
}
