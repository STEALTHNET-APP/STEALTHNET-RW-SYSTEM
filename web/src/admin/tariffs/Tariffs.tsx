import { useCallback, useEffect, useId, useMemo, useState } from 'react';
import { api, ApiError } from '../../lib/api';
import { count, groupDigits, NBSP, plural } from '../../lib/format';
import { Icon } from '../../lib/icons';
import { ConfirmDialog, Dialog } from '../client/dialogs';
import { DeleteTariff } from './DeleteTariff';
import { useSession } from '../session';
import { Switch } from '../ui';
import {
  blankDraft,
  devicesText,
  draftOf,
  fromRaw,
  GIB,
  inputOf,
  limitText,
  nextKey,
  perMonthNote,
  priceOf,
  priceText,
  RESETS,
  salesWord,
  termText,
  trafficText,
  type Applied,
  type Category,
  type Channels,
  type Draft,
  type DraftDiscount,
  type DraftPack,
  type Leftover,
  type Squad,
  type Tariff,
  type TrafficReset,
  type View,
} from './model';
import '../form.css';
import './tariffs.css';

type TrialVariant = { id: number; name: string; enabled: boolean; days: number; devices: number; squads: string[]; traffic: { limit_bytes: number | null; reset: TrafficReset } };
type Trial = { enabled: boolean; variants: TrialVariant[] };
type VariantDraft = { key: string; id: number; name: string; enabled: boolean; days: string; devices: string; gb: string; squads: string[] };

const TRIAL_VARIANTS_MAX = 10;
const TRIAL_NAME_MAX = 40;

function offeredVariants(trial: Trial): TrialVariant[] {
  return trial.enabled ? trial.variants.filter((v) => v.enabled && v.squads.length > 0) : [];
}

function trialSummary(trial: Trial): string {
  const offered = offeredVariants(trial);
  const [only] = offered;
  if (!only) {
    return 'выключен';
  }
  if (offered.length === 1) {
    return `${only.days}${NBSP}${plural(only.days, 'день', 'дня', 'дней')}`;
  }
  return count(offered.length, 'вариант', 'варианта', 'вариантов');
}
type Multi = { enabled: boolean };

const DEFAULT_CHANNELS: Channels = { bot: true, cabinet: true, miniapp: true };

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function Tariffs() {
  const { can, me } = useSession();
  const settingsOk = can('settings');
  const remnaOff = me.remnawave === false;
  const [view, setView] = useState<View | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [squads, setSquads] = useState<Squad[] | null>(null);
  const [category, setCategory] = useState<number | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [saved, setSaved] = useState<{ id: number; text: string } | null>(null);
  const [busy, setBusy] = useState(false);
  const [formError, setFormError] = useState<string | null>(null);
  const [hwidOff, setHwidOff] = useState(false);
  const [trial, setTrial] = useState<Trial | null>(null);
  const [multi, setMulti] = useState<Multi | null>(null);
  const [dialog, setDialog] = useState<'' | 'category' | 'category-edit' | 'trial' | 'multi' | 'delete'>('');

  const load = useCallback(async () => {
    try {
      const data = await api<View>('/api/admin/tariffs');
      setView(data);
      setError(null);
      setCategory((current) => (current && data.categories.some((c) => c.id === current) ? current : (data.categories[0]?.id ?? null)));
      return data;
    } catch (err) {
      setError(failText(err, 'Тарифы не загрузились'));
      return null;
    }
  }, []);

  useEffect(() => {
    void load();
    if (remnaOff) {
      setSquads([]);
      setHwidOff(false);
    } else {
      api<Squad[]>('/api/admin/tariffs/squads')
        .then(setSquads)
        .catch(() => setSquads([]));
    }
    if (settingsOk) {
      if (!remnaOff) {
        api<{ hwidSettings?: { enabled: boolean } | null }>('/api/admin/settings/subscription')
          .then((sub) => setHwidOff(sub.hwidSettings ? !sub.hwidSettings.enabled : false))
          .catch(() => setHwidOff(false));
      }
      api<Trial>('/api/admin/settings/trial')
        .then(setTrial)
        .catch(() => setTrial(null));
      api<Multi>('/api/admin/settings/multi_subscriptions')
        .then(setMulti)
        .catch(() => setMulti(null));
    }
  }, [load, settingsOk, remnaOff]);

  const currency = view?.currency ?? 'USD';
  const stats = useMemo(() => new Map((view?.stats ?? []).map((s) => [s.tariff_id, s])), [view]);
  const squadName = useCallback(
    (uuid: string) => squads?.find((s) => s.uuid === uuid)?.name ?? `${uuid.slice(0, 8)}…`,
    [squads],
  );
  const counts = useMemo(() => {
    const map = new Map<number, number>();
    for (const t of view?.tariffs ?? []) {
      map.set(t.category_id, (map.get(t.category_id) ?? 0) + 1);
    }
    return map;
  }, [view]);
  const shown = (view?.tariffs ?? []).filter((t) => t.category_id === category);
  const picked = view?.categories.find((c) => c.id === category) ?? null;

  const pickCategory = (id: number) => {
    setCategory(id);
    setDraft(null);
    setSaved(null);
    setFormError(null);
  };

  const open = (tariff: Tariff) => {
    if (draft?.id === tariff.id) {
      setDraft(null);
      setFormError(null);
      return;
    }
    setSaved(null);
    setFormError(null);
    setDraft(draftOf(tariff, stats.get(tariff.id)?.channels ?? DEFAULT_CHANNELS, currency));
  };

  const startNew = () => {
    if (!view || category === null) {
      setDialog('category');
      return;
    }
    setSaved(null);
    setFormError(null);
    setDraft(blankDraft(category, squads ?? [], currency));
  };

  const toggleSale = async (tariff: Tariff) => {
    try {
      await api(`/api/admin/tariffs/${tariff.id}`, { method: 'PATCH', body: { on_sale: !tariff.on_sale } });
      await load();
    } catch (err) {
      setError(failText(err, 'Не получилось изменить продажу'));
    }
  };

  const save = async () => {
    if (!draft) {
      return;
    }
    const { body, error: invalid } = inputOf(draft, currency);
    if (invalid) {
      setFormError(invalid);
      return;
    }
    setBusy(true);
    setFormError(null);
    try {
      let id = draft.id;
      let applied: Applied = null;
      if (id === null) {
        const created = await api<Tariff>('/api/admin/tariffs', { method: 'POST', body });
        id = created.id;
      } else {
        const updated = await api<Tariff & { applied: Applied }>(`/api/admin/tariffs/${id}${draft.apply ? '?apply=true' : ''}`, {
          method: 'PUT',
          body,
        });
        applied = updated.applied;
      }
      const before = stats.get(id)?.channels ?? DEFAULT_CHANNELS;
      const c = draft.channels;
      if (c.bot !== before.bot || c.cabinet !== before.cabinet || c.miniapp !== before.miniapp) {
        await api(`/api/admin/tariffs/${id}/channels`, { method: 'PUT', body: c });
      }
      let text = 'Сохранено.';
      if (applied && (applied.squads || applied.traffic || applied.devices)) {
        if (applied.users === 0) {
          text = 'Сохранено. Текущих подписчиков на тарифе нет';
        } else if (applied.failed > 0) {
          text = `Сохранено, но у ${groupDigits(applied.failed)} из ${groupDigits(applied.users)} подписок Remnawave не принял изменения. Повторите сохранение позже`;
        } else {
          text = `Сохранено. Изменения для текущих подписчиков отправлены в Remnawave: ${count(applied.users, 'подписка', 'подписки', 'подписок')}`;
        }
      }
      setDraft(null);
      setSaved({ id, text });
      await load();
    } catch (err) {
      setFormError(failText(err, 'Тариф не сохранился'));
    } finally {
      setBusy(false);
    }
  };

  const edit = (patch: Partial<Draft>) => setDraft((d) => (d ? { ...d, ...patch } : d));

  if (error && !view) {
    return (
      <div className="tf">
        <h1 className="page-title">Тарифы</h1>
        <p className="err" role="alert">
          {error}
        </p>
      </div>
    );
  }

  const subtitle = `Что видят клиенты в боте, кабинете и мини-приложении. ${currency === 'RUB' ? 'Цены в рублях' : 'Цены в долларах'}, валюта выбирается в настройках.`;
  const isNew = draft !== null && draft.id === null;

  return (
    <div className="tf">
      <header className="tf-head">
        <div className="tf-title">
          <h1 className="page-title">Тарифы</h1>
          <span className="muted">{subtitle}</span>
        </div>
        {settingsOk && multi ? (
          <button type="button" className="b44 hov tf-trial" onClick={() => setDialog('multi')}>
            Подписок у клиента <span className="muted">{multi.enabled ? 'несколько' : 'одна'}</span>
          </button>
        ) : null}
        {settingsOk && trial ? (
          <button type="button" className="b44 hov tf-trial" onClick={() => setDialog('trial')}>
            Пробный период <span className="muted">{trialSummary(trial)}</span>
          </button>
        ) : null}
        <button type="button" className="btn-main btn" onClick={startNew} style={{ padding: '0 18px' }}>
          Новый тариф
        </button>
      </header>

      <div role="group" aria-label="Категория" className="chips">
        {(view?.categories ?? []).map((c) => (
          <button key={c.id} type="button" className="chip" aria-pressed={c.id === category} onClick={() => pickCategory(c.id)}>
            <span>{c.name}</span>
            <span className="chip-n">{counts.get(c.id) ?? 0}</span>
          </button>
        ))}
        <button type="button" className="chip dashed hov" onClick={() => setDialog('category')}>
          Новая категория
        </button>
      </div>

      {picked ? (
        <div className="tf-cat">
          <span>
            <span className="muted">Подписок этой категории у клиента:</span> <strong>{limitText(picked.subscription_limit)}</strong>
            {picked.subscription_limit && multi && !multi.enabled ? <span className="muted"> · не действует, пока у клиента одна подписка</span> : null}
          </span>
          <button type="button" className="b38 hov" onClick={() => setDialog('category-edit')}>
            Изменить категорию
          </button>
        </div>
      ) : null}

      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}

      {isNew && draft ? (
        <section aria-label="Новый тариф" className="tf-sec">
          <div className="tf-row">
            <div className="tf-info">
              <h2 className="tf-name">Новый тариф</h2>
              <span className="muted">Заполните поля и сохраните — тариф появится в выбранной категории</span>
            </div>
          </div>
          <TariffForm
            draft={draft}
            currency={currency}
            squads={squads}
            hwidOff={hwidOff}
            busy={busy}
            error={formError}
            onEdit={edit}
            onSave={() => void save()}
            onCancel={() => {
              setDraft(null);
              setFormError(null);
            }}
            onDelete={null}
          />
        </section>
      ) : null}

      {view && category !== null && shown.length === 0 && !isNew ? (
        <section className="tf-sec tf-empty">
          <p className="muted">В этой категории пока нет тарифов.</p>
          <div className="cc-btns" style={{ gap: 8 }}>
            <button type="button" className="b38 hov" onClick={startNew}>
              Создать тариф
            </button>
            <button type="button" className="b38 hov" style={{ color: 'var(--bad)' }} onClick={() => setDialog('delete')}>
              Удалить категорию
            </button>
          </div>
        </section>
      ) : null}

      {view && view.categories.length === 0 ? (
        <section className="tf-sec tf-empty">
          <p className="muted">Тарифов ещё нет. Начните с категории: например, «Стандарт» или «Белые списки».</p>
          <div className="cc-btns">
            <button type="button" className="b38 hov" onClick={() => setDialog('category')}>
              Создать категорию
            </button>
          </div>
        </section>
      ) : null}

      {shown.map((tariff) => {
        const st = stats.get(tariff.id);
        const sales = st?.sales_30d ?? 0;
        const opened = draft?.id === tariff.id;
        const packs = tariff.traffic.limit_bytes ? (tariff.traffic_addon?.packs ?? []) : [];
        const meta = [
          devicesText(tariff.devices, tariff.device_discounts ?? []),
          trafficText(tariff.traffic),
          packs.length ? `докупка трафика по ${packs.map((p) => p.gb).join(', ')} ГБ` : null,
          `сквады: ${tariff.squads.map(squadName).join(', ')}`,
        ]
          .filter(Boolean)
          .join(' · ');
        return (
          <section key={tariff.id} aria-label={tariff.name} className="tf-sec">
            <div className="tf-row">
              <div className="tf-info">
                <div className="tf-namerow">
                  <h2 className="tf-name">{tariff.name}</h2>
                  {tariff.description ? <span className="muted">{tariff.description}</span> : null}
                </div>
                <div className="tf-prices">
                  {tariff.periods.map((p) => (
                    <span key={p.id} className="tf-price">
                      <span className="muted">{termText(p.days)}</span>
                      <strong className="num">{priceText(priceOf(p.prices, currency), currency)}</strong>
                    </span>
                  ))}
                </div>
                <span style={{ color: 'var(--ink-2)' }}>{meta}</span>
              </div>
              <div className="tf-side">
                <span className="tf-sales">
                  <strong className="num">{groupDigits(sales)}</strong>
                  <span>{salesWord(sales)}</span>
                </span>
                <span className="tf-sale">
                  <Switch on={tariff.on_sale} onToggle={() => void toggleSale(tariff)} label={`В продаже: ${tariff.name}`} />
                  <span style={{ color: tariff.on_sale ? 'var(--ink-2)' : 'var(--muted)' }}>{tariff.on_sale ? 'В продаже' : 'Скрыт'}</span>
                </span>
                <button type="button" className="b40 hov" aria-expanded={opened} onClick={() => open(tariff)}>
                  {opened ? 'Свернуть' : 'Изменить'}
                </button>
              </div>
            </div>
            {saved?.id === tariff.id && !opened ? (
              <div style={{ padding: '0 20px 18px' }}>
                <p role="status" className="tf-saved">
                  <strong>{saved.text.split(' ')[0]}</strong> {saved.text.split(' ').slice(1).join(' ')}
                </p>
              </div>
            ) : null}
            {opened && draft ? (
              <TariffForm
                draft={draft}
                currency={currency}
                squads={squads}
                hwidOff={hwidOff}
                busy={busy}
                error={formError}
                onEdit={edit}
                onSave={() => void save()}
                onCancel={() => {
                  setDraft(null);
                  setFormError(null);
                }}
                onDelete={() => setDialog('delete')}
              />
            ) : null}
          </section>
        );
      })}

      {dialog === 'category' || (dialog === 'category-edit' && picked) ? (
        <CategoryDialog
          category={dialog === 'category-edit' ? picked : null}
          multi={multi}
          onMulti={settingsOk && multi ? () => setDialog('multi') : null}
          onClose={() => setDialog('')}
          onDone={async (id) => {
            setDialog('');
            await load();
            pickCategory(id);
          }}
        />
      ) : null}
      {dialog === 'multi' && multi ? (
        <MultiDialog
          value={multi}
          onClose={() => setDialog('')}
          onDone={(next) => {
            setMulti(next);
            setDialog('');
          }}
        />
      ) : null}
      {dialog === 'trial' && trial ? (
        <TrialDialog
          trial={trial}
          squads={squads ?? []}
          onClose={() => setDialog('')}
          onDone={(next) => {
            setTrial(next);
            setDialog('');
          }}
        />
      ) : null}
      {dialog === 'delete' && draft?.id ? (
        <DeleteTariff
          tariffId={draft.id}
          onSale={(view?.tariffs ?? []).find((t) => t.id === draft.id)?.on_sale ?? draft.on_sale}
          onClose={() => setDialog('')}
          onDeleted={async () => {
            setDialog('');
            setDraft(null);
            await load();
          }}
          onHidden={async () => {
            setDialog('');
            setDraft(null);
            await load();
          }}
        />
      ) : null}
      {dialog === 'delete' && !draft && category !== null ? (
        <ConfirmDialog
          title="Удалить категорию"
          text="Пустая категория исчезнет из списка и из каталога для клиентов."
          confirm="Удалить категорию"
          onClose={() => setDialog('')}
          run={async () => {
            await api(`/api/admin/categories/${category}`, { method: 'DELETE' });
            setDialog('');
            setCategory(null);
            await load();
          }}
        />
      ) : null}
    </div>
  );
}

type FormProps = {
  draft: Draft;
  currency: 'USD' | 'RUB';
  squads: Squad[] | null;
  hwidOff: boolean;
  busy: boolean;
  error: string | null;
  onEdit: (patch: Partial<Draft>) => void;
  onSave: () => void;
  onCancel: () => void;
  onDelete: (() => void) | null;
};

function TariffForm({ draft, currency, squads, hwidOff, busy, error, onEdit, onSave, onCancel, onDelete }: FormProps) {
  const ids = useId();
  const sym = currency === 'RUB' ? '₽' : '$';
  const base = draft.periods.find((p) => Number(p.days) === 30);
  const baseMinor = base ? fromRaw(base.price, currency) : null;
  const resetHint = draft.gb.trim() ? (RESETS.find((r) => r.value === draft.reset)?.hint ?? '') : '';
  const extraMax = Number(draft.extraMax);
  const extraWord = Number.isInteger(extraMax) && extraMax % 10 === 1 && extraMax % 100 !== 11 ? 'устройства' : 'устройств';
  const knownSquads = squads ?? [];
  const unknown = draft.squads.filter((s) => !knownSquads.some((k) => k.uuid === s));

  const setPeriod = (key: string, patch: Partial<Draft['periods'][number]>) =>
    onEdit({ periods: draft.periods.map((p) => (p.key === key ? { ...p, ...patch } : p)) });
  const setPack = (key: string, patch: Partial<DraftPack>) => onEdit({ packs: draft.packs.map((p) => (p.key === key ? { ...p, ...patch } : p)) });
  const setDiscount = (key: string, patch: Partial<DraftDiscount>) => onEdit({ discounts: draft.discounts.map((d) => (d.key === key ? { ...d, ...patch } : d)) });
  const periodic = draft.reset === 'day' || draft.reset === 'week' || draft.reset === 'month' || draft.reset === 'month_rolling';
  const leftovers: Array<[Leftover, string]> = [
    ['burn', 'Сгорает'],
    ['carry', 'Переносится'],
  ];
  const leftoverHint =
    draft.leftover === 'carry'
      ? periodic
        ? 'Что клиент не израсходовал из докупленного, остаётся и после сброса трафика.'
        : 'Что клиент не израсходовал из докупленного, добавится к лимиту после продления.'
      : periodic
        ? 'Докупленный трафик действует до ближайшего сброса трафика.'
        : 'Докупленный трафик действует до продления подписки.';
  const toggleSquad = (uuid: string) => {
    const on = draft.squads.includes(uuid);
    if (on && draft.squads.length === 1) {
      return;
    }
    onEdit({ squads: on ? draft.squads.filter((s) => s !== uuid) : [...draft.squads, uuid] });
  };
  const toggleChannel = (key: keyof Channels) => {
    const next = { ...draft.channels, [key]: !draft.channels[key] };
    if (!next.bot && !next.cabinet && !next.miniapp) {
      return;
    }
    onEdit({ channels: next });
  };

  return (
    <div className="tf-form">
      <div className="tf-grid">
        <label className="tf-field">
          <span>Название</span>
          <input className="field" type="text" value={draft.name} maxLength={100} onChange={(e) => onEdit({ name: e.target.value })} disabled={busy} />
        </label>
        <label className="tf-field">
          <span>Описание для клиента</span>
          <input className="field" type="text" value={draft.description} maxLength={2000} onChange={(e) => onEdit({ description: e.target.value })} disabled={busy} />
        </label>
      </div>

      <div className="tf-block" style={{ gap: 8 }}>
        <span className="tf-bt">Цены</span>
        {draft.periods.map((p) => {
          const days = Number(p.days);
          const minor = fromRaw(p.price, currency);
          const note =
            Number.isInteger(days) && typeof minor === 'number' && typeof baseMinor === 'number' && p.key !== base?.key
              ? perMonthNote(days, minor, baseMinor, currency)
              : Number.isInteger(days) && typeof minor === 'number' && p.key !== base?.key
                ? perMonthNote(days, minor, null, currency)
                : '';
          const term = Number.isInteger(days) && days > 0 ? termText(days) : 'срок';
          return (
            <div key={p.key} className="tf-period">
              <label className="tf-inline">
                <span className="sr-only">Срок в днях</span>
                <input className="field num" inputMode="numeric" value={p.days} onChange={(e) => setPeriod(p.key, { days: e.target.value.replace(/\D/g, '').slice(0, 4) })} style={{ width: 88 }} disabled={busy} />
                <span className="muted" style={{ minWidth: 36 }}>
                  {plural(Number.isInteger(days) ? days : 0, 'день', 'дня', 'дней')}
                </span>
              </label>
              <label className="tf-inline">
                <span className="sr-only">Цена</span>
                {currency === 'USD' ? <span className="muted">{sym}</span> : null}
                <input className="field num" inputMode="decimal" value={p.price} onChange={(e) => setPeriod(p.key, { price: e.target.value })} style={{ width: 104 }} disabled={busy} />
                {currency === 'RUB' ? <span className="muted">{sym}</span> : null}
              </label>
              <label className="tf-inline">
                <span className="sr-only">Цена в звёздах Telegram</span>
                <input className="field num" inputMode="numeric" value={p.stars} onChange={(e) => setPeriod(p.key, { stars: e.target.value.replace(/\D/g, '').slice(0, 7) })} style={{ width: 96 }} disabled={busy} />
                <span className="muted" style={{ minWidth: 48 }}>
                  {plural(Number(p.stars) || 0, 'звезда', 'звезды', 'звёзд')}
                </span>
              </label>
              <span className="muted tf-note">{note}</span>
              {draft.periods.length > 1 ? (
                <button type="button" className="tf-x hov" aria-label={`Убрать срок ${term}`} onClick={() => onEdit({ periods: draft.periods.filter((x) => x.key !== p.key) })} disabled={busy}>
                  <Icon name="close" size={16} stroke={2} />
                </button>
              ) : null}
            </div>
          );
        })}
        <button
          type="button"
          className="tf-add"
          onClick={() =>
            onEdit({
              periods: [...draft.periods, { key: nextKey(), id: null, days: '', price: '', stars: '', other: null, sort: draft.periods.length, active: true }],
            })
          }
          disabled={busy || draft.periods.length >= 8}
        >
          Добавить срок
        </button>
      </div>

      <div className="tf-block">
        <span className="tf-bt">Устройства</span>
        <div className="tf-wrap" style={{ alignItems: 'flex-end' }}>
          <label className="tf-field">
            <span>В тарифе</span>
            <input className="field num" inputMode="numeric" value={draft.included} onChange={(e) => onEdit({ included: e.target.value.replace(/\D/g, '').slice(0, 3) })} style={{ width: 104 }} disabled={busy} />
          </label>
          <span className="tf-switchrow">
            <Switch on={draft.extraOn} onToggle={() => onEdit({ extraOn: !draft.extraOn })} label="Продавать доп. устройства" />
            <span style={{ fontWeight: 600 }} aria-hidden="true">
              Продавать доп. устройства
            </span>
          </span>
          {draft.extraOn ? (
            <>
              <label className="tf-field">
                <span>Цена за устройство в месяц</span>
                <span className="tf-inline">
                  {currency === 'USD' ? <span className="muted">{sym}</span> : null}
                  <input className="field num" inputMode="decimal" value={draft.extraPrice} onChange={(e) => onEdit({ extraPrice: e.target.value })} style={{ width: 104 }} disabled={busy} />
                  {currency === 'RUB' ? <span className="muted">{sym}</span> : null}
                </span>
              </label>
              <label className="tf-field">
                <span>В звёздах</span>
                <span className="tf-inline">
                  <input className="field num" inputMode="numeric" value={draft.extraStars} onChange={(e) => onEdit({ extraStars: e.target.value.replace(/\D/g, '').slice(0, 6) })} style={{ width: 88 }} disabled={busy} />
                  <span className="muted">{plural(Number(draft.extraStars) || 0, 'звезда', 'звезды', 'звёзд')}</span>
                </span>
              </label>
              <label className="tf-field">
                <span>Не больше</span>
                <span className="tf-inline">
                  <input className="field num" inputMode="numeric" value={draft.extraMax} onChange={(e) => onEdit({ extraMax: e.target.value.replace(/\D/g, '').slice(0, 3) })} style={{ width: 88 }} disabled={busy} />
                  <span className="muted">{extraWord}</span>
                </span>
              </label>
            </>
          ) : null}
        </div>
        {draft.extraOn ? (
          <div className="tf-col" style={{ gap: 8, marginTop: 8 }}>
            <span className="tf-bt">Скидка за доп. устройства</span>
            <span className="muted">Чем больше доп. устройств, тем больше скидка. Действует только на сумму за доп. устройства: при покупке, продлении, автопродлении, смене тарифа и докупке.</span>
            {draft.discounts.map((d) => {
              const from = Number(d.from) || 0;
              return (
                <div key={d.key} className="tf-period">
                  <label className="tf-inline">
                    <span className="muted">от</span>
                    <input
                      className="field num"
                      inputMode="numeric"
                      value={d.from}
                      onChange={(e) => setDiscount(d.key, { from: e.target.value.replace(/\D/g, '').slice(0, 3) })}
                      style={{ width: 72 }}
                      aria-label="С какого числа доп. устройств"
                      disabled={busy}
                    />
                    <span className="muted" style={{ minWidth: 112 }}>
                      {plural(from, 'доп. устройства', 'доп. устройств', 'доп. устройств')}
                    </span>
                  </label>
                  <label className="tf-inline">
                    <span className="muted">скидка</span>
                    <input
                      className="field num"
                      inputMode="numeric"
                      value={d.percent}
                      onChange={(e) => setDiscount(d.key, { percent: e.target.value.replace(/\D/g, '').slice(0, 2) })}
                      style={{ width: 64 }}
                      aria-label="Скидка в процентах"
                      disabled={busy}
                    />
                    <span className="muted">%</span>
                  </label>
                  <button type="button" className="tf-x hov" aria-label={`Убрать скидку от ${d.from || ''} доп. устройств`} onClick={() => onEdit({ discounts: draft.discounts.filter((x) => x.key !== d.key) })} disabled={busy}>
                    <Icon name="close" size={16} stroke={2} />
                  </button>
                </div>
              );
            })}
            <button
              type="button"
              className="tf-add"
              onClick={() => {
                const last = draft.discounts[draft.discounts.length - 1];
                const next = last ? String(Math.min((Number(last.from) || 0) + 1, Number(draft.extraMax) || 1)) : String(Math.min(2, Number(draft.extraMax) || 1));
                onEdit({ discounts: [...draft.discounts, { key: nextKey(), from: next, percent: '' }] });
              }}
              disabled={busy || draft.discounts.length >= 10}
            >
              Добавить ступень скидки
            </button>
          </div>
        ) : null}
        {hwidOff ? (
          <p className="tf-warn">
            Лимит устройств выключен в Remnawave: клиенты подключат сколько угодно устройств.{' '}
            <a href="/admin/settings/remnawave#devices" aria-label="Включить лимит устройств в Remnawave">
              Включить
            </a>
          </p>
        ) : null}
      </div>

      <div className="tf-block">
        <span className="tf-bt">Трафик</span>
        <div className="tf-wrap" style={{ alignItems: 'flex-start' }}>
          <label className="tf-field">
            <span>Лимит трафика</span>
            <span className="tf-inline">
              <input className="field num" inputMode="numeric" placeholder="Без лимита" value={draft.gb} onChange={(e) => onEdit({ gb: e.target.value.replace(/\D/g, '').slice(0, 6) })} style={{ width: 140 }} disabled={busy} />
              <span className="muted">ГБ</span>
            </span>
          </label>
          <div className="tf-field">
            <label htmlFor={`${ids}-reset`} style={{ fontWeight: 600 }}>
              Сброс
            </label>
            <select
              id={`${ids}-reset`}
              className="field"
              value={draft.reset}
              onChange={(e) => onEdit({ reset: e.target.value as TrafficReset })}
              aria-describedby={resetHint ? `${ids}-hint` : undefined}
              disabled={busy}
              style={{ width: 300, maxWidth: '100%' }}
            >
              {RESETS.map((r) => (
                <option key={r.value} value={r.value}>
                  {r.label}
                </option>
              ))}
            </select>
            {resetHint ? (
              <span id={`${ids}-hint`} className="cap">
                {resetHint}
              </span>
            ) : null}
          </div>
        </div>
        {draft.gb.trim() ? (
          <div className="tf-col" style={{ gap: 8, marginTop: 8 }}>
            <span className="tf-bt">Докупка трафика</span>
            <span className="muted">Пакеты, которые клиент покупает к действующей подписке. Цена пакета не зависит от того, сколько дней осталось.</span>
            {draft.packs.map((pack) => (
              <div key={pack.key} className="tf-period">
                <label className="tf-inline">
                  <span className="sr-only">Размер пакета</span>
                  <input className="field num" inputMode="numeric" value={pack.gb} onChange={(e) => setPack(pack.key, { gb: e.target.value.replace(/\D/g, '').slice(0, 6) })} style={{ width: 88 }} disabled={busy} />
                  <span className="muted" style={{ minWidth: 36 }}>
                    ГБ
                  </span>
                </label>
                <label className="tf-inline">
                  <span className="sr-only">Цена пакета</span>
                  {currency === 'USD' ? <span className="muted">{sym}</span> : null}
                  <input className="field num" inputMode="decimal" value={pack.price} onChange={(e) => setPack(pack.key, { price: e.target.value })} style={{ width: 104 }} disabled={busy} />
                  {currency === 'RUB' ? <span className="muted">{sym}</span> : null}
                </label>
                <label className="tf-inline">
                  <span className="sr-only">Цена пакета в звёздах Telegram</span>
                  <input className="field num" inputMode="numeric" value={pack.stars} onChange={(e) => setPack(pack.key, { stars: e.target.value.replace(/\D/g, '').slice(0, 7) })} style={{ width: 96 }} disabled={busy} />
                  <span className="muted" style={{ minWidth: 48 }}>
                    {plural(Number(pack.stars) || 0, 'звезда', 'звезды', 'звёзд')}
                  </span>
                </label>
                <button type="button" className="tf-x hov" aria-label={`Убрать пакет ${pack.gb || ''} ГБ`} onClick={() => onEdit({ packs: draft.packs.filter((x) => x.key !== pack.key) })} disabled={busy}>
                  <Icon name="close" size={16} stroke={2} />
                </button>
              </div>
            ))}
            <button
              type="button"
              className="tf-add"
              onClick={() => onEdit({ packs: [...draft.packs, { key: nextKey(), gb: '', price: '', stars: '', other: null }] })}
              disabled={busy || draft.packs.length >= 8}
            >
              Добавить пакет
            </button>
            {draft.packs.length ? (
              <div className="tf-col" style={{ gap: 6 }}>
                <span id={`${ids}-left`} style={{ fontWeight: 600 }}>
                  Неизрасходованный докупленный трафик
                </span>
                <span role="group" aria-labelledby={`${ids}-left`} className="segs" style={{ alignSelf: 'flex-start' }}>
                  {leftovers.map(([value, label]) => (
                    <button key={value} type="button" aria-pressed={draft.leftover === value} onClick={() => onEdit({ leftover: value })} disabled={busy}>
                      {label}
                    </button>
                  ))}
                </span>
                <span className="cap">{leftoverHint}</span>
              </div>
            ) : null}
          </div>
        ) : null}
      </div>

      <div className="tf-block">
        <div className="tf-col" style={{ gap: 2 }}>
          <span id={`${ids}-sq`} className="tf-bt">
            Сквады
          </span>
          <span className="muted">К каким нодам получит доступ клиент</span>
        </div>
        <div role="group" aria-labelledby={`${ids}-sq`} className="chips">
          {squads === null ? <span className="muted">Загружаем сквады…</span> : null}
          {squads && squads.length === 0 && unknown.length === 0 ? <span className="muted">Remnawave не отдал сквады: проверьте подключение в настройках</span> : null}
          {knownSquads.map((s) => {
            const on = draft.squads.includes(s.uuid);
            const lock = on && draft.squads.length === 1;
            return (
              <button key={s.uuid} type="button" className="chip" aria-pressed={on} aria-disabled={lock} onClick={() => toggleSquad(s.uuid)} style={lock ? { cursor: 'default', opacity: 1 } : undefined}>
                <span>{s.name}</span>
                {s.nodes !== null ? <span className="chip-n">{count(s.nodes, 'нода', 'ноды', 'нод')}</span> : null}
              </button>
            );
          })}
          {unknown.map((uuid) => (
            <button key={uuid} type="button" className="chip" aria-pressed="true" onClick={() => toggleSquad(uuid)} title="Сквада нет в Remnawave">
              <span>{uuid.slice(0, 8)}…</span>
              <span className="chip-n">нет в панели</span>
            </button>
          ))}
        </div>
        {draft.squads.length === 1 ? <span className="cap">Нужен хотя бы один сквад, последний снять нельзя.</span> : null}
      </div>

      <fieldset className="tf-fieldset">
        <legend className="tf-bt">Где показывать</legend>
        <div className="tf-wrap" style={{ gap: '10px 24px' }}>
          {(
            [
              ['bot', 'Бот'],
              ['cabinet', 'Кабинет'],
              ['miniapp', 'Мини-приложение'],
            ] as Array<[keyof Channels, string]>
          ).map(([key, label]) => (
            <label key={key} className="tf-check">
              <input type="checkbox" checked={draft.channels[key]} onChange={() => toggleChannel(key)} disabled={busy} />
              {label}
            </label>
          ))}
        </div>
      </fieldset>

      <div className="tf-col" style={{ gap: 16 }}>
        {draft.id !== null ? (
          <div className="tf-col" style={{ gap: 2 }}>
            <label className="tf-check" style={{ fontWeight: 600, width: 'fit-content', cursor: 'pointer' }}>
              <input type="checkbox" checked={draft.apply} onChange={() => onEdit({ apply: !draft.apply })} aria-describedby={`${ids}-apply`} disabled={busy} />
              <span>Применить к текущим подписчикам тарифа</span>
            </label>
            <span id={`${ids}-apply`} className="cap" style={{ paddingLeft: 25 }}>
              Без галочки изменения коснутся только новых покупок
            </span>
          </div>
        ) : null}
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <div className="tf-actions">
          <button type="button" className="btn-main btn" onClick={onSave} disabled={busy}>
            {busy ? 'Сохраняем…' : 'Сохранить'}
          </button>
          <button type="button" className="btn-line hov" onClick={onCancel} disabled={busy} style={{ padding: '0 16px' }}>
            Отменить
          </button>
          <span style={{ flex: 1 }} />
          {onDelete ? (
            <button type="button" className="tf-del" onClick={onDelete} disabled={busy}>
              Удалить тариф
            </button>
          ) : null}
        </div>
      </div>
    </div>
  );
}

type CategoryProps = {
  category: Category | null;
  multi: Multi | null;
  onMulti: (() => void) | null;
  onClose: () => void;
  onDone: (id: number) => Promise<void>;
};

function CategoryDialog({ category, multi, onMulti, onClose, onDone }: CategoryProps) {
  const [name, setName] = useState(category?.name ?? '');
  const [limit, setLimit] = useState(category?.subscription_limit ? String(category.subscription_limit) : '');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const field = useId();
  const limitField = useId();
  const save = async () => {
    const value = limit.trim() ? Number(limit) : null;
    if (value !== null && (!Number.isInteger(value) || value < 1 || value > 100)) {
      setError('Подписок этой категории у клиента: от 1 до 100 или пусто');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const body = { name: name.trim(), sort: category?.sort ?? 0, visible: category?.visible ?? true, subscription_limit: value };
      const saved = category
        ? await api<{ id: number }>(`/api/admin/categories/${category.id}`, { method: 'PUT', body })
        : await api<{ id: number }>('/api/admin/categories', { method: 'POST', body });
      await onDone(saved.id);
    } catch (err) {
      setError(failText(err, category ? 'Категория не сохранилась' : 'Категория не создалась'));
      setBusy(false);
    }
  };
  return (
    <Dialog title={category ? 'Категория' : 'Новая категория'} busy={busy} onClose={onClose}>
      <div className="field-row">
        <label htmlFor={field}>Название</label>
        <input id={field} className="field" maxLength={60} value={name} onChange={(e) => setName(e.target.value)} placeholder="Например: Белые списки" disabled={busy} autoFocus />
        <span className="cap">Клиенты увидят её как раздел каталога</span>
      </div>
      <div className="field-row">
        <label htmlFor={limitField}>Подписок этой категории у клиента</label>
        <span className="tf-inline">
          <input
            id={limitField}
            className="field num"
            inputMode="numeric"
            value={limit}
            onChange={(e) => setLimit(e.target.value.replace(/\D/g, '').slice(0, 3))}
            placeholder="Без ограничений"
            style={{ width: 168 }}
            disabled={busy}
          />
        </span>
        <span className="cap">
          Пусто — без ограничений. Когда у клиента набралось столько подписок этой категории, новую оформить нельзя: покупка тарифа этой категории продлит одну из них. Подписки, которые уже есть, остаются.
        </span>
        {multi && !multi.enabled ? (
          <p className="tf-warn">
            Сейчас у клиента может быть только одна подписка, поэтому ограничение не действует.{' '}
            {onMulti ? (
              <button type="button" className="tf-link" onClick={onMulti} disabled={busy}>
                Разрешить несколько подписок
              </button>
            ) : null}
          </p>
        ) : null}
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || !name.trim()}>
          {busy ? (category ? 'Сохраняем…' : 'Создаём…') : category ? 'Сохранить' : 'Создать'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

function MultiDialog({ value, onClose, onDone }: { value: Multi; onClose: () => void; onDone: (value: Multi) => void }) {
  const [enabled, setEnabled] = useState(value.enabled);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const save = async () => {
    setBusy(true);
    setError(null);
    try {
      onDone(await api<Multi>('/api/admin/settings/multi_subscriptions', { method: 'PUT', body: { enabled } }));
    } catch (err) {
      setError(failText(err, 'Настройка не сохранилась'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Подписки клиента" busy={busy} onClose={onClose} wide>
      <span className="tf-switchrow">
        <Switch on={enabled} onToggle={() => setEnabled(!enabled)} label="Несколько подписок у клиента" disabled={busy} />
        <span style={{ fontWeight: 600 }} aria-hidden="true">
          Несколько подписок у клиента
        </span>
      </span>
      {enabled ? (
        <p className="cap">
          Клиент может держать несколько подписок. В боте при покупке он выбирает, какую продлить, или оформляет новую. Тариф той же категории продлевает выбранную подписку, другой категории — переводит её на новый тариф. Сколько подписок одной категории может быть у клиента, задаётся в настройках категории.
        </p>
      ) : (
        <p className="cap">
          У клиента одна подписка. Покупка тарифа той же категории продлевает её. Тариф другой категории — переход: перед оплатой клиент видит, что текущая подписка будет удалена, а новая станет доступна сразу. Оставшиеся дни не переносятся, ссылка подписки остаётся прежней. Если у клиента уже несколько подписок, они сохранятся, новые не появятся.
        </p>
      )}
      <p className="cap">Имя в Remnawave — ник Telegram или имя из почты: у первой подписки без номера, у второй с _2, у третьей с _3. Если имя занято, берётся следующий свободный номер.</p>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy || enabled === value.enabled}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}

function variantDraft(variant: TrialVariant, index: number): VariantDraft {
  return {
    key: `v${variant.id || index}`,
    id: variant.id,
    name: variant.name,
    enabled: variant.enabled,
    days: String(variant.days),
    devices: String(variant.devices),
    gb: variant.traffic.limit_bytes ? String(Math.round(variant.traffic.limit_bytes / GIB)) : '',
    squads: variant.squads,
  };
}

function variantLabel(draft: VariantDraft, index: number): string {
  return draft.name.trim() ? `вариант «${draft.name.trim()}»` : `вариант ${index + 1}`;
}

function TrialDialog({ trial, squads, onClose, onDone }: { trial: Trial; squads: Squad[]; onClose: () => void; onDone: (trial: Trial) => void }) {
  const [enabled, setEnabled] = useState(trial.enabled);
  const [drafts, setDrafts] = useState<VariantDraft[]>(() => trial.variants.map(variantDraft));
  const [added, setAdded] = useState(0);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const ids = useId();
  const many = drafts.length > 1;
  const patch = (key: string, change: Partial<VariantDraft>) => setDrafts((list) => list.map((d) => (d.key === key ? { ...d, ...change } : d)));
  const add = () => {
    setDrafts((list) => [...list, { key: `n${added}`, id: 0, name: '', enabled: true, days: '3', devices: '1', gb: '', squads: [] }]);
    setAdded(added + 1);
  };
  const remove = (key: string) => setDrafts((list) => (list.length > 1 ? list.filter((d) => d.key !== key) : list));
  const save = async () => {
    const variants: TrialVariant[] = [];
    const names: string[] = [];
    for (const [index, draft] of drafts.entries()) {
      const label = variantLabel(draft, index);
      const name = draft.name.trim();
      if (many && !name) {
        setError(`Укажите название, ${label}: по нему клиент выбирает вариант`);
        return;
      }
      if (name.length > TRIAL_NAME_MAX) {
        setError(`Название, ${label}: до ${TRIAL_NAME_MAX} знаков`);
        return;
      }
      if (name && names.includes(name.toLowerCase())) {
        setError(`Название «${name}» уже есть у другого варианта`);
        return;
      }
      names.push(name.toLowerCase());
      const days = Number(draft.days);
      if (!Number.isInteger(days) || days < 1 || days > 90) {
        setError(many ? `Срок, ${label}: от 1 до 90 дней` : 'Срок пробного периода: от 1 до 90 дней');
        return;
      }
      const devices = Number(draft.devices);
      if (!Number.isInteger(devices) || devices < 1 || devices > 100) {
        setError(many ? `Устройств, ${label}: от 1 до 100` : 'Устройств: от 1 до 100');
        return;
      }
      const limit = draft.gb.trim() ? Number(draft.gb) : null;
      if (limit !== null && (!Number.isInteger(limit) || limit < 1)) {
        setError(many ? `Трафик, ${label}: целое число ГБ или пусто` : 'Трафик: целое число ГБ или пусто');
        return;
      }
      if (enabled && draft.enabled && draft.squads.length === 0) {
        setError(many ? `Выберите хотя бы один сквад, ${label}` : 'Выберите хотя бы один сквад');
        return;
      }
      variants.push({ id: draft.id, name, enabled: draft.enabled, days, devices, squads: draft.squads, traffic: { limit_bytes: limit === null ? null : limit * GIB, reset: 'no_reset' } });
    }
    if (enabled && !variants.some((v) => v.enabled)) {
      setError('Включите хотя бы один вариант или выключите пробный период');
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const next = await api<Trial>('/api/admin/settings/trial', { method: 'PUT', body: { enabled, variants } });
      onDone(next);
    } catch (err) {
      setError(failText(err, 'Настройки не сохранились'));
      setBusy(false);
    }
  };
  return (
    <Dialog title="Пробный период" busy={busy} onClose={onClose} wide>
      <span className="tf-switchrow">
        <Switch on={enabled} onToggle={() => setEnabled(!enabled)} label="Выдавать пробный период" />
        <span style={{ fontWeight: 600 }} aria-hidden="true">
          Выдавать пробный период новым клиентам
        </span>
      </span>
      {drafts.map((draft, index) => {
        const label = variantLabel(draft, index);
        return (
          <section key={draft.key} className={many ? 'tf-variant' : 'tf-block'} aria-label={many ? label : undefined}>
            {many ? (
              <div className="tf-variant-head">
                <label className="tf-field tf-variant-name" htmlFor={`${ids}-${draft.key}-n`}>
                  <span>Название для клиента</span>
                  <input id={`${ids}-${draft.key}-n`} className="field" value={draft.name} maxLength={TRIAL_NAME_MAX} placeholder="Например, Европа" onChange={(e) => patch(draft.key, { name: e.target.value })} disabled={busy} />
                </label>
                <span className="tf-switchrow">
                  <Switch on={draft.enabled} onToggle={() => patch(draft.key, { enabled: !draft.enabled })} label={`Выдавать: ${label}`} />
                  <span className="muted" aria-hidden="true">
                    {draft.enabled ? 'выдаётся' : 'не выдаётся'}
                  </span>
                </span>
                <button type="button" className="tf-link tf-variant-del" onClick={() => remove(draft.key)} disabled={busy}>
                  Убрать
                </button>
              </div>
            ) : null}
            <div className="tf-wrap">
              <label className="tf-field" htmlFor={`${ids}-${draft.key}-d`}>
                <span>Срок</span>
                <span className="tf-inline">
                  <input id={`${ids}-${draft.key}-d`} className="field num" inputMode="numeric" value={draft.days} onChange={(e) => patch(draft.key, { days: e.target.value.replace(/\D/g, '').slice(0, 2) })} style={{ width: 88 }} disabled={busy} />
                  <span className="muted">{plural(Number(draft.days) || 0, 'день', 'дня', 'дней')}</span>
                </span>
              </label>
              <label className="tf-field" htmlFor={`${ids}-${draft.key}-v`}>
                <span>Устройств</span>
                <input id={`${ids}-${draft.key}-v`} className="field num" inputMode="numeric" value={draft.devices} onChange={(e) => patch(draft.key, { devices: e.target.value.replace(/\D/g, '').slice(0, 3) })} style={{ width: 88 }} disabled={busy} />
              </label>
              <label className="tf-field" htmlFor={`${ids}-${draft.key}-g`}>
                <span>Трафик на весь срок</span>
                <span className="tf-inline">
                  <input id={`${ids}-${draft.key}-g`} className="field num" inputMode="numeric" placeholder="Без лимита" value={draft.gb} onChange={(e) => patch(draft.key, { gb: e.target.value.replace(/\D/g, '').slice(0, 6) })} style={{ width: 120 }} disabled={busy} />
                  <span className="muted">ГБ</span>
                </span>
              </label>
            </div>
            <div className="field-row">
              <span style={{ fontWeight: 600 }}>Сквады</span>
              <div role="group" aria-label={many ? `Сквады, ${label}` : 'Сквады пробного периода'} className="chips">
                {squads.map((s) => {
                  const on = draft.squads.includes(s.uuid);
                  return (
                    <button key={s.uuid} type="button" className="chip" aria-pressed={on} onClick={() => patch(draft.key, { squads: on ? draft.squads.filter((x) => x !== s.uuid) : [...draft.squads, s.uuid] })} disabled={busy}>
                      <span>{s.name}</span>
                    </button>
                  );
                })}
                {squads.length === 0 ? <span className="muted">Remnawave не отдал сквады</span> : null}
              </div>
            </div>
          </section>
        );
      })}
      <div className="tf-variant-more">
        {drafts.length < TRIAL_VARIANTS_MAX ? (
          <button type="button" className="btn-line hov" onClick={add} disabled={busy}>
            Добавить вариант
          </button>
        ) : null}
        <span className="muted">
          {many
            ? 'Клиент выбирает вариант в боте, кабинете или мини-приложении. Пробный период даётся один раз, какой бы вариант клиент ни выбрал.'
            : 'Несколько вариантов нужны, если пробный период разный, например по локациям: свои сквады, срок, устройства и трафик.'}
        </span>
      </div>
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
          {busy ? 'Сохраняем…' : 'Сохранить'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Отменить
        </button>
      </div>
    </Dialog>
  );
}
