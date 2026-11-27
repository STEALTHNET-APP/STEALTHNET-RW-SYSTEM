import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../lib/api';
import { money } from '../lib/format';
import { t } from '../lib/i18n';
import type { Referral } from '../cabinet/model';
import { levelRows, siteInviteLink } from '../lib/referral';
import { pick, profileBlock, profileOn, sectionPath, withPercent } from './config';
import { FRIENDS_TEXTS } from './labels';
import { BackHead, useMini } from './MiniApp';

export function useReferral(): Referral | null | undefined {
  const { demo } = useMini();
  const [referral, setReferral] = useState<Referral | null | undefined>(undefined);
  useEffect(() => {
    if (demo) {
      return;
    }
    api<Referral>('/api/client/m/referral/me')
      .then(setReferral)
      .catch(() => setReferral(null));
  }, [demo]);
  return demo ? demo.referral : referral;
}

export function useFriendsText(): { title: string; text: (percent: number) => string } {
  const { config, lang } = useMini();
  const block = profileBlock(config, 'friends');
  return {
    title: pick(block.title, lang, t(FRIENDS_TEXTS.title)),
    text: (percent) => withPercent(pick(block.text, lang, t(FRIENDS_TEXTS.text)), percent),
  };
}

export function FriendsCard({ referral, heading, part }: { referral: Referral; heading: boolean; part?: string }) {
  const [copied, setCopied] = useState<string | null>(null);
  const { title, text } = useFriendsText();
  const rows = levelRows(referral.levels);
  const line = rows.length ? text(referral.levels[0] ?? 0) : '';
  const site = siteInviteLink(referral.code);

  const copy = async (value: string, key: string) => {
    try {
      await navigator.clipboard.writeText(value);
      setCopied(key);
      window.setTimeout(() => setCopied(null), 2000);
    } catch {
      return;
    }
  };

  return (
    <div className="mn-card mn-pad" data-part={part}>
      {heading ? <strong>{title}</strong> : null}
      {line ? <span className="mn-muted small mn-lines-text">{line}</span> : null}
      {rows.length ? (
        <dl className="mn-levels">
          {rows.map((row) => (
            <div key={row.label}>
              <dt>{row.label}</dt>
              <dd>{row.percent}%</dd>
            </div>
          ))}
        </dl>
      ) : null}
      {referral.bot_link ? (
        <>
          <span className="mn-muted small">{t('Ссылка на Telegram-бота')}</span>
          <span className="mn-url mono">{referral.bot_link.replace(/^https:\/\//, '')}</span>
          <button type="button" className="mn-btn2 press" onClick={() => void copy(referral.bot_link ?? '', 'bot')}>
            {copied === 'bot' ? t('Скопировано') : t('Скопировать ссылку')}
          </button>
        </>
      ) : null}
      <span className="mn-muted small">{t('Ссылка на сайт')}</span>
      <span className="mn-url mono">{site.replace(/^https?:\/\//, '')}</span>
      <button type="button" className="mn-btn2 press" onClick={() => void copy(site, 'site')}>
        {copied === 'site' ? t('Скопировано') : t('Скопировать ссылку')}
      </button>
      <span className="mn-muted small">{t('Пришли по ссылке: {count} · заработано {amount}', { count: referral.invited, amount: money(referral.earned) })}</span>
      {referral.withdrawals_enabled && referral.balance && referral.balance.minor > 0 ? (
        <Link to="/app/withdraw" className="mn-btn2 press mn-btn-link">
          {t('Вывести {amount}', { amount: money(referral.balance) })}
        </Link>
      ) : (referral.withdrawals?.length ?? 0) > 0 ? (
        <Link to="/app/withdraw" className="mn-link press">
          {t('Заявки на вывод')}
        </Link>
      ) : null}
    </div>
  );
}

export function MiniFriends() {
  const { config } = useMini();
  const referral = useReferral();
  const { title } = useFriendsText();
  const tab = config.tabs.some((item) => item.std === 'friends');
  const back = !tab && profileOn(profileBlock(config, 'friends')) ? sectionPath('profile') : sectionPath('home');
  return (
    <main className="mn-page">
      <BackHead title={title} to={back} />
      {referral === undefined ? <p className="mn-muted">{t('Загружаем…')}</p> : null}
      {referral === null ? <p className="mn-note">{t('Приглашать друзей сейчас нельзя.')}</p> : null}
      {referral ? <FriendsCard referral={referral} heading={false} part="friends" /> : null}
    </main>
  );
}
