import { Fragment, useState, type ReactNode } from 'react';
import { Link } from 'react-router';
import { documentTitle, documentUrl } from '../lib/documents';
import { money, plural } from '../lib/format';
import { t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { supportLink, supportTickets } from '../lib/support';
import { heldBalance, photoUrl } from '../cabinet/model';
import { audienceOf, profileOn, SECTION_ICONS, sectionPath, type FriendsBlock, type HelpBlock, type LinksBlock, type ProfileBlock } from './config';
import { AccountCard } from './MiniAccount';
import { BackHead, useMini } from './MiniApp';
import { FriendsCard, useFriendsText, useReferral } from './MiniFriends';
import { needsExtras, Row, rowLabel, unreadText, useExtras, useGifts, useRows } from './sections';
import { openExternal } from './telegram';
import { useClientPhoto } from './useClientPhoto';

function Person() {
  const { me, name, username } = useMini();
  const photo = useClientPhoto(photoUrl(me));
  const [broken, setBroken] = useState<string | null>(null);
  return (
    <div className="mn-person" data-part="person">
      <span className="mn-person-photo" aria-hidden="true">
        <Icon name="user" size={36} />
        {photo && photo !== broken ? <img src={photo} alt="" decoding="async" draggable={false} onError={() => setBroken(photo)} /> : null}
      </span>
      {name ? <strong>{name}</strong> : null}
      {username ? <span className="mn-muted">@{username}</span> : null}
    </div>
  );
}

function Balance({ shown }: { shown: boolean }) {
  const { me } = useMini();
  const held = heldBalance(me);
  const kept = me.balance.minor > 0 || !!held;
  if (me.balance_enabled !== false) {
    return shown || kept ? (
      <div className="mn-card mn-pad" data-part="balance">
        <span className="mn-muted">{t('Баланс')}</span>
        <strong className="mn-big">{money(me.balance)}</strong>
        {held ? <span className="mn-muted small">{t('Удержано до ответа платёжной системы: {amount}', { amount: money(held) })}</span> : null}
        <Link to="/app/topup" className="mn-btn2 press mn-btn-link">
          {t('Пополнить')}
        </Link>
      </div>
    ) : null;
  }
  return kept ? (
    <div className="mn-card mn-pad" data-part="balance">
      <span className="mn-muted">{t('Остаток на балансе')}</span>
      <strong className="mn-big">{money(me.balance)}</strong>
      {held ? <span className="mn-muted small">{t('Удержано до ответа платёжной системы: {amount}', { amount: money(held) })}</span> : null}
      <span className="mn-muted small">{t('Пополнить баланс нельзя. Остатком можно оплатить подписку, если его хватает на всю сумму')}</span>
    </div>
  ) : null;
}

function Friends({ block }: { block: FriendsBlock }) {
  const referral = useReferral();
  const { title } = useFriendsText();
  if (!referral) {
    return null;
  }
  if (block.place !== 'screen') {
    return <FriendsCard referral={referral} heading part="friends" />;
  }
  return (
    <div className="mn-list" data-part="friends">
      <Link to={sectionPath('friends')} className="mn-row press">
        <Icon name="share" size={21} />
        <span className="mn-row-name">{title}</span>
        {referral.invited ? (
          <span className="mn-row-val num">
            {referral.invited} {plural(referral.invited, 'пришёл', 'пришли', 'пришли')}
          </span>
        ) : null}
        <Icon name="chevron-right" size={16} />
      </Link>
    </div>
  );
}

function Help({ block }: { block: HelpBlock }) {
  const { me, site, unread } = useMini();
  const gifts = useGifts(block.gifts === true);
  const link = supportLink(site);
  const tickets = supportTickets(site);
  return (
    <div className="mn-list" data-part="help">
      {gifts ? <Row row={{ key: 'gifts', label: rowLabel('gifts'), icon: SECTION_ICONS.gifts, target: { to: sectionPath('gifts') }, away: false }} /> : null}
      <Link to="/app/payments" className="mn-row press">
        <Icon name="receipt" size={21} />
        <span className="mn-row-name">{t('История оплат')}</span>
        {me.payments_count ? <span className="mn-row-val num">{me.payments_count}</span> : null}
        <Icon name="chevron-right" size={16} />
      </Link>
      {tickets ? (
        <Link to={sectionPath('support')} className="mn-row press">
          <Icon name="chat" size={21} />
          <span className="mn-row-name">{link ? t('Написать в поддержку') : t('Поддержка')}</span>
          {unread > 0 ? <span className="mn-row-val num fresh">{unreadText(unread)}</span> : null}
          <Icon name="chevron-right" size={16} />
        </Link>
      ) : null}
      {link ? (
        <button type="button" className="mn-row press" onClick={() => openExternal(link)}>
          <Icon name="chat" size={21} />
          <span className="mn-row-name">{tickets ? t('Чат поддержки') : t('Поддержка')}</span>
          <Icon name="chevron-right" size={16} />
        </button>
      ) : null}
    </div>
  );
}

function Links({ block }: { block: LinksBlock }) {
  const { subs } = useMini();
  const items = block.items ?? [];
  const extras = useExtras(needsExtras(items));
  const rows = useRows(extras, audienceOf(subs))(items);
  if (rows.length === 0) {
    return null;
  }
  return (
    <nav aria-label={t('Ссылки')} className="mn-list" data-part="links">
      {rows.map((row) => (
        <Row key={row.key} row={row} />
      ))}
    </nav>
  );
}

function Documents() {
  const { site } = useMini();
  const documents = site.documents ?? [];
  if (documents.length === 0) {
    return null;
  }
  return (
    <div className="mn-list" aria-label={t('Документы')} data-part="documents">
      {documents.map((doc) => (
        <button key={doc.url} type="button" className="mn-row press" onClick={() => openExternal(documentUrl(doc))}>
          <Icon name="file" size={21} />
          <span className="mn-row-name">{documentTitle(doc)}</span>
          <Icon name="external" size={16} />
        </button>
      ))}
    </div>
  );
}

export function MiniProfile() {
  const { config } = useMini();
  const blocks = (block: ProfileBlock): ReactNode => {
    switch (block.kind) {
      case 'person':
        return profileOn(block) ? <Person /> : null;
      case 'balance':
        return <Balance shown={profileOn(block)} />;
      case 'friends':
        return profileOn(block) ? <Friends block={block} /> : null;
      case 'help':
        return <Help block={block} />;
      case 'links':
        return profileOn(block) ? <Links block={block} /> : null;
      case 'documents':
        return <Documents />;
      case 'account':
        return <AccountCard part="account" />;
    }
  };
  return (
    <main className="mn-page profile" data-part="profile">
      <BackHead title={t('Профиль')} />
      {config.profile.map((block) => (
        <Fragment key={block.kind}>{blocks(block)}</Fragment>
      ))}
    </main>
  );
}
