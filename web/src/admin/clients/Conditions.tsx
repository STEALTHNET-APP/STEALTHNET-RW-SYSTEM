import { useEffect, useRef, useState, type ReactNode } from 'react';
import { Icon } from '../../lib/icons';
import type { Currency } from '../../lib/types';
import {
  DAYS_MAX,
  KINDS,
  ORIGINS,
  STATUSES,
  activeKinds,
  clearKind,
  isSet,
  kindDef,
  label,
  type Conditions,
  type Flag,
  type FilterOptions,
  type Kind,
  type Origin,
} from './filters';

export type FilterMode = 'clients' | 'broadcast' | 'rule';

type FieldsProps = {
  kind: Kind;
  value: Conditions;
  onChange: (next: Conditions) => void;
  options: FilterOptions | null;
  currency: Currency;
  mode: FilterMode;
};

function FlagSegs({ value, onChange, yes, no, any, name }: { value: Flag; onChange: (next: Flag) => void; yes: string; no: string; any?: string; name: string }) {
  return (
    <span className="segs" role="group" aria-label={name}>
      <button type="button" aria-pressed={value === true} onClick={() => onChange(true)}>
        {yes}
      </button>
      <button type="button" aria-pressed={value === false} onClick={() => onChange(false)}>
        {no}
      </button>
      {any ? (
        <button type="button" aria-pressed={value === null} onClick={() => onChange(null)}>
          {any}
        </button>
      ) : null}
    </span>
  );
}

function toggle<T>(list: T[], item: T): T[] {
  return list.includes(item) ? list.filter((x) => x !== item) : [...list, item];
}

function digits(raw: string, max: number): number | null {
  const clean = raw.replace(/[^\d]/g, '').slice(0, 10);
  if (!clean) {
    return null;
  }
  return Math.min(Number.parseInt(clean, 10), max);
}

export function ConditionFields({ kind, value, onChange, options, currency, mode }: FieldsProps) {
  const set = (patch: Partial<Conditions>) => onChange({ ...value, ...patch });
  switch (kind) {
    case 'status':
      return (
        <div className="flt-list" role="radiogroup" aria-label="Состояние подписки">
          {STATUSES.filter(([key]) => mode === 'clients' || key !== 'blocked').map(([key, title]) => (
            <label key={key} className="flt-check">
              <input type="radio" name={`flt-status-${mode}`} checked={value.status === key} onChange={() => set({ status: key })} />
              <span>{title}</span>
            </label>
          ))}
        </div>
      );
    case 'tariff': {
      const tariffs = options?.tariffs ?? [];
      if (!tariffs.length) {
        return <p className="cap">Тарифов пока нет</p>;
      }
      return (
        <div className="flt-list">
          {(options?.categories ?? []).map((category) => {
            const inside = tariffs.filter((t) => t.category_id === category.id);
            if (!inside.length) {
              return null;
            }
            return (
              <div key={category.id} className="flt-group">
                <span className="flt-group-name">{category.name}</span>
                {inside.map((t) => (
                  <label key={t.id} className="flt-check">
                    <input type="checkbox" checked={value.tariff.includes(t.id)} onChange={() => set({ tariff: toggle(value.tariff, t.id) })} />
                    <span>
                      {t.name}
                      {t.deleted ? <span className="muted"> · удалён</span> : null}
                    </span>
                  </label>
                ))}
              </div>
            );
          })}
        </div>
      );
    }
    case 'category':
      return (
        <div className="flt-list">
          {(options?.categories ?? []).map((category) => (
            <label key={category.id} className="flt-check">
              <input type="checkbox" checked={value.category.includes(category.id)} onChange={() => set({ category: toggle(value.category, category.id) })} />
              <span>{category.name}</span>
            </label>
          ))}
        </div>
      );
    case 'term': {
      const past = value.ended !== null;
      const n = past ? value.ended : value.ends;
      return (
        <div className="flt-line">
          <select
            className="field flt-select"
            aria-label="Когда"
            value={past ? 'ended' : 'ends'}
            onChange={(e) => set(e.target.value === 'ended' ? { ended: n ?? 7, ends: null } : { ends: n ?? 7, ended: null })}
          >
            <option value="ends">Закончится в ближайшие</option>
            <option value="ended">Закончилась за последние</option>
          </select>
          <input
            className="fld"
            inputMode="numeric"
            aria-label="Сколько дней"
            value={n ?? ''}
            onChange={(e) => {
              const next = digits(e.target.value, DAYS_MAX);
              set(past ? { ended: next } : { ends: next });
            }}
          />
          <span>дней</span>
        </div>
      );
    }
    case 'autorenew':
      return <FlagSegs name="Автопродление" value={value.autorenew} onChange={(v) => set({ autorenew: v })} yes="Включено" no="Выключено" />;
    case 'source':
      return (
        <div className="flt-list">
          {ORIGINS.map(([key, title]) => (
            <label key={key} className="flt-check">
              <input type="checkbox" checked={value.source.includes(key)} onChange={() => set({ source: toggle<Origin>(value.source, key) })} />
              <span>{title}</span>
            </label>
          ))}
        </div>
      );
    case 'lang':
      return (
        <span className="segs" role="group" aria-label="Язык">
          <button type="button" aria-pressed={value.lang === 'ru'} onClick={() => set({ lang: 'ru' })}>
            Русский
          </button>
          <button type="button" aria-pressed={value.lang === 'en'} onClick={() => set({ lang: 'en' })}>
            Английский
          </button>
        </span>
      );
    case 'registered':
      return (
        <div className="flt-line">
          <label className="flt-date">
            <span className="cap">С</span>
            <input type="date" className="field" value={value.from ?? ''} max={value.to ?? undefined} onChange={(e) => set({ from: e.target.value || null })} />
          </label>
          <label className="flt-date">
            <span className="cap">По</span>
            <input type="date" className="field" value={value.to ?? ''} min={value.from ?? undefined} onChange={(e) => set({ to: e.target.value || null })} />
          </label>
        </div>
      );
    case 'contact':
      return (
        <div className="flt-rows">
          <div className="flt-row">
            <span>Telegram</span>
            <FlagSegs name="Telegram" value={value.telegram} onChange={(v) => set({ telegram: v })} yes="Есть" no="Нет" any="Не важно" />
          </div>
          <div className="flt-row">
            <span>Почта</span>
            <FlagSegs name="Почта" value={value.email} onChange={(v) => set({ email: v })} yes="Есть" no="Нет" any="Не важно" />
          </div>
        </div>
      );
    case 'bot':
      return <FlagSegs name="Бот" value={value.bot_stopped} onChange={(v) => set({ bot_stopped: v })} yes="Остановлен" no="Не остановлен" />;
    case 'paid': {
      const sign = currency === 'RUB' ? '₽' : '$';
      return (
        <div className="flt-rows">
          <FlagSegs
            name="Оплаты"
            value={value.paid}
            onChange={(v) => set(v === false ? { paid: false, paid_from: null, paid_to: null } : { paid: v })}
            yes="Платили"
            no="Не платили"
          />
          {value.paid !== false ? (
            <div className="flt-line">
              <span>Всего оплачено от</span>
              <input
                className="fld flt-money"
                inputMode="numeric"
                aria-label="Сумма от"
                value={value.paid_from ?? ''}
                onChange={(e) => set({ paid_from: digits(e.target.value, 1_000_000_000) })}
              />
              <span>до</span>
              <input
                className="fld flt-money"
                inputMode="numeric"
                aria-label="Сумма до"
                value={value.paid_to ?? ''}
                onChange={(e) => set({ paid_to: digits(e.target.value, 1_000_000_000) })}
              />
              <span>{sign}</span>
            </div>
          ) : null}
        </div>
      );
    }
    case 'balance':
      return <FlagSegs name="Баланс" value={value.balance} onChange={(v) => set({ balance: v })} yes="Есть деньги" no="Пустой" />;
    case 'referral':
      return (
        <div className="flt-rows">
          <div className="flt-row">
            <span>Пришёл по приглашению</span>
            <FlagSegs name="Пришёл по приглашению" value={value.invited} onChange={(v) => set({ invited: v })} yes="Да" no="Нет" any="Не важно" />
          </div>
          <div className="flt-row">
            <span>Приглашал друзей</span>
            <FlagSegs name="Приглашал друзей" value={value.invites} onChange={(v) => set({ invites: v })} yes="Да" no="Нет" any="Не важно" />
          </div>
        </div>
      );
    case 'utm':
      return (
        <div className="flt-rows">
          <input
            className="field mono"
            list={`flt-utm-${mode}`}
            placeholder="Например, tiktok"
            maxLength={256}
            aria-label="Метка или старт-параметр"
            value={value.utm ?? ''}
            onChange={(e) => set({ utm: e.target.value.replace(/[^A-Za-z0-9_-]/g, '') || null })}
          />
          <datalist id={`flt-utm-${mode}`}>
            {(options?.utm ?? []).map((v) => (
              <option key={v} value={v} />
            ))}
          </datalist>
          <span className="cap">Любая метка ссылки (источник, кампания…) или старт-параметр целиком</span>
        </div>
      );
  }
}

function useDismiss(open: boolean, close: () => void, box: React.RefObject<HTMLElement | null>) {
  useEffect(() => {
    if (!open) {
      return;
    }
    const onDown = (event: MouseEvent) => {
      if (box.current && !box.current.contains(event.target as Node)) {
        close();
      }
    };
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        close();
      }
    };
    document.addEventListener('mousedown', onDown);
    document.addEventListener('keydown', onKey);
    return () => {
      document.removeEventListener('mousedown', onDown);
      document.removeEventListener('keydown', onKey);
    };
  }, [open, close, box]);
}

export type BarProps = {
  value: Conditions;
  onChange: (next: Conditions) => void;
  options: FilterOptions | null;
  currency: Currency;
  money: boolean;
  mode: FilterMode;
  phone: boolean;
  tail?: ReactNode;
  exclude?: Kind[];
};

function kindsFor(mode: FilterMode, money: boolean, exclude: Kind[] = []) {
  return KINDS.filter((def) => (mode === 'broadcast' || def.kind !== 'status') && (money || !def.money) && !exclude.includes(def.kind));
}

function Sheet({ title, onClose, children }: { title: string; onClose: () => void; children: ReactNode }) {
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (event.key === 'Escape') {
        onClose();
      }
    };
    document.addEventListener('keydown', onKey);
    return () => document.removeEventListener('keydown', onKey);
  }, [onClose]);
  return (
    <div className="psheet-back" role="presentation" onClick={onClose}>
      <div className="psheet flt-sheet" role="dialog" aria-modal="true" aria-label={title} onClick={(event) => event.stopPropagation()}>
        <div className="flt-sheet-head">
          <b>{title}</b>
          <button type="button" className="flt-x" aria-label="Закрыть" onClick={onClose}>
            <Icon name="close" size={18} stroke={2.2} />
          </button>
        </div>
        {children}
      </div>
    </div>
  );
}

export function FilterBar({ value, onChange, options, currency, money, mode, phone, tail, exclude }: BarProps) {
  const [menu, setMenu] = useState(false);
  const [editing, setEditing] = useState<Kind | null>(null);
  const menuBox = useRef<HTMLDivElement>(null);
  const editBox = useRef<HTMLDivElement>(null);
  const closeMenu = () => setMenu(false);
  const closeEdit = () => setEditing(null);
  useDismiss(menu && !phone, closeMenu, menuBox);
  useDismiss(editing !== null && !phone, closeEdit, editBox);
  const kinds = kindsFor(mode, money, exclude);
  const shown = activeKinds(value, mode === 'clients').filter((kind) => kinds.some((def) => def.kind === kind));
  if (editing && !shown.includes(editing)) {
    shown.push(editing);
  }
  const free = kinds.filter((def) => !isSet(value, def.kind) && def.kind !== editing);
  const groups = [...new Set(free.map((def) => def.group))];
  const pick = (kind: Kind) => {
    setMenu(false);
    if (kind === 'term' && !isSet(value, 'term')) {
      onChange({ ...value, ends: 7 });
    }
    if (kind === 'status' && !value.status) {
      onChange({ ...value, status: 'active' });
    }
    setEditing(kind);
  };
  const menuList = (
    <div className="flt-menu-list" role="menu">
      {groups.map((group) => (
        <div key={group} className="flt-menu-group" role="group" aria-label={group}>
          <span className="flt-group-name">{group}</span>
          {free
            .filter((def) => def.group === group)
            .map((def) => (
              <button key={def.kind} type="button" role="menuitem" className="flt-menu-item hov" onClick={() => pick(def.kind)}>
                {def.title}
              </button>
            ))}
        </div>
      ))}
    </div>
  );
  const editor = (kind: Kind) => (
    <>
      <ConditionFields kind={kind} value={value} onChange={onChange} options={options} currency={currency} mode={mode} />
      <div className="flt-pop-acts">
        <button type="button" className="btn-main btn flt-done" onClick={closeEdit}>
          Готово
        </button>
        {isSet(value, kind) ? (
          <button
            type="button"
            className="b38"
            onClick={() => {
              onChange(clearKind(value, kind));
              closeEdit();
            }}
          >
            Убрать условие
          </button>
        ) : null}
      </div>
    </>
  );
  return (
    <div className="flt-bar" role="group" aria-label="Условия отбора">
      <div className="flt-item" ref={menuBox}>
        <button type="button" className="flt-add" aria-expanded={menu} onClick={() => setMenu(!menu)}>
          <Icon name="plus" size={16} stroke={2.4} />
          Условие
        </button>
        {menu && !phone ? <div className="flt-pop flt-menu">{menuList}</div> : null}
      </div>
      {shown.map((kind) => {
        const on = isSet(value, kind);
        return (
          <div key={kind} className="flt-item" ref={editing === kind ? editBox : undefined}>
            <span className={editing === kind ? 'flt-chip open' : 'flt-chip'}>
              <button type="button" className="flt-chip-text" aria-expanded={editing === kind} onClick={() => setEditing(editing === kind ? null : kind)}>
                {on ? label(kind, value, options, currency) : `${kindDef(kind).title}: не выбрано`}
              </button>
              {on ? (
                <button type="button" className="flt-chip-x" aria-label={`Убрать условие: ${kindDef(kind).title}`} onClick={() => onChange(clearKind(value, kind))}>
                  <Icon name="close" size={14} stroke={2.2} />
                </button>
              ) : null}
            </span>
            {editing === kind && !phone ? (
              <div className="flt-pop" role="dialog" aria-label={kindDef(kind).title}>
                <b className="flt-pop-title">{kindDef(kind).title}</b>
                {editor(kind)}
              </div>
            ) : null}
          </div>
        );
      })}
      {tail ? <span className="flt-tail">{tail}</span> : null}
      {phone && menu ? (
        <Sheet title="Добавить условие" onClose={closeMenu}>
          {menuList}
        </Sheet>
      ) : null}
      {phone && editing ? (
        <Sheet title={kindDef(editing).title} onClose={closeEdit}>
          <div className="flt-sheet-body">{editor(editing)}</div>
        </Sheet>
      ) : null}
    </div>
  );
}

export function filterCount(value: Conditions, mode: FilterMode, exclude: Kind[] = []): number {
  return activeKinds(value, mode === 'clients').filter((kind) => !exclude.includes(kind)).length;
}
