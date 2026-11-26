import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { Link, useNavigate, useParams, useSearchParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { count, dateLong, days, daysLeft, money, type Money } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { useSession } from '../session';
import { ClientProtect } from '../protect/ClientProtect';
import type { Subscription } from '../clients/types';
import { ClientPhone } from './ClientPhone';
import { BalanceDialog, BlockDialog, ConfirmDialog, GrantDialog, MessageDialog } from './dialogs';
import { EmailDialog, EmailRow } from './EmailDialog';
import { eventsCount, historyLine } from './history';
import { PasswordDialog } from './PasswordDialog';
import { RefundDialog } from './RefundDialog';
import { RemnaAddButton } from './RemnaLink';
import { emailOf, liveSubscriptions, nextReset, oauthLogins, personName, seenText, sinceText, tariffOf, telegramOf, visitText } from './model';
import { SubscriptionBlock } from './SubscriptionBlock';
import type { ClientCard, HistoryPage, SubscriptionDetails } from './types';
import './client.css';
import { registrationText, useClientUtm, UtmValue } from '../utm/ClientSource';
import { Avatar } from '../Avatar';
import { ExtendDialog } from './ExtendDialog';

const FIRST_PAGE = 5;
const MORE_PAGE = 20;

type Dialogs = '' | 'message' | 'balance-add' | 'balance-edit' | 'block' | 'grant' | 'change' | 'extend' | 'delete-sub' | 'delete-client' | 'reset-2fa' | 'password' | 'email';

function fingerprint(card: ClientCard): string {
  return JSON.stringify(card.subscriptions.map((s) => [s.id, s.tariff_id, s.mirror.expire_at, s.ended_at, s.period_days]));
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function ClientPage() {
  const phone = usePhone();
  return phone ? <ClientPhone /> : <ClientDesktop />;
}

function ClientDesktop() {
  const { id } = useParams();
  const clientId = Number(id);
  const utm = useClientUtm(clientId);
  const { can, me } = useSession();
  const navigate = useNavigate();
  const [params, setParams] = useSearchParams();
  const writable = can('clients_write');
  const loginOk = can('clients_login');
  const moneyOk = can('money');
  const refundsOk = can('refunds');
  const [refundFor, setRefundFor] = useState<number | null>(null);
  const [card, setCard] = useState<ClientCard | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [subId, setSubId] = useState<number | null>(null);
  const [details, setDetails] = useState<SubscriptionDetails | null>(null);
  const [history, setHistory] = useState<HistoryPage | null>(null);
  const [historyAll, setHistoryAll] = useState(false);
  const [historyBusy, setHistoryBusy] = useState(false);
  const [dialog, setDialog] = useState<Dialogs>('');
  const [notice, setNotice] = useState<string | null>(null);
  const [note, setNote] = useState('');
  const [noteState, setNoteState] = useState<'' | 'saving' | 'saved' | 'error'>('');
  const [dangerBusy, setDangerBusy] = useState(false);
  const [detailsKey, setDetailsKey] = useState(0);
  const extendRef = useRef<((days: number) => Promise<void>) | null>(null);

  const loadCard = useCallback(async () => {
    try {
      const data = await api<ClientCard>(`/api/admin/clients/${clientId}`);
      setCard(data);
      setNote(data.client.note ?? '');
      setError(null);
      return data;
    } catch (err) {
      setError(failText(err, 'Карточка не загрузилась'));
      return null;
    }
  }, [clientId]);

  const loadHistory = useCallback(
    async (more = false) => {
      setHistoryBusy(true);
      try {
        const limit = more ? MORE_PAGE : FIRST_PAGE;
        const after = more && history?.next ? `&after=${history.next}` : '';
        const page = await api<HistoryPage>(`/api/admin/clients/${clientId}/history?limit=${limit}${after}`);
        setHistory((prev) => (more && prev ? { items: [...prev.items, ...page.items], next: page.next, total: page.total } : page));
      } catch {
        setHistory((prev) => prev ?? { items: [], next: null, total: 0 });
      } finally {
        setHistoryBusy(false);
      }
    },
    [clientId, history?.next],
  );

  const refreshHistory = useCallback(() => {
    void (async () => {
      try {
        const page = await api<HistoryPage>(`/api/admin/clients/${clientId}/history?limit=${FIRST_PAGE}`);
        setHistory(page);
        setHistoryAll(false);
      } catch {
        return;
      }
    })();
  }, [clientId]);

  useEffect(() => {
    setCard(null);
    setDetails(null);
    setHistory(null);
    setHistoryAll(false);
    setSubId(null);
    void loadCard();
    refreshHistory();
  }, [loadCard, refreshHistory]);

  const live = useMemo(() => (card ? liveSubscriptions(card) : []), [card]);
  const tabs = useMemo(() => [...live].sort((a, b) => a.id - b.id), [live]);
  const sub: Subscription | null = live.find((s) => s.id === subId) ?? live[0] ?? null;

  useEffect(() => {
    if (!sub) {
      setDetails(null);
      return;
    }
    let cancelled = false;
    setDetails(null);
    api<SubscriptionDetails>(`/api/admin/subscriptions/${sub.id}`)
      .then((data) => {
        if (!cancelled) {
          setDetails(data);
          setCard((prev) =>
            prev ? { ...prev, subscriptions: prev.subscriptions.map((s) => (s.id === data.subscription.id ? data.subscription : s)) } : prev,
          );
        }
      })
      .catch(() => {
        if (!cancelled) {
          setDetails(null);
        }
      });
    return () => {
      cancelled = true;
    };
  }, [sub?.id, detailsKey]);

  useEffect(() => {
    if (params.get('grant') === '1' && card && writable) {
      setDialog(liveSubscriptions(card).length > 0 ? 'change' : 'grant');
      const next = new URLSearchParams(params);
      next.delete('grant');
      setParams(next, { replace: true });
    }
  }, [card, params, setParams, writable]);

  const waitForGrant = useCallback(
    async (before: string) => {
      setNotice('Выдаём подписку…');
      for (let attempt = 0; attempt < 12; attempt += 1) {
        await new Promise((resolve) => window.setTimeout(resolve, 1500));
        const data = await loadCard();
        if (data && fingerprint(data) !== before) {
          setNotice('Готово: подписка обновлена');
          setDetailsKey((k) => k + 1);
          refreshHistory();
          return;
        }
      }
      setNotice('Заказ на выдачу создан: подписка появится в течение пары минут, обновите страницу позже');
      refreshHistory();
    },
    [loadCard, refreshHistory],
  );

  const onView = useCallback((view: { subscription: Subscription; subscription_url?: string | null } & Partial<SubscriptionDetails>) => {
    setCard((prev) =>
      prev ? { ...prev, subscriptions: prev.subscriptions.map((s) => (s.id === view.subscription.id ? view.subscription : s)) } : prev,
    );
    setDetails((prev) => {
      if (!prev) {
        return prev;
      }
      const next = { ...prev, subscription: view.subscription };
      if (view.subscription_url !== undefined) {
        next.subscription_url = view.subscription_url;
      }
      if (view.status) {
        next.status = view.status;
      }
      if (view.progress !== undefined) {
        next.progress = view.progress;
      }
      if (view.renewal !== undefined) {
        next.renewal = view.renewal;
      }
      return next;
    });
  }, []);

  const onExtendHeader = useCallback((runner: (days: number) => Promise<void>) => {
    extendRef.current = runner;
  }, []);

  const saveNote = async () => {
    if (!card || note === (card.client.note ?? '')) {
      return;
    }
    setNoteState('saving');
    try {
      await api(`/api/admin/clients/${clientId}/note`, { method: 'PUT', body: { note: note.trim() ? note.trim() : null } });
      setCard({ ...card, client: { ...card.client, note: note.trim() || null } });
      setNoteState('saved');
      refreshHistory();
    } catch {
      setNoteState('error');
    }
  };

  const unblock = async () => {
    setDangerBusy(true);
    try {
      await api(`/api/admin/clients/${clientId}/unblock`, { method: 'POST' });
      setNotice('Клиент разблокирован, подписки снова работают');
      await loadCard();
      refreshHistory();
    } catch (err) {
      setNotice(failText(err, 'Разблокировать не получилось'));
    } finally {
      setDangerBusy(false);
    }
  };

  if (error && !card) {
    return (
      <div className="cc">
        <Link to="/admin/clients" className="cc-back">
          <Icon name="chevron-left" size={16} stroke={2} />
          Все клиенты
        </Link>
        <p role="alert" className="err">
          {error}
        </p>
      </div>
    );
  }
  if (!card) {
    return (
      <div className="cc">
        <Link to="/admin/clients" className="cc-back">
          <Icon name="chevron-left" size={16} stroke={2} />
          Все клиенты
        </Link>
        <p className="muted">Загружаем карточку…</p>
      </div>
    );
  }

  const name = personName(card);
  const tg = telegramOf(card);
  const email = emailOf(card);
  const blocked = card.client.status === 'blocked';
  const tariff = tariffOf(card, sub);
  const notifies = card.bot_available && !!tg && !card.client.bot_unreachable;

  const mirror = sub?.mirror;
  const expired = !!sub && (mirror?.status === 'expired' || (!!mirror?.expire_at && Date.parse(mirror.expire_at) <= Date.now()));
  const reset = sub ? nextReset(sub, tariff) : '';
  const leftDays = mirror?.expire_at ? daysLeft(mirror.expire_at) : null;
  const [statusText, statusColor] = blocked
    ? ['Заблокирован', 'var(--bad)']
    : !sub
      ? ['Нет подписки', 'var(--muted)']
      : mirror?.status === 'limited'
        ? [`Трафик исчерпан${reset ? `, сброс ${reset}` : ''}`, 'var(--warn)']
        : expired
          ? [mirror?.expire_at ? `Подписка истекла ${dateLong(mirror.expire_at)}` : 'Подписка истекла', 'var(--muted)']
          : mirror?.status === 'disabled'
            ? ['Подписка отключена', 'var(--bad)']
            : sub.kind === 'trial'
              ? ['Пробный период', 'var(--ink-2)']
              : leftDays !== null && leftDays <= 3
                ? [`Подписка истекает через ${days(Math.max(leftDays, 1))}`, 'var(--warn)']
                : ['Подписка активна', 'var(--ink-2)'];

  const seen = sub
    ? mirror?.online_at
      ? `в сети ${seenText(mirror.online_at)}${details?.last_node ? ` через ${details.last_node}` : ''}`
      : 'подключений ещё не было'
    : '';
  const subline = [
    tg?.meta?.username ? `@${tg.meta.username}` : null,
    tg ? `Telegram ${tg.value}` : email ? email.value : null,
    sinceText(card.client.created_at),
    seen || null,
  ]
    .filter(Boolean)
    .join(' · ');

  const messageBlock = !card.bot_available
    ? 'Бот не подключён: сообщение некуда отправить. Подключите бота в настройках.'
    : !tg
      ? 'У клиента нет Telegram: написать можно только тем, кто пришёл через бота.'
      : card.client.bot_unreachable
        ? 'Клиент остановил бота, сообщение не дойдёт.'
        : null;

  const balance: Money | undefined = card.balance;
  const referralBalance = card.referral_balance;
  const onRemnaAdded = (text: string, added: number) => {
    setNotice(text);
    setSubId(added);
    void loadCard();
    refreshHistory();
  };
  const lines = (history?.items ?? []).map((item, index) => historyLine(item, index, card.stats.referrer));
  const stats = card.stats;
  const locale = card.client.locale === 'en' ? 'English' : 'Русский';

  return (
    <div className="cc">
      <Link to="/admin/clients" className="cc-back">
        <Icon name="chevron-left" size={16} stroke={2} />
        Все клиенты
      </Link>

      <header className="cc-head">
        <Avatar id={card.client.id} name={name} className="cc-ava" retry />
        <div className="cc-who">
          <div className="cc-titlerow">
            <h1 className="cc-name">{name}</h1>
            <span style={{ fontWeight: 600, color: statusColor }}>{statusText}</span>
          </div>
          <span className="muted">{subline}</span>
        </div>
        <div className="cc-btns" style={{ gap: 8 }}>
          {writable && sub && !sub.ended_at ? (
            <button type="button" className="btn-main btn" onClick={() => setDialog('extend')}>
              Продлить
            </button>
          ) : null}
          {writable && !sub && !blocked ? (
            <button type="button" className="btn-main btn" onClick={() => setDialog('grant')}>
              Выдать подписку
            </button>
          ) : null}
          {writable ? (
            <button type="button" className="b44 hov" onClick={() => setDialog('message')}>
              Написать
            </button>
          ) : null}
          {moneyOk && balance ? (
            <button type="button" className="b44 hov" onClick={() => setDialog('balance-add')}>
              Пополнить баланс
            </button>
          ) : null}
        </div>
      </header>

      {notice ? (
        <div role="status" className="cc-note">
          <span className="cc-note-text wide">{notice}</span>
          <button type="button" className="b34 on-surface" onClick={() => setNotice(null)}>
            Скрыть
          </button>
        </div>
      ) : null}

      <div className="cc-cols">
        <div className="cc-main">
          {tabs.length > 1 ? (
            <div role="group" aria-label="Подписки клиента" className="segs" style={{ alignSelf: 'flex-start' }}>
              {tabs.map((s, index) => (
                <button key={s.id} type="button" aria-pressed={s.id === sub?.id} onClick={() => setSubId(s.id)}>
                  Подписка {index + 1}
                </button>
              ))}
            </div>
          ) : null}
          {sub ? (
            <SubscriptionBlock
              sub={sub}
              details={details}
              tariff={tariff}
              clientBlocked={blocked}
              writable={writable}
              notifies={notifies}
              onView={onView}
              onHistory={refreshHistory}
              onExtendHeader={onExtendHeader}
              onChangeTariff={() => setDialog('change')}
              onLinked={(text) => {
                setNotice(text);
                void loadCard();
                setDetailsKey((k) => k + 1);
                refreshHistory();
              }}
            />
          ) : (
            <section className="cc-sec cc-subsec" aria-labelledby="sub-h">
              <div className="cc-col">
                <span className="cc-cap">Подписка</span>
                <h2 id="sub-h" className="cc-plan">
                  Подписки нет
                </h2>
              </div>
              <p className="muted">
                {blocked ? 'Клиент заблокирован: чтобы выдать подписку, сначала разблокируйте его.' : 'Клиент ещё не покупал подписку или она завершена.'}
              </p>
              {writable && !blocked ? (
                <div className="cc-btns">
                  <button type="button" className="b38 hov" onClick={() => setDialog('grant')}>
                    Выдать подписку
                  </button>
                  <RemnaAddButton clientId={clientId} onAdded={onRemnaAdded} />
                </div>
              ) : null}
            </section>
          )}
          {details && !details.remna_reachable ? (
            <p className="cap" role="status">
              Remnawave сейчас не отвечает: показаны сохранённые данные, действия с подпиской могут не пройти.
            </p>
          ) : null}
          {writable && !blocked && sub && card.multi_subscriptions ? <RemnaAddButton more clientId={clientId} onAdded={onRemnaAdded} /> : null}

          <section aria-labelledby="his-h" className="cc-sec cc-list">
            <h2 id="his-h" className="cc-h2" style={{ marginBottom: 8 }}>
              История
            </h2>
            {history === null ? <p className="cc-empty muted">Загружаем историю…</p> : null}
            {history && lines.length === 0 ? <p className="cc-empty muted">Событий пока нет</p> : null}
            {lines.map((line) => (
              <div key={line.key} className="cc-his">
                <span className="cc-small muted">{line.when}</span>
                <span className="cc-col" style={{ minWidth: 0 }}>
                  <span style={{ fontWeight: 600 }}>{line.what}</span>
                  {line.note ? <span className="cc-small muted">{line.note}</span> : null}
                  {refundsOk && line.refund ? (
                    <button type="button" className="cc-his-act" onClick={() => setRefundFor(line.refund ?? null)}>
                      Вернуть деньги
                    </button>
                  ) : null}
                </span>
                <span className="num" style={{ fontWeight: 700, whiteSpace: 'nowrap' }}>
                  {line.sum}
                </span>
              </div>
            ))}
            {history && !historyAll && history.total > history.items.length ? (
              <button
                type="button"
                className="cc-more"
                onClick={() => {
                  setHistoryAll(true);
                  void loadHistory(true);
                }}
                disabled={historyBusy}
              >
                Вся история, {eventsCount(history.total)}
              </button>
            ) : null}
            {history && historyAll && history.next ? (
              <button type="button" className="cc-more" onClick={() => void loadHistory(true)} disabled={historyBusy}>
                {historyBusy ? 'Загружаем…' : `Показать ещё, всего ${eventsCount(history.total)}`}
              </button>
            ) : null}
          </section>
        </div>

        <aside className="cc-aside">
          {moneyOk && balance ? (
            <section aria-labelledby="money-h" className="cc-sec cc-box" style={{ gap: 14 }}>
              <h2 id="money-h" className="cc-h2">
                Деньги
              </h2>
              <div className="cc-money">
                <span className="cc-col">
                  <span className="cc-small muted">Баланс</span>
                  <span className="cc-balance num">{money(balance)}</span>
                </span>
                <button type="button" className="b36 hov" onClick={() => setDialog('balance-edit')}>
                  Изменить
                </button>
              </div>
              <div className="cc-col" style={{ gap: 6, color: 'var(--ink-2)' }}>
                {stats.paid_total ? (
                  <span>
                    Всего оплачено: <strong style={{ color: 'var(--ink)' }}>{money(stats.paid_total)}</strong>
                    {stats.payments_count ? ` за ${count(stats.payments_count, 'платёж', 'платежа', 'платежей')}` : ''}
                  </span>
                ) : null}
                {referralBalance && referralBalance.minor > 0 ? <span>Реферальный баланс: {money(referralBalance)}</span> : null}
              </div>
            </section>
          ) : null}

          <section aria-labelledby="prof-h" className="cc-sec cc-box">
            <h2 id="prof-h" className="cc-h2" style={{ marginBottom: 6 }}>
              Профиль
            </h2>
            <div className="cc-prof">
              <span className="muted">Telegram</span>
              <span>{tg ? (tg.meta?.username ? `@${tg.meta.username}` : tg.value) : 'нет'}</span>
            </div>
            <EmailRow email={email} editable={loginOk} onEdit={() => setDialog('email')} />
            {oauthLogins(card).map((login) => (
              <div className="cc-prof" key={login.title}>
                <span className="muted">{login.title}</span>
                <span>{login.account}</span>
              </div>
            ))}
            <div className="cc-prof">
              <span className="muted">Язык</span>
              <span>{locale}</span>
            </div>
            <div className="cc-prof">
              <span className="muted">Приглашённые</span>
              <span>{stats.referrals_count > 0 ? count(stats.referrals_count, 'друг', 'друга', 'друзей') : 'никого'}</span>
            </div>
            {stats.referrer ? (
              <div className="cc-prof">
                <span className="muted">Пригласивший</span>
                <Link to={`/admin/clients/${stats.referrer.client_id}`}>
                  {stats.referrer.telegram_username ? `@${stats.referrer.telegram_username}` : (stats.referrer.name ?? `клиент ${stats.referrer.client_id}`)}
                </Link>
              </div>
            ) : null}
            <div className="cc-prof">
              <span className="muted">Регистрация</span>
              <span>{registrationText(card.client.registration_source, card.client.created_at)}</span>
            </div>
            {utm ? (
              <div className="cc-prof">
                <span className="muted">Метка</span>
                <UtmValue found={utm} />
              </div>
            ) : null}
            <div className="cc-prof">
              <span className="muted">Вход в кабинет</span>
              <span>{visitText(stats.cabinet_seen_at)}</span>
            </div>
            {stats.miniapp_seen_at ? (
              <div className="cc-prof">
                <span className="muted">Мини-приложение</span>
                <span>{visitText(stats.miniapp_seen_at)}</span>
              </div>
            ) : null}
          </section>

          <ClientProtect clientId={clientId} onChanged={() => setDetailsKey((k) => k + 1)} />

          <section aria-labelledby="note-h" className="cc-sec cc-box" style={{ gap: 8 }}>
            <label id="note-h" htmlFor="client-note" className="cc-h2">
              Заметка
            </label>
            <textarea
              id="client-note"
              rows={3}
              className="cc-notefield"
              placeholder="Видят только администраторы"
              value={note}
              maxLength={2000}
              readOnly={!writable}
              onChange={(event) => {
                setNote(event.target.value);
                setNoteState('');
              }}
              onBlur={() => void saveNote()}
            />
            {noteState ? (
              <span role="status" className={noteState === 'error' ? 'err' : 'cap'}>
                {noteState === 'saving' ? 'Сохраняем…' : noteState === 'saved' ? 'Заметка сохранена' : 'Заметка не сохранилась, попробуйте ещё раз'}
              </span>
            ) : null}
          </section>

          {writable ? (
            <details className="cc-sec cc-danger">
              <summary>Опасные действия</summary>
              <div className="cc-btns" style={{ gap: 8, paddingTop: 14 }}>
                {blocked ? (
                  <button type="button" className="b36 hov" onClick={() => void unblock()} disabled={dangerBusy}>
                    {dangerBusy ? 'Разблокируем…' : 'Разблокировать'}
                  </button>
                ) : (
                  <button type="button" className="b36 bad hov" onClick={() => setDialog('block')}>
                    Заблокировать
                  </button>
                )}
                {loginOk ? (
                  <button type="button" className="b36 bad hov" onClick={() => setDialog('reset-2fa')} disabled={!card.two_factor.enabled}>
                    Сбросить 2FA
                  </button>
                ) : null}
                {loginOk && emailOf(card) ? (
                  <button type="button" className="b36 bad hov" onClick={() => setDialog('password')}>
                    Новый пароль для сайта
                  </button>
                ) : null}
                {moneyOk && sub ? (
                  <button type="button" className="b36 bad hov" onClick={() => setDialog('delete-sub')}>
                    Удалить подписку
                  </button>
                ) : null}
                {moneyOk ? (
                  <button type="button" className="b36 bad hov" onClick={() => setDialog('delete-client')}>
                    Удалить клиента
                  </button>
                ) : null}
              </div>
              <p className="cap" style={{ paddingTop: 10 }}>
                {card.two_factor.enabled ? `2FA включена${card.two_factor.enabled_at ? ` ${dateLong(card.two_factor.enabled_at)}` : ''}` : '2FA у клиента не включена'}
              </p>
              {blocked && card.client.block_reason ? <p className="cap" style={{ paddingTop: 4 }}>Причина блокировки: {card.client.block_reason}</p> : null}
            </details>
          ) : null}
        </aside>
      </div>

      {refundFor !== null ? (
        <RefundDialog
          paymentId={refundFor}
          onClose={() => setRefundFor(null)}
          onDone={(_, text) => {
            setRefundFor(null);
            setNotice(text);
            void loadCard();
            setDetailsKey((k) => k + 1);
            refreshHistory();
          }}
        />
      ) : null}
      {dialog === 'message' ? (
        <MessageDialog
          clientId={clientId}
          blockedReason={messageBlock}
          onClose={() => setDialog('')}
          onSent={() => {
            setDialog('');
            setNotice('Сообщение поставлено в очередь, бот отправит его в течение минуты');
            refreshHistory();
          }}
        />
      ) : null}
      {(dialog === 'balance-add' || dialog === 'balance-edit') && balance ? (
        <BalanceDialog
          clientId={clientId}
          currency={me.currency}
          balance={balance}
          initial="add"
          onClose={() => setDialog('')}
          onDone={(next) => {
            setDialog('');
            setCard((prev) => (prev ? { ...prev, balance: next } : prev));
            setNotice(`Баланс изменён, теперь ${money(next)}`);
            refreshHistory();
          }}
        />
      ) : null}
      {dialog === 'extend' && sub ? (
        <ExtendDialog until={sub.mirror.expire_at} onExtend={(n) => extendRef.current?.(n) ?? Promise.resolve()} onClose={() => setDialog('')} />
      ) : null}
      {dialog === 'grant' || dialog === 'change' ? (
        <GrantDialog
          clientId={clientId}
          subscriptionId={dialog === 'change' && sub ? sub.id : null}
          currentTariffId={dialog === 'change' && sub ? sub.tariff_id : null}
          currentTrial={dialog === 'change' && sub?.kind === 'trial'}
          held={live.map((s) => ({ id: s.id, tariff_id: s.tariff_id }))}
          onClose={() => setDialog('')}
          onDone={() => {
            const before = fingerprint(card);
            setDialog('');
            void waitForGrant(before);
          }}
        />
      ) : null}
      {dialog === 'delete-sub' && sub ? (
        <ConfirmDialog
          title="Удалить подписку"
          text="Пользователь удалится в Remnawave, ссылка и все устройства перестанут работать. Вернуть подписку будет нельзя, только выдать новую. Оплаты останутся в истории."
          confirm="Удалить подписку"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/subscriptions/${sub.id}/actions/delete`, { method: 'POST', body: {} });
            setDialog('');
            setNotice('Подписка удалена');
            await loadCard();
            refreshHistory();
          }}
        />
      ) : null}
      {dialog === 'password' && emailOf(card) ? (
        <PasswordDialog clientId={clientId} email={emailOf(card)?.value ?? ''} onClose={() => setDialog('')} onIssued={refreshHistory} />
      ) : null}
      {dialog === 'email' ? (
        <EmailDialog
          clientId={clientId}
          current={email}
          onClose={() => setDialog('')}
          onDone={(text) => {
            setDialog('');
            setNotice(text);
            void loadCard();
            refreshHistory();
          }}
        />
      ) : null}
      {dialog === 'reset-2fa' ? (
        <ConfirmDialog
          title="Сбросить 2FA"
          text="Клиент войдёт в кабинет на сайте без кода из приложения и сможет заново включить защиту в разделе «Безопасность». Все входы в кабинет на сайте закроются, мини-приложение в Telegram продолжит работать."
          confirm="Сбросить 2FA"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/clients/${clientId}/totp/reset`, { method: 'POST', body: {} });
            setDialog('');
            setNotice('2FA сброшена: при следующем входе на сайт код не понадобится');
            await loadCard();
            refreshHistory();
          }}
        />
      ) : null}
      {dialog === 'delete-client' ? (
        <ConfirmDialog
          title="Удалить клиента"
          text="Подписки клиента удалятся в Remnawave, вход в кабинет закроется, профиль исчезнет из списка. Если человек снова напишет боту, он придёт как новый клиент. Оплаты останутся в отчётах."
          confirm="Удалить клиента"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/clients/${clientId}/delete`, { method: 'POST', body: {} });
            navigate('/admin/clients', { replace: true });
          }}
        />
      ) : null}
      {dialog === 'block' ? (
        <BlockDialog
          clientId={clientId}
          onClose={() => setDialog('')}
          onDone={() => {
            setDialog('');
            setNotice('Клиент заблокирован');
            void loadCard();
            refreshHistory();
          }}
        />
      ) : null}
    </div>
  );
}

