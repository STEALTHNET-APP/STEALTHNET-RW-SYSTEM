import { useCallback, useEffect, useState } from 'react';
import { Link, useParams } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { pathPart } from '../../lib/apiPath';
import { useSession } from '../session';
import { PAYOUT_KEY, PayoutMethods, payoutItems, payoutProblem, payoutRows, type PayoutRow } from '../referral/PayoutMethods';
import { changedConfig, initialValues, missingRequired, SchemaFields, type FieldValues, type SecretState } from '../SchemaFields';
import { Switch } from '../ui';
import { AiModelField } from './AiModelField';
import { AiUsage } from './AiUsage';
import type { ModuleConfig, ModulesData, ModuleView } from './model';
import { OauthSetup } from './OauthSetup';
import '../form.css';
import './modules.css';

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

const NO_SECRETS: SecretState = { clear: [], replacing: [] };

const OWN_SCREENS: Record<string, string> = { promolinks: 'Промо-ссылки' };

export function ModulePage() {
  const { id = '' } = useParams();
  const { reload, me } = useSession();
  const [view, setView] = useState<ModuleView | null>(null);
  const [cfg, setCfg] = useState<ModuleConfig | null>(null);
  const [values, setValues] = useState<FieldValues>({});
  const [secrets, setSecrets] = useState<SecretState>(NO_SECRETS);
  const [missing, setMissing] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [note, setNote] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [saved, setSaved] = useState(0);
  const [payouts, setPayouts] = useState<PayoutRow[] | null>(null);
  const [payoutsSaved, setPayoutsSaved] = useState('');
  const [invalid, setInvalid] = useState<string | null>(null);

  const apply = (next: ModuleConfig) => {
    setCfg(next);
    setValues(initialValues(next.schema, next.config));
    setSecrets(NO_SECRETS);
    const field = next.schema.find((f) => f.key === PAYOUT_KEY && f.kind.type === 'list');
    const rows = field ? payoutRows(next.config[PAYOUT_KEY], field.default) : null;
    setPayouts(rows);
    setPayoutsSaved(rows ? JSON.stringify(payoutItems(rows)) : '');
    setInvalid(null);
  };

  const load = useCallback(async () => {
    try {
      const [list, config] = await Promise.all([api<ModulesData>('/api/admin/modules'), api<ModuleConfig>(`/api/admin/modules/${pathPart(id)}/config`)]);
      const found = list.modules.find((m) => m.info.id === id) ?? null;
      setMissing(found === null);
      setView(found);
      apply(config);
      setError(null);
    } catch (err) {
      if (err instanceof ApiError && err.status === 404) {
        setMissing(true);
        return;
      }
      setError(failText(err, 'Модуль не загрузился'));
    }
  }, [id]);

  useEffect(() => {
    void load();
  }, [load]);

  const toggle = async () => {
    if (!view || view.locked || busy) {
      return;
    }
    setBusy(true);
    setNote(null);
    try {
      setView(await api<ModuleView>(`/api/admin/modules/${pathPart(id)}`, { method: 'PATCH', body: { enabled: !view.active } }));
      await reload();
    } catch (err) {
      setError(failText(err, 'Модуль не переключился'));
    } finally {
      setBusy(false);
    }
  };

  const save = async () => {
    if (!cfg) {
      return;
    }
    const lacking = missingRequired(cfg.schema, cfg.config, values, secrets);
    if (lacking.length) {
      setError(`Заполните: ${lacking.map((f) => f.label).join(', ')}`);
      return;
    }
    if (payouts) {
      const problem = payoutProblem(payouts, values);
      if (problem) {
        setError(problem.text);
        setInvalid(problem.key);
        return;
      }
    }
    const config = changedConfig(cfg.schema, cfg.config, values);
    if (payouts && JSON.stringify(payoutItems(payouts)) !== payoutsSaved) {
      config[PAYOUT_KEY] = payoutItems(payouts);
    }
    const clear = secrets.clear;
    setBusy(true);
    setError(null);
    setNote(null);
    try {
      apply(await api<ModuleConfig>(`/api/admin/modules/${pathPart(id)}/config`, { method: 'PUT', body: { config, clear } }));
      const list = await api<ModulesData>('/api/admin/modules');
      setView(list.modules.find((m) => m.info.id === id) ?? view);
      setSaved((n) => n + 1);
      setNote('Настройки сохранены');
    } catch (err) {
      setError(failText(err, 'Настройки не сохранились'));
    } finally {
      setBusy(false);
    }
  };

  if (missing) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        <h1 className="page-title tight">Модуль не найден</h1>
        <p className="muted">Такого модуля нет в этой установке.</p>
      </div>
    );
  }

  if (!view || !cfg) {
    return (
      <div className="md-page">
        <Link to="/admin/modules" className="md-back">
          Все модули
        </Link>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем модуль…</p>
        )}
      </div>
    );
  }

  const section = view.info.menu?.label ?? OWN_SCREENS[view.info.id] ?? null;
  const payoutField = cfg.schema.find((f) => f.key === PAYOUT_KEY && f.kind.type === 'list') ?? null;

  return (
    <div className="md-page">
      <Link to="/admin/modules" className="md-back">
        Все модули
      </Link>
      <div className="md-title">
        <h1 className="page-title tight">{view.info.title}</h1>
        <p>{view.info.description}</p>
      </div>
      <section className="card md-state" aria-label="Состояние модуля">
        <div className="md-text">
          <span className="md-name">{view.active ? 'Модуль включён' : 'Модуль выключен'}</span>
          <span className="md-where">Где работает: {view.info.where_used}</span>
          {view.active && view.warning ? <span className="md-warn">{view.warning}</span> : null}
          {view.locked ? <span className="md-note">{view.locked}</span> : null}
        </div>
        <Switch on={view.active} onToggle={() => void toggle()} label={view.info.title} disabled={busy || view.locked !== null} />
      </section>
      {view.info.id === 'ai' && view.active ? <AiUsage version={saved} /> : null}
      {section && view.active ? (
        <Link to={`/admin/m/${view.info.id}`} className="md-back">
          Открыть раздел «{section}»
        </Link>
      ) : null}
      {view.info.id === 'referral' ? (
        <Link to="/admin/settings/bot/texts?section=referral" className="md-back">
          Текст «Пригласить друга» в боте
        </Link>
      ) : null}
      {view.info.id === 'oauth' ? <OauthSetup /> : null}
      {cfg.schema.length === 0 ? (
        <section className="card card-pad">
          <p className="muted">{section ? `Настройки и список — в разделе «${section}».` : 'Своих настроек у модуля нет: он работает сам там, где указано выше.'}</p>
        </section>
      ) : (
        <section className="card md-form" aria-labelledby="md-form-h">
          <h2 id="md-form-h" className="section-title">
            Настройки
          </h2>
          <SchemaFields
            schema={cfg.schema}
            config={cfg.config}
            values={values}
            onChange={setValues}
            secrets={secrets}
            onSecrets={setSecrets}
            busy={busy}
            idPrefix={`mod-${id}`}
            custom={view.info.id === 'ai' ? { model: (slot) => <AiModelField slot={slot} values={values} config={cfg.config} busy={busy} /> } : undefined}
          />
          {payoutField && payouts ? (
            <PayoutMethods
              field={payoutField}
              rows={payouts}
              onChange={(rows) => {
                setPayouts(rows);
                setInvalid(null);
              }}
              values={values}
              currency={me.currency}
              busy={busy}
              invalid={invalid}
            />
          ) : null}
          {error ? (
            <p className="err" role="alert">
              {error}
            </p>
          ) : null}
          <div className="md-actions">
            <button type="button" className="btn-main btn" onClick={() => void save()} disabled={busy}>
              {busy ? 'Сохраняем…' : 'Сохранить'}
            </button>
            {note ? (
              <span role="status" className="muted">
                {note}
              </span>
            ) : null}
          </div>
        </section>
      )}
      {cfg.schema.length === 0 && error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
    </div>
  );
}
