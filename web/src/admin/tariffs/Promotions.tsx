import { useId, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { NBSP } from '../../lib/format';
import { Dialog } from '../client/dialogs';
import { Switch } from '../ui';
import type { Tariff } from './model';

export type PromotionState = 'off' | 'scheduled' | 'running' | 'finished';
export type Promotion = {
  id: number;
  name: string;
  percent: number;
  tariff_ids: number[];
  starts_at: string;
  ends_at: string;
  enabled: boolean;
  state: PromotionState;
};
export type PromotionsView = { promotions: Promotion[]; offset_minutes: number; now: string };

type Draft = { id: number | null; name: string; percent: string; tariffs: number[]; startDate: string; startTime: string; endDate: string; endTime: string; enabled: boolean };

const NAME_MAX = 40;
const MONTHS = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];

function two(n: number): string {
  return String(n).padStart(2, '0');
}

export function zoneLabel(minutes: number): string {
  if (minutes === 0) {
    return 'UTC';
  }
  const abs = Math.abs(minutes);
  const tail = abs % 60 ? `:${two(abs % 60)}` : '';
  return `UTC${minutes < 0 ? '−' : '+'}${Math.floor(abs / 60)}${tail}`;
}

function panelDate(ms: number, minutes: number): Date {
  return new Date(ms + minutes * 60_000);
}

function parts(iso: string, minutes: number): { date: string; time: string } {
  const at = panelDate(Date.parse(iso), minutes);
  return {
    date: `${at.getUTCFullYear()}-${two(at.getUTCMonth() + 1)}-${two(at.getUTCDate())}`,
    time: `${two(at.getUTCHours())}:${two(at.getUTCMinutes())}`,
  };
}

function fromPanel(date: string, time: string, minutes: number): string | null {
  const d = /^(\d{4})-(\d{2})-(\d{2})$/.exec(date);
  const t = /^(\d{1,2}):(\d{2})$/.exec(time);
  if (!d || !t) {
    return null;
  }
  const ms = Date.UTC(Number(d[1]), Number(d[2]) - 1, Number(d[3]), Number(t[1]), Number(t[2])) - minutes * 60_000;
  return Number.isNaN(ms) ? null : new Date(ms).toISOString();
}

function stamp(iso: string, minutes: number, nowIso: string): string {
  const at = panelDate(Date.parse(iso), minutes);
  const now = panelDate(Date.parse(nowIso), minutes);
  const year = at.getUTCFullYear() === now.getUTCFullYear() ? '' : ` ${at.getUTCFullYear()}`;
  return `${at.getUTCDate()}${NBSP}${MONTHS[at.getUTCMonth()] ?? ''}${year}, ${two(at.getUTCHours())}:${two(at.getUTCMinutes())}`;
}

function stateText(p: Promotion, minutes: number, now: string): string {
  switch (p.state) {
    case 'running':
      return 'идёт';
    case 'scheduled':
      return `начнётся ${stamp(p.starts_at, minutes, now)}`;
    case 'off':
      return 'выключена';
    default:
      return 'закончилась';
  }
}

export function promotionsSummary(view: PromotionsView): string {
  const running = view.promotions.filter((p) => p.state === 'running');
  const [only] = running;
  if (only && running.length === 1) {
    return `идёт −${only.percent}%`;
  }
  if (running.length > 1) {
    return `идут: ${running.length}`;
  }
  return view.promotions.some((p) => p.state === 'scheduled') ? 'запланирована' : 'нет';
}

function blank(view: PromotionsView): Draft {
  const now = Date.parse(view.now);
  const start = parts(new Date(Math.ceil(now / 3_600_000) * 3_600_000).toISOString(), view.offset_minutes);
  const end = parts(new Date(now + 3 * 86_400_000).toISOString(), view.offset_minutes);
  return { id: null, name: '', percent: '', tariffs: [], startDate: start.date, startTime: start.time, endDate: end.date, endTime: '23:59', enabled: true };
}

function draftOf(p: Promotion, minutes: number): Draft {
  const start = parts(p.starts_at, minutes);
  const end = parts(p.ends_at, minutes);
  return { id: p.id, name: p.name, percent: String(p.percent), tariffs: p.tariff_ids, startDate: start.date, startTime: start.time, endDate: end.date, endTime: end.time, enabled: p.enabled };
}

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function PromotionsDialog({ view, tariffs, onClose, onChanged }: { view: PromotionsView; tariffs: Tariff[]; onClose: () => void; onChanged: () => Promise<void> }) {
  const [draft, setDraft] = useState<Draft | null>(null);
  const [removing, setRemoving] = useState<number | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ids = useId();
  const minutes = view.offset_minutes;
  const zone = zoneLabel(minutes);
  const nameOf = (id: number) => tariffs.find((t) => t.id === id)?.name ?? `№${id}`;
  const patch = (change: Partial<Draft>) => setDraft((d) => (d ? { ...d, ...change } : d));

  const run = async (work: () => Promise<unknown>, fallback: string) => {
    setBusy(true);
    setError(null);
    try {
      await work();
      await onChanged();
      return true;
    } catch (err) {
      setError(failText(err, fallback));
      return false;
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!draft) {
      return;
    }
    const name = draft.name.trim();
    if (!name) {
      setError('Укажите название: его увидят клиенты');
      return;
    }
    const percent = Number(draft.percent);
    if (!Number.isInteger(percent) || percent < 1 || percent > 90) {
      setError('Скидка — от 1 до 90 %');
      return;
    }
    if (draft.tariffs.length === 0) {
      setError('Выберите хотя бы один тариф');
      return;
    }
    const starts = fromPanel(draft.startDate, draft.startTime, minutes);
    const ends = fromPanel(draft.endDate, draft.endTime, minutes);
    if (!starts || !ends) {
      setError('Укажите дату и время начала и конца');
      return;
    }
    if (Date.parse(ends) <= Date.parse(starts)) {
      setError('Конец акции должен быть позже начала');
      return;
    }
    const body = { name, percent, tariff_ids: draft.tariffs, starts_at: starts, ends_at: ends, enabled: draft.enabled };
    const done = await run(
      () =>
        draft.id === null
          ? api('/api/admin/tariffs/promotions', { method: 'POST', body })
          : api(`/api/admin/tariffs/promotions/${draft.id}`, { method: 'PUT', body }),
      'Акция не сохранилась',
    );
    if (done) {
      setDraft(null);
    }
  };

  const toggle = (p: Promotion) => void run(() => api(`/api/admin/tariffs/promotions/${p.id}`, { method: 'PATCH', body: { enabled: !p.enabled } }), 'Акция не переключилась');

  const remove = async (id: number) => {
    if (await run(() => api(`/api/admin/tariffs/promotions/${id}`, { method: 'DELETE' }), 'Акция не удалилась')) {
      setRemoving(null);
    }
  };

  if (draft) {
    return (
      <Dialog title={draft.id === null ? 'Новая акция' : 'Акция'} busy={busy} onClose={onClose} wide>
        <label className="tf-field" htmlFor={`${ids}-n`}>
          <span>Название для клиентов</span>
          <input id={`${ids}-n`} className="field" value={draft.name} maxLength={NAME_MAX} placeholder="Например, Чёрная пятница" onChange={(e) => patch({ name: e.target.value })} disabled={busy} />
        </label>
        <label className="tf-field" htmlFor={`${ids}-p`}>
          <span>Скидка</span>
          <span className="tf-inline">
            <input id={`${ids}-p`} className="field num" inputMode="numeric" value={draft.percent} placeholder="25" onChange={(e) => patch({ percent: e.target.value.replace(/\D/g, '').slice(0, 2) })} style={{ width: 88 }} disabled={busy} />
            <span className="muted">% от цены каждого срока</span>
          </span>
        </label>
        <div className="field-row">
          <span style={{ fontWeight: 600 }}>Тарифы</span>
          <div role="group" aria-label="Тарифы акции" className="chips">
            {tariffs.map((t) => {
              const on = draft.tariffs.includes(t.id);
              return (
                <button key={t.id} type="button" className="chip" aria-pressed={on} onClick={() => patch({ tariffs: on ? draft.tariffs.filter((x) => x !== t.id) : [...draft.tariffs, t.id] })} disabled={busy}>
                  <span>{t.name}</span>
                </button>
              );
            })}
            {tariffs.length === 0 ? <span className="muted">Тарифов пока нет</span> : null}
          </div>
        </div>
        <div className="tf-wrap">
          <fieldset className="tf-fieldset tf-when">
            <legend>Начало</legend>
            <span className="tf-inline">
              <input type="date" aria-label="Дата начала" className="field" value={draft.startDate} onChange={(e) => patch({ startDate: e.target.value })} disabled={busy} />
              <input type="time" aria-label="Время начала" className="field" value={draft.startTime} onChange={(e) => patch({ startTime: e.target.value })} disabled={busy} />
            </span>
          </fieldset>
          <fieldset className="tf-fieldset tf-when">
            <legend>Конец</legend>
            <span className="tf-inline">
              <input type="date" aria-label="Дата конца" className="field" value={draft.endDate} onChange={(e) => patch({ endDate: e.target.value })} disabled={busy} />
              <input type="time" aria-label="Время конца" className="field" value={draft.endTime} onChange={(e) => patch({ endTime: e.target.value })} disabled={busy} />
            </span>
          </fieldset>
        </div>
        <span className="muted tf-promo-note">Время — по поясу панели, {zone}. Пояс задаётся в «Настройки → Общие».</span>
        <span className="tf-switchrow">
          <Switch on={draft.enabled} onToggle={() => patch({ enabled: !draft.enabled })} label="Акция включена" disabled={busy} />
          <span aria-hidden="true">{draft.enabled ? 'Включена' : 'Выключена'}</span>
        </span>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <div className="dialog-actions">
          <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
          <button
            type="button"
            className="btn-line hov"
            onClick={() => {
              setDraft(null);
              setError(null);
            }}
            disabled={busy}
          >
            К списку
          </button>
        </div>
      </Dialog>
    );
  }

  return (
    <Dialog title="Акции" busy={busy} onClose={onClose} wide>
      <span className="muted tf-promo-note">
        Скидка в процентах от цены каждого срока выбранных тарифов: в рублях цена округляется вниз до рубля, в долларах — до цента, в звёздах вверх до звезды. Доп. устройства и пакеты трафика — без скидки. Промокод со скидкой с акцией не складывается: клиент получает большую из двух скидок. Tribute и автопродление Lava.top списывают свою цену, скидка к ним не применяется.
      </span>
      {view.promotions.length === 0 ? <p className="muted">Акций пока нет. Без акций цены не меняются.</p> : null}
      {view.promotions.length > 0 ? (
        <ul className="tf-promos">
          {view.promotions.map((p) => (
            <li key={p.id} className="tf-promo">
              <div className="tf-promo-main">
                <span className="tf-promo-name">
                  <b>{p.name}</b> <span>−{p.percent}%</span> <span className={p.state === 'running' ? 'tf-promo-on' : 'muted'}>{stateText(p, minutes, view.now)}</span>
                </span>
                <span className="muted">{p.tariff_ids.map(nameOf).join(', ')}</span>
                <span className="muted">
                  {stamp(p.starts_at, minutes, view.now)} — {stamp(p.ends_at, minutes, view.now)} ({zone})
                </span>
              </div>
              {removing === p.id ? (
                <div className="tf-promo-actions">
                  <span>Удалить акцию? Клиенты сразу увидят полные цены.</span>
                  <button type="button" className="tf-link" onClick={() => void remove(p.id)} disabled={busy}>
                    Удалить
                  </button>
                  <button type="button" className="tf-link" onClick={() => setRemoving(null)} disabled={busy}>
                    Отменить
                  </button>
                </div>
              ) : (
                <div className="tf-promo-actions">
                  {p.state !== 'finished' ? <Switch on={p.enabled} onToggle={() => toggle(p)} label={`Акция «${p.name}» включена`} disabled={busy} /> : null}
                  {p.state !== 'finished' ? (
                    <button
                      type="button"
                      className="tf-link"
                      onClick={() => {
                        setDraft(draftOf(p, minutes));
                        setError(null);
                      }}
                      disabled={busy}
                    >
                      Изменить
                    </button>
                  ) : null}
                  <button type="button" className="tf-link" onClick={() => setRemoving(p.id)} disabled={busy}>
                    Удалить
                  </button>
                </div>
              )}
            </li>
          ))}
        </ul>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button
          type="button"
          className="btn-main btn"
          onClick={() => {
            setDraft(blank(view));
            setError(null);
          }}
          disabled={busy}
        >
          Новая акция
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Закрыть
        </button>
      </div>
    </Dialog>
  );
}
