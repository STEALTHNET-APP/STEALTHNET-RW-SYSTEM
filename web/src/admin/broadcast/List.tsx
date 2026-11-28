import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { plural } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { BASE, channelLabel, failText, outcome, percent, ROOT, statusLine, type ListItem, type Options } from './model';
import { BroadcastTabs } from './Tabs';
import '../home/phone.css';

type PageData = { items: ListItem[]; next: number | null };

function meta(item: ListItem): string {
  const parts = [item.audience_label, channelLabel(item.channel)];
  if (item.media === 'video') {
    parts.push('с видео');
  } else if (item.has_image) {
    parts.push('с картинкой');
  }
  if (item.buttons > 0) {
    parts.push(`${item.buttons} ${plural(item.buttons, 'кнопка', 'кнопки', 'кнопок')}`);
  }
  return parts.join(' · ');
}

function busy(items: ListItem[] | null): boolean {
  const soon = Date.now() + 120_000;
  return (items ?? []).some(
    (item) => item.status === 'sending' || (item.status === 'scheduled' && item.send_at !== null && Date.parse(item.send_at) < soon),
  );
}

export function useList() {
  const [items, setItems] = useState<ListItem[] | null>(null);
  const [next, setNext] = useState<number | null>(null);
  const [options, setOptions] = useState<Options | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [more, setMore] = useState(false);

  const load = useCallback(async () => {
    try {
      const [page, opts] = await Promise.all([api<PageData>(`${BASE}/broadcasts?limit=30`), api<Options>(`${BASE}/options`)]);
      setItems((current) => {
        if (!current || current.length <= page.items.length) {
          return page.items;
        }
        const fresh = new Map(page.items.map((item) => [item.id, item]));
        return current.map((item) => fresh.get(item.id) ?? item);
      });
      setNext((current) => current ?? page.next);
      setOptions(opts);
      setError(null);
    } catch (err) {
      setItems((current) => current ?? []);
      setError(failText(err, 'Рассылки не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const live = busy(items);
  useEffect(() => {
    if (!live) {
      return;
    }
    const timer = window.setInterval(() => void load(), 3000);
    return () => window.clearInterval(timer);
  }, [live, load]);

  const loadMore = async () => {
    if (next === null) {
      return;
    }
    setMore(true);
    try {
      const page = await api<PageData>(`${BASE}/broadcasts?limit=30&after=${next}`);
      setItems((list) => [...(list ?? []), ...page.items]);
      setNext(page.next);
    } catch (err) {
      setError(failText(err, 'Рассылки не загрузились'));
    } finally {
      setMore(false);
    }
  };

  return { items, next, options, error, more, loadMore };
}

export function BotWarning({ options }: { options: Options | null }) {
  if (!options || options.bot) {
    return null;
  }
  return (
    <p className="warn-line bc-warn">
      Бот не подключён: рассылки не уйдут, пока не задан токен. <Link to="/admin/settings/bot">Настройки бота</Link>
    </p>
  );
}

export function List() {
  const phone = usePhone();
  const data = useList();
  return phone ? <ListPhone {...data} /> : <ListDesktop {...data} />;
}

type Data = ReturnType<typeof useList>;

function ListDesktop({ items, next, options, error, more, loadMore }: Data) {
  return (
    <>
      <div className="bc-top">
        <h1 className="page-title tight">Рассылки</h1>
        <Link to={`${ROOT}/new`} className="btn-main btn">
          Новая рассылка
        </Link>
      </div>
      <BroadcastTabs />
      <BotWarning options={options} />
      <section className="card bc-list" aria-label="Все рассылки">
        {error ? (
          <p className="err bc-pad" role="alert">
            {error}
          </p>
        ) : null}
        {items === null ? <p className="muted bc-pad">Загружаем рассылки…</p> : null}
        {items !== null && items.length === 0 && !error ? (
          <div className="bc-empty">
            <p>Рассылок пока нет. Соберите сообщение, выберите, кому его отправить, проверьте на одном клиенте и отправьте сразу или ко времени.</p>
            <Link to={`${ROOT}/new`} className="btn-main btn">
              Новая рассылка
            </Link>
          </div>
        ) : null}
        {items?.map((item) => {
          const status = statusLine(item);
          return (
            <Link key={item.id} to={`${ROOT}/${item.id}`} className="bc-row hov">
              <span className="bc-main">
                <span className="bc-line">{item.preview || `Рассылка №${item.id}`}</span>
                <span className="bc-meta">{meta(item)}</span>
              </span>
              <span className="bc-state">
                <span className={`bc-status ${status.tone}`}>{status.text}</span>
                {item.status === 'sending' && item.progress.total > 0 ? (
                  <span className="bc-bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent(item.progress)}>
                    <span style={{ width: `${percent(item.progress)}%` }} />
                  </span>
                ) : null}
                {(item.status === 'done' || item.status === 'canceled') && item.progress.total > 0 ? (
                  <span className="bc-meta">{outcome(item.progress, item.channel)}</span>
                ) : null}
                {item.notice && item.status === 'sending' ? <span className="bc-meta">{item.notice}</span> : null}
                {item.mail_notice && item.status === 'sending' ? <span className="bc-meta">{item.mail_notice}</span> : null}
              </span>
            </Link>
          );
        })}
        {next !== null ? (
          <div className="bc-pad">
            <button type="button" className="btn-line hov" onClick={() => void loadMore()} disabled={more}>
              {more ? 'Загружаем…' : 'Показать ещё'}
            </button>
          </div>
        ) : null}
      </section>
    </>
  );
}

function ListPhone({ items, next, options, error, more, loadMore }: Data) {
  return (
    <div className="hp bc-phone">
      <PhoneHeader title="Рассылки">
        <Link to={`${ROOT}/new`} className="ph-circ press" aria-label="Новая рассылка">
          <span className="ph-dot">
            <Icon name="plus" size={20} />
          </span>
        </Link>
      </PhoneHeader>
      <BroadcastTabs />
      <BotWarning options={options} />
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {items === null ? <p className="hp-quiet">Загружаем рассылки…</p> : null}
      {items !== null && items.length === 0 && !error ? (
        <section className="hp-group">
          <div className="hp-card bc-empty">
            <p>Рассылок пока нет. Соберите сообщение, выберите, кому отправить, и проверьте на одном клиенте.</p>
            <Link to={`${ROOT}/new`} className="btn-main btn">
              Новая рассылка
            </Link>
          </div>
        </section>
      ) : null}
      {items && items.length > 0 ? (
        <section className="hp-group" aria-label="Все рассылки">
          <div className="hp-card hp-list">
            {items.map((item) => {
              const status = statusLine(item);
              return (
                <Link key={item.id} to={`${ROOT}/${item.id}`} className="hp-todo bc-prow">
                  <span className="hp-text">
                    <span className="hp-title">{item.preview || `Рассылка №${item.id}`}</span>
                    <span className={`hp-note ${status.tone === 'warn' ? 'warn' : status.tone === 'muted' ? 'muted' : ''}`}>{status.text}</span>
                    {(item.status === 'done' || item.status === 'canceled') && item.progress.total > 0 ? (
                      <span className="hp-note muted">{outcome(item.progress, item.channel)}</span>
                    ) : (
                      <span className="hp-note muted">
                        {item.audience_label} · {channelLabel(item.channel)}
                      </span>
                    )}
                  </span>
                  <Icon name="chevron-right" size={18} />
                </Link>
              );
            })}
          </div>
          {next !== null ? (
            <button type="button" className="btn-line hov" onClick={() => void loadMore()} disabled={more}>
              {more ? 'Загружаем…' : 'Показать ещё'}
            </button>
          ) : null}
        </section>
      ) : null}
    </div>
  );
}
