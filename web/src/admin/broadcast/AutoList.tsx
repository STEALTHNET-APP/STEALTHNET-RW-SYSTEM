import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api } from '../../lib/api';
import { Icon } from '../../lib/icons';
import { usePhone } from '../../lib/media';
import { PhoneHeader } from '../phone';
import { Switch } from '../ui';
import { BotWarning } from './List';
import { BroadcastTabs } from './Tabs';
import { AUTO_ROOT, BASE, at, channelLabel, clients, count, failText, type AutoReach, type Options, type RuleItem } from './model';
import '../form.css';
import '../home/phone.css';

function meta(item: RuleItem): string {
  const parts = [item.timing, channelLabel(item.channel)];
  if (item.with_autorenew) {
    parts.push('и с автопродлением');
  }
  if (item.audience_label) {
    parts.push(`кому: ${item.audience_label.charAt(0).toLowerCase()}${item.audience_label.slice(1)}`);
  }
  return parts.join(' · ');
}

function state(item: RuleItem): { text: string; tone: 'ink' | 'muted' } {
  if (!item.enabled) {
    return { text: 'Выключено', tone: 'muted' };
  }
  if (item.stats.cases === 0) {
    return { text: 'Ещё не срабатывало', tone: 'muted' };
  }
  return { text: `Отправлено ${count(item.stats.progress.sent)}`, tone: 'ink' };
}

function details(item: RuleItem): string {
  const parts: string[] = [];
  if (!item.enabled && item.stats.progress.sent > 0) {
    parts.push(`отправлено ${count(item.stats.progress.sent)}`);
  }
  if (item.stats.progress.pending > 0) {
    parts.push(`в очереди ${count(item.stats.progress.pending)}`);
  }
  if (item.stats.last_sent_at) {
    parts.push(`последнее ${at(item.stats.last_sent_at)}`);
  }
  return parts.join(' · ');
}

function useRules() {
  const [items, setItems] = useState<RuleItem[] | null>(null);
  const [options, setOptions] = useState<Options | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | null>(null);

  const load = useCallback(async () => {
    try {
      const [page, opts] = await Promise.all([api<{ items: RuleItem[] }>(`${BASE}/auto`), api<Options>(`${BASE}/options`)]);
      setItems(page.items);
      setOptions(opts);
      setError(null);
    } catch (err) {
      setItems((current) => current ?? []);
      setError(failText(err, 'Правила не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const live = (items ?? []).some((item) => item.stats.progress.pending > 0);
  useEffect(() => {
    if (!live) {
      return;
    }
    const timer = window.setInterval(() => void load(), 4000);
    return () => window.clearInterval(timer);
  }, [live, load]);

  const presetsOff = (items ?? []).filter((item) => item.preset !== null && !item.enabled);

  const presetsWaiting = async (): Promise<number> => {
    const counts = await Promise.all(
      presetsOff.map((item) =>
        api<AutoReach>(`${BASE}/auto/reach`, {
          method: 'POST',
          body: { trigger: item.trigger, amount: item.amount, unit: item.unit, percent: item.percent, with_autorenew: item.with_autorenew, id: item.id },
        })
          .then((reach) => reach.recipients)
          .catch(() => 0),
      ),
    );
    return counts.reduce((sum, value) => sum + value, 0);
  };

  const enablePresets = async () => {
    setBusy(-1);
    setError(null);
    try {
      await api(`${BASE}/auto/presets/enable`, { method: 'POST' });
      await load();
    } catch (err) {
      setError(failText(err, 'Не получилось включить стандартные правила'));
    } finally {
      setBusy(null);
    }
  };

  const toggle = async (item: RuleItem) => {
    setBusy(item.id);
    setError(null);
    try {
      await api(`${BASE}/auto/${item.id}/switch`, { method: 'POST', body: { enabled: !item.enabled } });
      await load();
    } catch (err) {
      setError(failText(err, 'Не получилось переключить правило'));
    } finally {
      setBusy(null);
    }
  };

  return { items, options, error, busy, toggle, presetsOff, presetsWaiting, enablePresets };
}

type Data = ReturnType<typeof useRules>;

export function AutoList() {
  const phone = usePhone();
  const data = useRules();
  return phone ? <AutoPhone {...data} /> : <AutoDesktop {...data} />;
}

function Lead({ options, compact }: { options: Options | null; compact?: boolean }) {
  const every = options?.auto.every_minutes ?? 5;
  if (compact) {
    return (
      <div className="bc-lead">
        <p>Правило само пишет клиенту, когда наступает событие: каждому один раз на случай.</p>
        <p className="muted">
          Об оплате, продлении, автопродлении и ответах поддержки бот пишет и без правил. Их тексты — в <Link to="/admin/settings/bot/texts">настройках бота</Link>.
        </p>
      </div>
    );
  }
  return (
    <div className="bc-lead">
      <p>
        Правило само пишет клиенту, когда наступает событие. Каждый получает сообщение один раз на случай: на одну подписку, на одно окончание.
        Проверка раз в {every}&nbsp;минут, после включения правило подхватит и события за последние сутки.
      </p>
      <p className="muted">
        Об оплате, выдаче и продлении подписки, автопродлении и ответах поддержки бот пишет и без правил — это системные уведомления. Их тексты — в{' '}
        <Link to="/admin/settings/bot/texts">настройках бота</Link>.
      </p>
    </div>
  );
}

function Presets({ presetsOff, presetsWaiting, enablePresets, busy }: Data) {
  const [asking, setAsking] = useState<number | null>(null);
  if (presetsOff.length === 0) {
    return null;
  }
  const n = presetsOff.length;
  const ask = async () => {
    setAsking(-1);
    setAsking(await presetsWaiting());
  };
  return (
    <div className="bc-presets" role="group" aria-label="Стандартные правила">
      {asking === null ? (
        <>
          <p>
            {n === 1 ? 'Стандартное правило добавлено выключенным' : 'Стандартные правила добавлены выключенными'}, чтобы клиентам ничего не пришло неожиданно. Проверьте тексты и
            включите — переключателем у правила или все сразу.
          </p>
          <span>
            <button type="button" className="btn-main btn" onClick={() => void ask()} disabled={busy !== null}>
              Включить стандартные правила
            </button>
          </span>
        </>
      ) : (
        <>
          <p>
            {asking < 0
              ? 'Считаем, кому сообщения уйдут сразу…'
              : asking > 0
                ? `После включения в ближайшие минуты сообщения уйдут ${clients(asking)} — у них событие наступило за последние сутки. Дальше — каждому, у кого оно наступит.`
                : 'Сейчас под условия никто не попадает: сообщения начнут уходить, когда у клиентов наступят события.'}
          </p>
          <span className="bc-ask-btns">
            <button
              type="button"
              className="btn-main btn"
              onClick={() => {
                setAsking(null);
                void enablePresets();
              }}
              disabled={busy !== null || asking < 0}
            >
              {busy === -1 ? 'Включаем…' : `Включить ${count(n)}`}
            </button>
            <button type="button" className="btn-line hov" onClick={() => setAsking(null)} disabled={busy !== null}>
              Не включать
            </button>
          </span>
        </>
      )}
    </div>
  );
}

const EMPTY = 'Правил пока нет. Например: напомнить о продлении за 3 дня до конца подписки или написать тем, кто попробовал пробный период и ничего не купил.';

function AutoDesktop(data: Data) {
  const { items, options, error, busy, toggle } = data;
  return (
    <>
      <div className="bc-top">
        <h1 className="page-title tight">Рассылки</h1>
        <Link to={`${AUTO_ROOT}/new`} className="btn-main btn">
          Новое правило
        </Link>
      </div>
      <BroadcastTabs />
      <BotWarning options={options} />
      <Lead options={options} />
      <Presets {...data} />
      <section className="card bc-list" aria-label="Правила">
        {error ? (
          <p className="err bc-pad" role="alert">
            {error}
          </p>
        ) : null}
        {items === null ? <p className="muted bc-pad">Загружаем правила…</p> : null}
        {items !== null && items.length === 0 && !error ? (
          <div className="bc-empty">
            <p>{EMPTY}</p>
            <Link to={`${AUTO_ROOT}/new`} className="btn-main btn">
              Новое правило
            </Link>
          </div>
        ) : null}
        {items?.map((item) => {
          const line = state(item);
          const more = details(item);
          return (
            <div key={item.id} className="bc-rule">
              <Link to={`${AUTO_ROOT}/${item.id}`} className="bc-row hov">
                <span className="bc-main">
                  <span className="bc-line">{item.title}</span>
                  <span className="bc-meta">{meta(item)}</span>
                  {item.preview ? <span className="bc-meta bc-snippet">{item.preview}</span> : null}
                </span>
                <span className="bc-state">
                  <span className={`bc-status ${line.tone}`}>{line.text}</span>
                  {more ? <span className="bc-meta">{more}</span> : null}
                  {item.notice ? <span className="bc-meta warn">{item.notice}</span> : null}
                </span>
              </Link>
              <Switch
                on={item.enabled}
                onToggle={() => void toggle(item)}
                label={`${item.title}, ${item.timing}: ${item.enabled ? 'включено' : 'выключено'}`}
                disabled={busy !== null}
              />
            </div>
          );
        })}
      </section>
    </>
  );
}

function AutoPhone(data: Data) {
  const { items, options, error, busy, toggle } = data;
  return (
    <div className="hp bc-phone">
      <PhoneHeader title="Рассылки">
        <Link to={`${AUTO_ROOT}/new`} className="ph-circ press" aria-label="Новое правило">
          <span className="ph-dot">
            <Icon name="plus" size={20} />
          </span>
        </Link>
      </PhoneHeader>
      <BroadcastTabs />
      <BotWarning options={options} />
      <Lead options={options} compact />
      <Presets {...data} />
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      {items === null ? <p className="hp-quiet">Загружаем правила…</p> : null}
      {items !== null && items.length === 0 && !error ? (
        <section className="hp-group">
          <div className="hp-card bc-empty">
            <p>{EMPTY}</p>
            <Link to={`${AUTO_ROOT}/new`} className="btn-main btn">
              Новое правило
            </Link>
          </div>
        </section>
      ) : null}
      {items && items.length > 0 ? (
        <section className="hp-group" aria-label="Правила">
          <div className="hp-card hp-list">
            {items.map((item) => {
              const line = state(item);
              return (
                <div key={item.id} className="hp-todo bc-prule">
                  <Link to={`${AUTO_ROOT}/${item.id}`} className="hp-text bc-prow">
                    <span className="hp-title">{item.title}</span>
                    <span className="hp-note muted">{meta(item)}</span>
                    <span className={`hp-note ${line.tone === 'muted' ? 'muted' : ''}`}>{line.text}</span>
                    {item.notice ? <span className="hp-note warn">{item.notice}</span> : null}
                  </Link>
                  <Switch
                    on={item.enabled}
                    onToggle={() => void toggle(item)}
                    label={`${item.title}, ${item.timing}: ${item.enabled ? 'включено' : 'выключено'}`}
                    disabled={busy !== null}
                  />
                </div>
              );
            })}
          </div>
        </section>
      ) : null}
    </div>
  );
}
