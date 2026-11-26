import type { ReactNode } from 'react';
import type { Permission } from '../lib/types';
import { Switch } from './ui';
import './form.css';

export type FieldKind = { type: 'text' | 'secret' | 'number' | 'flag' | 'url' | 'list' } | { type: 'select'; options: string[] };

export type FieldCase = { when: string; hint?: string; placeholder?: string };

export type SettingField = {
  key: string;
  label: string;
  hint: string;
  kind: FieldKind;
  required: boolean;
  default: string | null;
  large?: boolean;
  labels?: string[];
  depends?: { on: string; cases: FieldCase[]; reset: boolean };
  need?: Permission;
  origin?: boolean;
};

export type FieldValues = Record<string, string | boolean>;

export type FieldSlot = {
  id: string;
  value: string;
  placeholder?: string;
  hint: ReactNode;
  hintId?: string;
  set: (value: string) => void;
};

export type SecretState = { clear: string[]; replacing: string[] };

export function fieldInitial(field: SettingField, config: Record<string, unknown>): string | boolean {
  const stored = config[field.key];
  if (field.kind.type === 'secret') {
    return '';
  }
  if (field.kind.type === 'flag') {
    return typeof stored === 'boolean' ? stored : field.default === 'true';
  }
  if (stored === null || stored === undefined) {
    return field.default ?? '';
  }
  return String(stored);
}

export function initialValues(schema: SettingField[], config: Record<string, unknown>): FieldValues {
  return Object.fromEntries(schema.filter((f) => f.kind.type !== 'list').map((f) => [f.key, fieldInitial(f, config)]));
}

export function secretFilled(config: Record<string, unknown>, key: string): boolean {
  const value = config[key];
  return typeof value === 'object' && value !== null && (value as { filled?: boolean }).filled === true;
}

export function changedConfig(schema: SettingField[], config: Record<string, unknown>, values: FieldValues): Record<string, unknown> {
  const out: Record<string, unknown> = {};
  for (const field of schema) {
    const value = values[field.key];
    if (field.kind.type === 'list') {
      continue;
    }
    if (field.kind.type === 'secret') {
      if (typeof value === 'string' && value.trim()) {
        out[field.key] = value.trim();
      }
      continue;
    }
    if (value === fieldInitial(field, config)) {
      continue;
    }
    out[field.key] = value === '' ? null : value;
  }
  return out;
}

function moved(schema: SettingField[], config: Record<string, unknown>, values: FieldValues, field: SettingField): boolean {
  const on = field.depends?.reset ? schema.find((f) => f.key === field.depends?.on) : undefined;
  return on !== undefined && values[on.key] !== fieldInitial(on, config);
}

function fieldCase(field: SettingField, values: FieldValues): FieldCase | undefined {
  const depends = field.depends;
  return depends ? depends.cases.find((c) => c.when === String(values[depends.on] ?? '')) : undefined;
}

export function missingRequired(schema: SettingField[], config: Record<string, unknown>, values: FieldValues, secrets: SecretState): SettingField[] {
  return schema.filter((field) => {
    if (!field.required || field.kind.type === 'list') {
      return false;
    }
    const value = values[field.key];
    if (field.kind.type === 'secret') {
      const kept = secretFilled(config, field.key) && !secrets.clear.includes(field.key) && !moved(schema, config, values, field);
      return !kept && !(typeof value === 'string' && value.trim());
    }
    if (field.kind.type === 'flag') {
      return false;
    }
    return typeof value !== 'string' || value.trim() === '';
  });
}

type Props = {
  schema: SettingField[];
  config: Record<string, unknown>;
  values: FieldValues;
  onChange: (values: FieldValues) => void;
  secrets: SecretState;
  onSecrets: (secrets: SecretState) => void;
  busy: boolean;
  idPrefix: string;
  custom?: Partial<Record<string, (slot: FieldSlot) => ReactNode>>;
  locked?: (field: SettingField) => string | null;
};

export function SchemaFields({ schema, config, values, onChange, secrets, onSecrets, busy, idPrefix, custom, locked }: Props) {
  const set = (key: string, value: string | boolean) => {
    const next: FieldValues = { ...values, [key]: value };
    const changer = schema.find((f) => f.key === key);
    const back = changer !== undefined && value === fieldInitial(changer, config);
    const reopen: string[] = [];
    for (const field of schema) {
      if (!field.depends?.reset || field.depends.on !== key || field.kind.type === 'list') {
        continue;
      }
      if (field.kind.type === 'secret') {
        if (!back && !secrets.replacing.includes(field.key)) {
          reopen.push(field.key);
        }
        continue;
      }
      next[field.key] = back ? fieldInitial(field, config) : '';
    }
    onChange(next);
    if (reopen.length) {
      onSecrets({ ...secrets, replacing: [...secrets.replacing, ...reopen] });
    }
  };
  const render = (field: SettingField): ReactNode => {
    const value = values[field.key];
    const id = `${idPrefix}-${field.key}`;
    const variant = fieldCase(field, values);
    const hintText = variant?.hint ?? field.hint;
    const placeholder = variant?.placeholder;
    const lock = locked?.(field) ?? null;
    const hintId = hintText || lock ? `${id}-hint` : undefined;
    const hint = hintId ? (
      <span id={hintId} className="cap">
        {[hintText, lock].filter(Boolean).join('. ')}
      </span>
    ) : null;
    const own = custom?.[field.key];
    if (own) {
      return (
        <div key={field.key} className="sf-field">
          <label htmlFor={id} className="sf-label">
            {field.label}
          </label>
          {own({ id, value: String(value ?? ''), placeholder, hint, hintId, set: (next) => set(field.key, next) })}
        </div>
      );
    }
    if (field.kind.type === 'flag') {
      return (
        <div key={field.key} className="sf-flag">
          <span className="sf-text">
            <span className="sf-label">{field.label}</span>
            {hint}
          </span>
          <Switch on={value === true} onToggle={() => set(field.key, value !== true)} label={field.label} describedBy={hintId} disabled={busy} />
        </div>
      );
    }
    if (field.kind.type === 'secret') {
      const filled = secretFilled(config, field.key) && !secrets.clear.includes(field.key);
      const editing = !filled || secrets.replacing.includes(field.key);
      return (
        <div key={field.key} className="sf-field">
          <label htmlFor={id} className="sf-label">
            {field.label}
          </label>
          {editing ? (
            <input
              id={id}
              className="field mono"
              type="password"
              autoComplete="off"
              spellCheck={false}
              value={typeof value === 'string' ? value : ''}
              onChange={(e) => set(field.key, e.target.value)}
              aria-describedby={hintId}
              placeholder={filled && !moved(schema, config, values, field) ? 'Новое значение' : 'Вставьте значение'}
              disabled={busy}
            />
          ) : (
            <span className="sf-secret">
              <span className="muted">Сохранено</span>
              <button type="button" className="b34 hov" onClick={() => onSecrets({ ...secrets, replacing: [...secrets.replacing, field.key] })} disabled={busy}>
                Заменить
              </button>
              {!field.required ? (
                <button type="button" className="b34 hov" onClick={() => onSecrets({ ...secrets, clear: [...secrets.clear, field.key] })} disabled={busy}>
                  Удалить
                </button>
              ) : null}
            </span>
          )}
          {hint}
        </div>
      );
    }
    return (
      <div key={field.key} className="sf-field">
        <label htmlFor={id} className="sf-label">
          {field.label}
        </label>
        {field.kind.type === 'text' && field.large ? (
          <textarea
            id={id}
            className="field"
            rows={8}
            value={String(value ?? '')}
            onChange={(e) => set(field.key, e.target.value)}
            aria-describedby={hintId}
            disabled={busy || lock !== null}
          />
        ) : field.kind.type === 'select' ? (
          <select id={id} className="field" value={String(value ?? '')} onChange={(e) => set(field.key, e.target.value)} aria-describedby={hintId} disabled={busy || lock !== null}>
            {field.required ? null : <option value="">Не выбрано</option>}
            {field.kind.options.map((option, index) => (
              <option key={option} value={option}>
                {field.labels?.[index] ?? option}
              </option>
            ))}
          </select>
        ) : (
          <input
            id={id}
            className="field"
            type={field.kind.type === 'url' ? 'url' : 'text'}
            inputMode={field.kind.type === 'number' ? 'decimal' : undefined}
            value={String(value ?? '')}
            onChange={(e) => set(field.key, e.target.value)}
            aria-describedby={hintId}
            placeholder={placeholder}
            spellCheck={placeholder ? false : undefined}
            disabled={busy || lock !== null}
          />
        )}
        {hint}
      </div>
    );
  };
  return <>{schema.map((field) => (field.kind.type === 'list' ? null : render(field)))}</>;
}
