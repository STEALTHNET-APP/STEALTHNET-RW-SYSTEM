import { Fragment, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../lib/api';
import { BrandMark } from '../lib/brand';
import { dateLong, money, plural } from '../lib/format';
import { t } from '../lib/i18n';
import { Icon, type IconName } from '../lib/icons';
import { photoUrl, resetWhen, trafficMeter } from '../cabinet/model';
import {
  audienceOf,
  homeOn,
  imageOf,
  imageUrl,
  isSection,
  linkIcon,
  pick,
  say,
  SECTION_ICONS,
  shownTo,
  withName,
  type ActionsBlock,
  type Banner,
  type BannersBlock,
  type GreetingBlock,
  type HomeBlock,
  type SectionsBlock,
  type SubscriptionBlock,
} from './config';
import { EMPTY_TEXTS } from './labels';
import { MiniSubSwitch, ThemeCirc, useMini } from './MiniApp';
import { actionLabel, Row, rowLabel, supportTarget, useExtras, useLinkTarget, useRows, useTargets, type Extras, type Target } from './sections';
import { openExternal } from './telegram';
import { useClientPhoto } from './useClientPhoto';

const R = 90;
const LEN = 2 * Math.PI * R;

type Btn = { label: string; icon: IconName; target: Target | null };

function Act({ button, className, size, part }: { button: Btn; className: string; size: number; part?: string }) {
  const { target } = button;
  const inner = (
    <>
      <Icon name={button.icon} size={size} />
      {button.label}
    </>
  );
  return target?.to ? (
    <Link to={target.to} className={className} data-part={part}>
      {inner}
    </Link>
  ) : (
    <button type="button" className={className} data-part={part} onClick={target?.url ? () => openExternal(target.url ?? '') : undefined}>
      {inner}
    </button>
  );
}

function useStatus() {
  const { me, view } = useMini();
  return {
    blocked: me.client.status === 'blocked' || view?.status === 'blocked',
    expired: view?.status === 'expired',
  };
}

function useConnect(): Btn {
  const { site, view } = useMini();
  const mode = site.connect?.mode ?? 'ours';
  const url = view?.subscription_url ?? null;
  return mode === 'remnawave' && url ? { label: t('Открыть страницу подключения'), icon: 'external', target: { url } } : { label: t('Подключить'), icon: 'zap', target: { to: '/app/connect' } };
}

function ProfileAvatar() {
  const { me } = useMini();
  const photo = useClientPhoto(photoUrl(me));
  const [broken, setBroken] = useState<string | null>(null);
  return (
    <Link to="/app/profile" className="mn-profile-avatar press" aria-label={t('Профиль')}>
      <Icon name="user" size={20} />
      {photo && photo !== broken ? <img src={photo} alt="" decoding="async" draggable={false} onError={() => setBroken(photo)} /> : null}
    </Link>
  );
}

function Header() {
  const { me, site, config } = useMini();
  return (
    <header className={config.header.avatar ? 'mn-head with-avatar' : 'mn-head'} data-part="header">
      <BrandMark name={site.brand_name} logo={site.logo_url} className="mn-logo" />
      <h1 className="mn-brand">{site.brand_name.toUpperCase()}</h1>
      {config.header.balance && (me.balance_enabled !== false || me.balance.minor > 0) ? (
        <Link
          to={me.balance_enabled !== false ? '/app/topup' : '/app/plans'}
          className="mn-balance press"
          aria-label={me.balance_enabled !== false ? t('Баланс {amount}, пополнить', { amount: money(me.balance) }) : t('Остаток на балансе {amount}', { amount: money(me.balance) })}
        >
          <span>{money(me.balance)}</span>
        </Link>
      ) : null}
      <ThemeCirc />
      {config.header.avatar ? <ProfileAvatar /> : null}
    </header>
  );
}

function BannerCard({ banner, target }: { banner: Banner; target: Target | null }) {
  const { lang } = useMini();
  const image = imageOf(banner, lang);
  const src = imageUrl(image);
  const title = say(banner.title, lang);
  const text = say(banner.text, lang);
  const inner = (
    <>
      {src ? <img className="mn-banner-img" src={src} width={image?.width} height={image?.height} alt={title || text ? '' : isSection(banner.to) ? rowLabel(banner.to) : t('Баннер')} draggable={false} /> : null}
      {title || text ? (
        <span className="mn-banner-row">
          <span className="mn-banner-text">
            {title ? <strong>{title}</strong> : null}
            {text ? <span>{text}</span> : null}
          </span>
          {target ? <Icon name={target.url ? 'external' : 'chevron-right'} size={16} /> : null}
        </span>
      ) : null}
    </>
  );
  const cls = src ? 'mn-banner' : 'mn-banner plain';
  if (target?.to) {
    return (
      <Link to={target.to} className={`${cls} press`}>
        {inner}
      </Link>
    );
  }
  if (target?.url) {
    return (
      <button type="button" className={`${cls} press`} onClick={() => openExternal(target.url ?? '')}>
        {inner}
      </button>
    );
  }
  return <div className={cls}>{inner}</div>;
}

function Banners({ block, extras }: { block: BannersBlock; extras: Extras }) {
  const { subs, lang } = useMini();
  const link = useLinkTarget(extras);
  const state = audienceOf(subs);
  const shown = (block.items ?? []).flatMap((banner, index) => {
    if (banner.on === false || !shownTo(banner.for, state) || (!imageOf(banner, lang) && !say(banner.title, lang))) {
      return [];
    }
    const target = banner.to ? link(banner.to) : null;
    return banner.to && !target ? [] : [{ banner, target, index }];
  });
  if (shown.length === 0) {
    return null;
  }
  const [only] = shown;
  if (shown.length === 1 && only) {
    return (
      <div className="mn-banner-one" data-part="banners">
        <BannerCard banner={only.banner} target={only.target} />
      </div>
    );
  }
  return (
    <div role="group" aria-label={t('Новости')} className="mn-banners" data-part="banners">
      {shown.map(({ banner, target, index }) => (
        <BannerCard key={index} banner={banner} target={target} />
      ))}
    </div>
  );
}

function Greeting({ block }: { block: GreetingBlock }) {
  const { subs, lang, name } = useMini();
  if (!shownTo(block.for, audienceOf(subs))) {
    return null;
  }
  const title = withName(say(block.title, lang), name);
  const text = withName(say(block.text, lang), name);
  if (!title && !text) {
    return null;
  }
  return (
    <section className="mn-greet" data-part="greeting">
      {title ? <h2>{title}</h2> : null}
      {text ? <p>{text}</p> : null}
    </section>
  );
}

function Ring({ left, share, size }: { left: number; share: number; size: 'big' | 'small' }) {
  return (
    <>
      <svg viewBox="0 0 208 208" width={size === 'big' ? 208 : 76} height={size === 'big' ? 208 : 76} aria-hidden="true">
        <circle cx="104" cy="104" r={R} fill="none" stroke="var(--track)" strokeWidth={size === 'big' ? 12 : 18} />
        {share > 0 ? (
          <circle
            cx="104"
            cy="104"
            r={R}
            fill="none"
            stroke="var(--accent)"
            strokeWidth={size === 'big' ? 12 : 18}
            strokeLinecap="round"
            strokeDasharray={LEN}
            strokeDashoffset={LEN * (1 - share)}
            transform="rotate(-90 104 104)"
          />
        ) : null}
      </svg>
      <span className="mn-ring-in">
        <strong>{left}</strong>
        <span>{size === 'big' ? plural(left, 'день остался', 'дня осталось', 'дней осталось') : plural(left, 'день', 'дня', 'дней')}</span>
      </span>
    </>
  );
}

function Subscription({ block, extras }: { block: SubscriptionBlock; extras: Extras }) {
  const { me, site, view, lang } = useMini();
  const { blocked, expired } = useStatus();
  const connect = useConnect();
  const limited = view?.status === 'limited';
  const left = view?.progress?.days_left ?? 0;
  const total = view?.progress?.days_total ?? 0;
  const share = !view || expired || !total ? 0 : Math.min(1, left / total);
  const expire = view?.subscription.mirror.expire_at ?? null;
  const name = view?.tariff?.name ?? (view?.subscription.kind === 'trial' ? t('Пробный период') : t('Подписка'));
  const meter = view && !expired && !blocked ? trafficMeter(view) : null;
  const tight = !!meter && (meter.exhausted || meter.share >= 0.9);
  const reset = view?.tariff?.traffic.reset ?? 'no_reset';
  const compact = block.look === 'compact';
  const shownLeft = view && !expired ? left : 0;

  const line2 = !view
    ? { text: pick(block.empty?.line, lang, t(EMPTY_TEXTS.line)), tone: 'muted' }
    : blocked
      ? { text: t('Подписка заблокирована'), tone: 'bad' }
      : limited
        ? { text: reset !== 'no_reset' ? t('Трафик закончился, {when}', { when: resetWhen(reset, view.subscription).tail }) : t('Трафик закончился'), tone: 'warn' }
        : expired
          ? { text: t('Истекла'), tone: 'muted' }
          : {
              text: view.subscription.autorenew.enabled || view.renewal?.status === 'active' ? t('Активна, автопродление включено') : t('Активна'),
              tone: 'ink2',
            };

  const main: Btn = !view
    ? { label: pick(block.empty?.button, lang, t(EMPTY_TEXTS.button)), icon: 'tag', target: { to: '/app/plans' } }
    : blocked
      ? { label: t('Написать в поддержку'), icon: 'chat', target: supportTarget(site) }
      : expired
        ? { label: t('Продлить'), icon: 'refresh', target: { to: '/app/plans' } }
        : connect;

  const title = view
    ? expire
      ? expired
        ? t('{name} · закончилась {date}', { name, date: dateLong(expire, false) })
        : t('{name} · до {date}', { name, date: dateLong(expire, false) })
      : `${name} · `
    : pick(block.empty?.title, lang, t(EMPTY_TEXTS.title));
  const ringLabel = expired && expire ? t('Подписка закончилась {date}', { date: dateLong(expire, false) }) : t('Осталось {left} из {total}', { left, total });
  const lines = (
    <span className="mn-lines">
      <strong>{title}</strong>
      {line2.text ? <span className={`mn-tone ${line2.tone}`}>{line2.text}</span> : null}
    </span>
  );

  return (
    <>
      <MiniSubSwitch part="subscription" />
      {compact ? (
        <section aria-label={t('Подписка')} className="mn-sub compact" data-part="subscription">
          {view ? (
            <div className="mn-ring small" role="img" aria-label={ringLabel}>
              <Ring left={shownLeft} share={share} size="small" />
            </div>
          ) : null}
          {lines}
        </section>
      ) : (
        <section aria-label={t('Подписка')} className="mn-sub" data-part="subscription">
          <div className="mn-ring" role="img" aria-label={ringLabel}>
            <Ring left={shownLeft} share={share} size="big" />
          </div>
          {lines}
        </section>
      )}

      {meter ? (
        <section aria-label={t('Трафик')} className="mn-traffic" data-part="subscription">
          <span className="mn-traffic-top">
            <span className="mn-traffic-name">{t('Трафик')}</span>
            <strong className={tight ? 'warn' : undefined}>{meter.exhausted ? t('закончился') : t('осталось {amount}', { amount: meter.left })}</strong>
          </span>
          <span
            className={tight ? 'mn-traffic-bar warn' : 'mn-traffic-bar'}
            role="progressbar"
            aria-label={t('Израсходовано трафика')}
            aria-valuemin={0}
            aria-valuemax={100}
            aria-valuenow={Math.round(meter.share * 100)}
            aria-valuetext={t('{n} из {limit}', { n: meter.used, limit: meter.limit })}
          >
            <span style={{ width: `${meter.share > 0 ? Math.max(meter.share * 100, 2) : 0}%` }} />
          </span>
          <span className="mn-traffic-sub">
            {t('{n} из {limit}', { n: meter.used, limit: meter.limit })} · {meter.tail}
          </span>
          {tight && me.options !== false ? (
            <Link to="/app/addons?kind=traffic" className="mn-link press">
              {t('Докупить трафик')}
            </Link>
          ) : null}
        </section>
      ) : null}

      {view && !blocked ? <RenewalRow /> : null}

      <Act button={main} className="mn-main press" size={20} part="subscription" />
      {!view && extras.trial ? (
        <Link to="/app/trial" className="mn-pairbtn mn-trialbtn press" data-part="subscription">
          <Icon name="gift" size={18} />
          {t('Попробовать бесплатно')}
        </Link>
      ) : null}
    </>
  );
}

function RenewalRow() {
  const { view, setView } = useMini();
  const [ask, setAsk] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const renewal = view?.renewal && (view.renewal.status === 'active' || view.renewal.status === 'cancelling') ? view.renewal : null;
  if (!view || !renewal) {
    return null;
  }
  const expire = view.subscription.mirror.expire_at;
  const text =
    renewal.status === 'cancelling'
      ? t('Отключаем автопродление в {provider}', { provider: renewal.title })
      : renewal.amount && renewal.next_charge_at
        ? t('{provider} спишет {amount} около {date}', { provider: renewal.title, amount: money(renewal.amount), date: dateLong(renewal.next_charge_at, false) })
        : t('Продлевает {provider}', { provider: renewal.title });
  const turnOff = async () => {
    setBusy(true);
    setError(null);
    try {
      await api(`/api/client/subscriptions/${view.subscription.id}/autorenew`, {
        method: 'PUT',
        body: { enabled: false, period_id: view.subscription.autorenew.period_id },
      });
      const fresh = await api<typeof view>(`/api/client/subscriptions/${view.subscription.id}`).catch(() => null);
      if (fresh) {
        setView(fresh);
      }
      setAsk(false);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : t('Автопродление не отключилось'));
    } finally {
      setBusy(false);
    }
  };
  return (
    <section aria-label={t('Автопродление')} className="mn-traffic" data-part="subscription">
      <span className="mn-traffic-top">
        <span className="mn-traffic-name">{t('Автопродление')}</span>
      </span>
      <span className="mn-traffic-sub">{text}</span>
      {ask ? (
        <>
          <span className="mn-traffic-sub">
            {expire
              ? t('{provider} больше не будет списывать деньги. Подписка доработает до {date}, продлить её можно будет вручную.', {
                  provider: renewal.title,
                  date: dateLong(expire, false),
                })
              : t('{provider} больше не будет списывать деньги. Продлить подписку можно будет вручную.', { provider: renewal.title })}
          </span>
          <div className="mn-pair">
            <button type="button" className="mn-pairbtn press" onClick={() => void turnOff()} disabled={busy}>
              {busy ? t('Отключаем…') : t('Отключить автопродление')}
            </button>
            <button type="button" className="mn-pairbtn press" onClick={() => setAsk(false)} disabled={busy}>
              {t('Оставить')}
            </button>
          </div>
        </>
      ) : renewal.status === 'active' ? (
        <button type="button" className="mn-link press" onClick={() => setAsk(true)}>
          {t('Отключить автопродление')}
        </button>
      ) : null}
      {error ? (
        <span className="mn-tone bad" role="alert">
          {error}
        </span>
      ) : null}
    </section>
  );
}

function Actions({ block, extras }: { block: ActionsBlock; extras: Extras }) {
  const { view, devices, subs, lang } = useMini();
  const { expired } = useStatus();
  const connect = useConnect();
  const target = useTargets(extras);
  const link = useLinkTarget(extras);
  const state = audienceOf(subs);
  const limit = view?.subscription.mirror.hwid_limit ?? view?.tariff?.devices.included ?? null;
  const devCount = devices?.length ?? view?.subscription.mirror.devices_count ?? 0;
  const count = limit ? `${devCount}/${limit}` : String(devCount);

  const shown = (block.items ?? []).flatMap((item, index): Array<Btn & { key: string }> => {
    if (!item.std) {
      const at = shownTo(item.for, state) ? link(item.to) : null;
      const label = say(item.label, lang);
      return at && label ? [{ key: `own-${index}`, label, icon: linkIcon(item), target: at }] : [];
    }
    const at = target(item.std);
    if (!at) {
      return [];
    }
    if (item.std === 'renew' && expired) {
      return [{ key: item.std, ...connect, label: t('Подключить') }];
    }
    if (item.std === 'devices') {
      const own = pick(item.label, lang, '');
      return [{ key: item.std, label: own ? `${own} ${count}` : t('Устройства {count}', { count }), icon: 'phone', target: at }];
    }
    return [{ key: item.std, label: pick(item.label, lang, actionLabel(item.std)), icon: SECTION_ICONS[item.std], target: at }];
  });
  if (shown.length === 0) {
    return null;
  }
  return (
    <div className={shown.length % 2 ? 'mn-pair odd' : 'mn-pair'} data-part="actions">
      {shown.map(({ key, ...button }) => (
        <Act key={key} button={button} className="mn-pairbtn press" size={18} />
      ))}
    </div>
  );
}

function Sections({ block, extras }: { block: SectionsBlock; extras: Extras }) {
  const { subs } = useMini();
  const rows = useRows(extras, audienceOf(subs))(block.items ?? []);
  if (rows.length === 0) {
    return null;
  }
  return (
    <nav aria-label={t('Ещё')} className="mn-list" data-part="sections">
      {rows.map((row) => (
        <Row key={row.key} row={row} />
      ))}
    </nav>
  );
}

export function MiniHome() {
  const { config } = useMini();
  const extras = useExtras();
  const blocks = (block: HomeBlock): ReactNode => {
    switch (block.kind) {
      case 'banners':
        return <Banners block={block} extras={extras} />;
      case 'greeting':
        return <Greeting block={block} />;
      case 'subscription':
        return <Subscription block={block} extras={extras} />;
      case 'actions':
        return <Actions block={block} extras={extras} />;
      case 'sections':
        return <Sections block={block} extras={extras} />;
    }
  };
  return (
    <main className="mn-page home">
      <Header />
      {config.home.filter(homeOn).map((block) => (
        <Fragment key={block.kind}>{blocks(block)}</Fragment>
      ))}
    </main>
  );
}
