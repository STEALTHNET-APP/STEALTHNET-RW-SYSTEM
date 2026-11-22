import { useId, useState, type ReactNode } from 'react';
import { api } from '../../lib/api';
import type { Currency } from '../../lib/types';
import {
  AUDIENCES,
  BASE,
  buildBody,
  failText,
  messageKey,
  PLACEHOLDERS,
  PRIZE_KINDS,
  type Audience,
  type Contest,
  type Draft,
  type DraftErrors,
  type MessageDraft,
  type Options,
  type PrizeFields,
  type TierDraft,
} from './model';

type Props = {
  phone: boolean;
  currency: Currency;
  options: Options | null;
  initial: Draft;
  contest: Contest | null;
  onCancel: () => void;
  onSaved: (contest: Contest) => void;
};

type FieldProps = {
  id: string;
  label: string;
  hint?: string;
  error?: string;
  className?: string;
  children: ReactNode;
};

function Field({ id, label, hint, error, className, children }: FieldProps) {
  return (
    <div className={className ? `pf-field ${className}` : 'pf-field'}>
      <label htmlFor={id} className="pf-label">
        {label}
      </label>
      {children}
      {error ? (
        <span id={`${id}-err`} className="err">
          {error}
        </span>
      ) : hint ? (
        <span id={`${id}-hint`} className="cap">
          {hint}
        </span>
      ) : null}
    </div>
  );
}

function described(id: string, error?: string): string {
  return error ? `${id}-err` : `${id}-hint`;
}

type PrizeInputsProps = {
  idPrefix: string;
  fields: PrizeFields;
  onChange: (patch: Partial<PrizeFields>) => void;
  currency: Currency;
  unit: string;
  phone: boolean;
  promoOff: boolean;
  error?: string;
};

function PrizeInputs({ idPrefix, fields, onChange, currency, unit, phone, promoOff, error }: PrizeInputsProps) {
  const kindId = `${idPrefix}-kind`;
  const valueId = `${idPrefix}-value`;
  const validId = `${idPrefix}-valid`;
  const titleId = `${idPrefix}-custom-title`;
  const descId = `${idPrefix}-custom-desc`;
  return (
    <>
      <div className="pf-field ct-kind">
        <span className="pf-label" id={kindId}>
          Что получит победитель
        </span>
        <div role="group" aria-labelledby={kindId} className={phone ? 'pf-psegs' : 'segs'}>
          {PRIZE_KINDS.map(([key, label]) => (
            <button key={key} type="button" aria-pressed={fields.prizeKind === key} onClick={() => onChange({ prizeKind: key })}>
              {label}
            </button>
          ))}
        </div>
        {fields.prizeKind === 'promo' && promoOff ? (
          <span className="err">Промокод сработает при оплате, только если включён модуль «Промокоды».</span>
        ) : null}
      </div>
      <div className="pf-grid pf-three">
        {fields.prizeKind === 'days' ? (
          <Field id={valueId} label="Сколько дней" error={error} hint="Добавятся к действующей подписке победителя">
            <span className="pf-unit">
              <input
                id={valueId}
                className="field num"
                inputMode="numeric"
                value={fields.days}
                onChange={(e) => onChange({ days: e.target.value })}
                maxLength={3}
                aria-invalid={error ? true : undefined}
                aria-describedby={described(valueId, error)}
              />
              <span className="muted">дней</span>
            </span>
          </Field>
        ) : null}
        {fields.prizeKind === 'balance' ? (
          <Field id={valueId} label="Сумма на баланс" error={error} hint="Зачислится победителю автоматически">
            <span className="pf-unit">
              {currency === 'USD' ? <span className="muted">{unit}</span> : null}
              <input
                id={valueId}
                className="field num"
                inputMode={currency === 'USD' ? 'decimal' : 'numeric'}
                value={fields.amount}
                onChange={(e) => onChange({ amount: e.target.value })}
                maxLength={10}
                aria-invalid={error ? true : undefined}
                aria-describedby={described(valueId, error)}
              />
              {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
            </span>
          </Field>
        ) : null}
        {fields.prizeKind === 'promo' ? (
          <>
            <Field id={valueId} label="Скидка" error={error} hint="Личный одноразовый код для каждого победителя">
              <span className="pf-unit">
                <input
                  id={valueId}
                  className="field num"
                  inputMode="numeric"
                  value={fields.percent}
                  onChange={(e) => onChange({ percent: e.target.value })}
                  maxLength={3}
                  aria-invalid={error ? true : undefined}
                  aria-describedby={described(valueId, error)}
                />
                <span className="muted">%</span>
              </span>
            </Field>
            <Field id={validId} label="Код действует" hint="Дней с момента выдачи">
              <span className="pf-unit">
                <input id={validId} className="field num" inputMode="numeric" value={fields.validDays} onChange={(e) => onChange({ validDays: e.target.value })} maxLength={3} />
                <span className="muted">дней</span>
              </span>
            </Field>
          </>
        ) : null}
        {fields.prizeKind === 'custom' ? (
          <>
            <Field id={titleId} label="Название приза" error={error} hint="Например, Telegram Premium на год">
              <input
                id={titleId}
                className="field"
                value={fields.customTitle}
                onChange={(e) => onChange({ customTitle: e.target.value })}
                maxLength={100}
                aria-invalid={error ? true : undefined}
                aria-describedby={described(titleId, error)}
              />
            </Field>
            <Field id={descId} label="Инструкция по получению" hint="Необязательно. Победитель увидит её вместе с призом" className="ct-wide">
              <textarea
                id={descId}
                className="field ct-area"
                rows={2}
                value={fields.customDescription}
                onChange={(e) => onChange({ customDescription: e.target.value })}
                maxLength={500}
              />
            </Field>
          </>
        ) : null}
      </div>
    </>
  );
}

export function ContestForm({ phone, currency, options, initial, contest, onCancel, onSaved }: Props) {
  const base = useId();
  const [draft, setDraft] = useState<Draft>(initial);
  const [errors, setErrors] = useState<DraftErrors>({});
  const [failure, setFailure] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const locked = contest !== null && !contest.editable.all;

  const set = <K extends keyof Draft>(key: K, value: Draft[K]) => {
    setDraft((d) => ({ ...d, [key]: value }));
    setErrors((e) => ({ ...e, [key === 'days' || key === 'amount' || key === 'percent' || key === 'validDays' ? 'value' : key]: undefined }));
  };

  const setMessage = (key: string, patch: Partial<MessageDraft>) => {
    setDraft((d) => ({ ...d, messages: (d.messages ?? []).map((m) => (m.key === key ? { ...m, ...patch } : m)) }));
    setErrors((e) => ({ ...e, messages: undefined }));
  };

  const addMessage = () => {
    setDraft((d) => ({
      ...d,
      messages: [...(d.messages ?? []), { key: messageKey(), id: null, audience: 'participants', onlyEligible: false, sendAt: '', text: '', locked: false }],
    }));
  };

  const setTier = (key: string, patch: Partial<TierDraft>) => {
    setDraft((d) => ({ ...d, tiers: d.tiers.map((t) => (t.key === key ? { ...t, ...patch } : t)) }));
    setErrors((e) => ({ ...e, tiers: undefined }));
  };

  const addTier = () => {
    setDraft((d) => {
      const last = d.tiers[d.tiers.length - 1];
      const from = last ? (Number.parseInt(last.toPlace, 10) || 0) + 1 : 1;
      return {
        ...d,
        prizeMode: 'tiered',
        tiers: [
          ...d.tiers,
          {
            key: messageKey(),
            fromPlace: String(from),
            toPlace: String(from),
            prizeKind: 'days',
            days: '30',
            amount: '',
            percent: '20',
            validDays: '14',
            customTitle: '',
            customDescription: '',
          },
        ],
      };
    });
    setErrors((e) => ({ ...e, tiers: undefined }));
  };

  const removeTier = (key: string) => {
    setDraft((d) => ({ ...d, tiers: d.tiers.filter((t) => t.key !== key) }));
    setErrors((e) => ({ ...e, tiers: undefined }));
  };

  const submit = async () => {
    const built = buildBody(draft, currency, options);
    const found = Object.values(built.errors).some(Boolean);
    setErrors(built.errors);
    setFailure(found ? 'Исправьте отмеченные поля' : null);
    if (found) {
      return;
    }
    setBusy(true);
    try {
      const saved = contest
        ? await api<Contest>(`${BASE}/contests/${contest.id}`, { method: 'PUT', body: built.body })
        : await api<Contest>(`${BASE}/contests`, { method: 'POST', body: built.body });
      onSaved(saved);
    } catch (err) {
      setFailure(failText(err, contest ? 'Изменения не сохранились' : 'Конкурс не создался'));
    } finally {
      setBusy(false);
    }
  };

  const ids = {
    title: `${base}-title`,
    description: `${base}-desc`,
    value: `${base}-value`,
    valid: `${base}-valid`,
    winners: `${base}-winners`,
    starts: `${base}-starts`,
    ends: `${base}-ends`,
    min: `${base}-min`,
    friends: `${base}-friends`,
  };

  const unit = currency === 'RUB' ? '₽' : '$';
  const messages = draft.messages;
  const canAdd = messages !== null && messages.length < (options?.messages_max ?? 6);
  const hasResults = (messages ?? []).some((m) => m.audience === 'results');

  return (
    <form
      className={phone ? 'pf pf-phone ct-form' : 'pf ct-form'}
      noValidate
      onSubmit={(e) => {
        e.preventDefault();
        void submit();
      }}
    >
      <fieldset className="ct-block" disabled={busy}>
        <legend className="ct-legend">Конкурс</legend>
        <div className="pf-grid">
          <Field id={ids.title} label="Название" error={errors.title} hint="Видят клиенты в боте и в сообщениях">
            <input
              id={ids.title}
              className="field"
              value={draft.title}
              onChange={(e) => set('title', e.target.value)}
              maxLength={options?.title_max ?? 100}
              placeholder="Например, Новогодний розыгрыш"
              aria-invalid={errors.title ? true : undefined}
              aria-describedby={described(ids.title, errors.title)}
            />
          </Field>
          <Field id={ids.description} label="Описание" error={errors.description} hint="Необязательно. Пара строк о конкурсе для клиента">
            <textarea
              id={ids.description}
              className="field ct-area"
              rows={2}
              value={draft.description}
              onChange={(e) => set('description', e.target.value)}
              maxLength={options?.description_max ?? 1000}
              aria-describedby={described(ids.description, errors.description)}
            />
          </Field>
        </div>
      </fieldset>

      <fieldset className="ct-block" disabled={busy || locked}>
        <legend className="ct-legend">Приз</legend>
        {locked ? <p className="cap ct-lock">Конкурс уже идёт: приз, победителей и условия менять нельзя — так честнее для участников.</p> : null}
        <div className="pf-field">
          <span className="pf-label" id={`${base}-mode`}>
            Режим призов
          </span>
          <div role="group" aria-labelledby={`${base}-mode`} className={phone ? 'pf-psegs' : 'segs'}>
            <button type="button" aria-pressed={draft.prizeMode === 'single'} onClick={() => set('prizeMode', 'single')}>
              Один приз на всех
            </button>
            <button
              type="button"
              aria-pressed={draft.prizeMode === 'tiered'}
              onClick={() => {
                set('prizeMode', 'tiered');
                if (!draft.tiers.length) {
                  addTier();
                }
              }}
            >
              Призы по местам
            </button>
          </div>
        </div>
        {draft.prizeMode === 'single' ? (
          <>
            <PrizeInputs
              idPrefix={base}
              fields={draft}
              onChange={(patch) => {
                setDraft((d) => ({ ...d, ...patch }));
                setErrors((e) => ({ ...e, value: undefined }));
              }}
              currency={currency}
              unit={unit}
              phone={phone}
              promoOff={!!options && !options.promo_enabled}
              error={errors.value}
            />
            <div className="pf-grid pf-three">
              <Field id={ids.winners} label="Победителей" error={errors.winners} hint={`От 1 до ${options?.winners_max ?? 100}`}>
                <input
                  id={ids.winners}
                  className="field num"
                  inputMode="numeric"
                  value={draft.winners}
                  onChange={(e) => set('winners', e.target.value)}
                  maxLength={3}
                  aria-invalid={errors.winners ? true : undefined}
                  aria-describedby={described(ids.winners, errors.winners)}
                />
              </Field>
            </div>
          </>
        ) : (
          <>
            <p className="cap ct-lead">Каждому призовому диапазону — свой приз. Победителей бот выберет столько, сколько охватывают места.</p>
            {draft.tiers.map((tier, index) => (
              <div key={tier.key} className="ct-tier">
                <div className="ct-tier-top">
                  <span className="ct-tier-n">Призовое место {index + 1}</span>
                  <button type="button" className="ct-remove hov" onClick={() => removeTier(tier.key)} aria-label={`Удалить призовое место ${index + 1}`}>
                    Удалить
                  </button>
                </div>
                <div className="pf-grid pf-three">
                  <Field id={`${tier.key}-from`} label="С места">
                    <input
                      id={`${tier.key}-from`}
                      className="field num"
                      inputMode="numeric"
                      value={tier.fromPlace}
                      onChange={(e) => setTier(tier.key, { fromPlace: e.target.value })}
                      maxLength={3}
                    />
                  </Field>
                  <Field id={`${tier.key}-to`} label="По место">
                    <input
                      id={`${tier.key}-to`}
                      className="field num"
                      inputMode="numeric"
                      value={tier.toPlace}
                      onChange={(e) => setTier(tier.key, { toPlace: e.target.value })}
                      maxLength={3}
                    />
                  </Field>
                  <div className="pf-field">
                    <span className="pf-label">Победителей в диапазоне</span>
                    <span className="ct-tier-count">
                      {Math.max(0, (Number.parseInt(tier.toPlace, 10) || 0) - (Number.parseInt(tier.fromPlace, 10) || 0) + 1)}
                    </span>
                  </div>
                </div>
                <PrizeInputs
                  idPrefix={tier.key}
                  fields={tier}
                  onChange={(patch) => setTier(tier.key, patch)}
                  currency={currency}
                  unit={unit}
                  phone={phone}
                  promoOff={!!options && !options.promo_enabled}
                />
              </div>
            ))}
            {errors.tiers ? <span className="err">{errors.tiers}</span> : null}
            <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={addTier}>
              + Добавить призовое место
            </button>
            <p className="cap">
              Победителей: {draft.tiers.reduce((max, tier) => Math.max(max, Number.parseInt(tier.toPlace, 10) || 0), 0)} — столько мест охватывают призовые диапазоны.
            </p>
          </>
        )}
      </fieldset>

      <fieldset className="ct-block" disabled={busy}>
        <legend className="ct-legend">Сроки</legend>
        <div className="pf-grid pf-three">
          {contest ? (
            <div className="pf-field">
              <span className="pf-label">Начало</span>
              <input className="field" type="datetime-local" value={draft.startsAt} disabled={locked} onChange={(e) => set('startsAt', e.target.value)} aria-label="Начало" />
              <span className="cap">{locked ? 'Конкурс уже начался' : 'Когда начнётся приём участников'}</span>
            </div>
          ) : (
            <Field id={ids.starts} label="Начало" error={errors.startsAt} hint={draft.startNow ? 'Приём участников начнётся сразу' : 'Когда начнётся приём участников'}>
              <span className="ct-start">
                <label className="ct-inline-check">
                  <input type="checkbox" checked={draft.startNow} onChange={(e) => set('startNow', e.target.checked)} />
                  <span>Сразу</span>
                </label>
                {draft.startNow ? null : (
                  <input
                    id={ids.starts}
                    className="field"
                    type="datetime-local"
                    value={draft.startsAt}
                    onChange={(e) => set('startsAt', e.target.value)}
                    aria-invalid={errors.startsAt ? true : undefined}
                    aria-describedby={described(ids.starts, errors.startsAt)}
                  />
                )}
              </span>
            </Field>
          )}
          <Field id={ids.ends} label="Итоги" error={errors.endsAt} hint={locked ? 'Итоги можно только отложить' : 'В это время бот выберет победителей и выдаст призы'}>
            <input
              id={ids.ends}
              className="field"
              type="datetime-local"
              value={draft.endsAt}
              onChange={(e) => set('endsAt', e.target.value)}
              aria-invalid={errors.endsAt ? true : undefined}
              aria-describedby={described(ids.ends, errors.endsAt)}
            />
          </Field>
        </div>
      </fieldset>

      <fieldset className="ct-block" disabled={busy || locked}>
        <legend className="ct-legend">Условия участия</legend>
        <p className="cap ct-lead">Проверяются по данным проекта — подпискам, оплатам и приглашениям, тариф не важен. Без условий участвовать может любой клиент бота. Перед жеребьёвкой условия проверяются ещё раз.</p>
        <label className="pf-check">
          <input type="checkbox" checked={draft.subscription} onChange={(e) => set('subscription', e.target.checked)} />
          <span>
            Действующая подписка
            <span className="cap">Подписка должна действовать и при вступлении, и в момент итогов</span>
          </span>
        </label>
        {draft.subscription ? (
          <label className="pf-check ct-indent">
            <input type="checkbox" checked={draft.paidOnly} onChange={(e) => set('paidOnly', e.target.checked)} />
            <span>
              Только купленная или подаренная
              <span className="cap">Пробная, по промо-ссылке и выданная вручную не считаются</span>
            </span>
          </label>
        ) : null}
        <label className="pf-check">
          <input type="checkbox" checked={draft.purchase} onChange={(e) => set('purchase', e.target.checked)} />
          <span>
            Оплата во время конкурса
            <span className="cap">Подписка, продление или доп. опции, оплаченные между началом и итогами. Возвращённые оплаты не считаются</span>
          </span>
        </label>
        {draft.purchase ? (
          <div className="ct-indent ct-row">
            <Field id={ids.min} label="Сумма оплаты от" error={errors.purchaseMin} hint="Пусто — любая сумма">
              <span className="pf-unit">
                {currency === 'USD' ? <span className="muted">{unit}</span> : null}
                <input
                  id={ids.min}
                  className="field num"
                  inputMode={currency === 'USD' ? 'decimal' : 'numeric'}
                  value={draft.purchaseMin}
                  onChange={(e) => set('purchaseMin', e.target.value)}
                  maxLength={10}
                  aria-invalid={errors.purchaseMin ? true : undefined}
                  aria-describedby={described(ids.min, errors.purchaseMin)}
                />
                {currency === 'RUB' ? <span className="muted">{unit}</span> : null}
              </span>
            </Field>
          </div>
        ) : null}
        <div className="ct-row">
          <Field
            id={ids.friends}
            label="Пригласить друзей"
            error={errors.friends}
            hint={options && !options.referral_enabled ? 'Модуль «Рефералы» выключен — приглашать будет нечем' : 'Сколько друзей пригласить за время конкурса. Пусто — не нужно'}
          >
            <input
              id={ids.friends}
              className="field num ct-short"
              inputMode="numeric"
              value={draft.friends}
              onChange={(e) => set('friends', e.target.value)}
              maxLength={3}
              placeholder="0"
              aria-invalid={errors.friends ? true : undefined}
              aria-describedby={described(ids.friends, errors.friends)}
            />
          </Field>
          {draft.friends.trim() && draft.friends.trim() !== '0' ? (
            <label className="pf-check ct-friend-paid">
              <input type="checkbox" checked={draft.friendsPaid} onChange={(e) => set('friendsPaid', e.target.checked)} />
              <span>
                Друг должен оплатить подписку
                <span className="cap">Защищает от накрутки пустыми аккаунтами</span>
              </span>
            </label>
          ) : null}
        </div>
      </fieldset>

      {messages !== null ? (
        <fieldset className="ct-block" disabled={busy}>
          <legend className="ct-legend">Сообщения</legend>
          <p className="cap ct-lead">
            Уходят через рассылки в бот, с кнопкой конкурса. Подстановки:{' '}
            {PLACEHOLDERS.map(([code, hint], i) => (
              <span key={code}>
                <code>{code}</code> — {hint}
                {i < PLACEHOLDERS.length - 1 ? ', ' : ''}
              </span>
            ))}
            . Победители получают отдельное сообщение со своим призом.
          </p>
          {messages.map((m, index) => (
            <div key={m.key} className={m.locked ? 'ct-msg ct-msg-locked' : 'ct-msg'}>
              <div className="ct-msg-top">
                <span className="ct-msg-n">Сообщение {index + 1}</span>
                {m.locked ? <span className="cap">Уже отправлено, не меняется</span> : null}
                {!m.locked ? (
                  <button
                    type="button"
                    className="ct-remove hov"
                    onClick={() => setDraft((d) => ({ ...d, messages: (d.messages ?? []).filter((x) => x.key !== m.key) }))}
                    aria-label={`Удалить сообщение ${index + 1}`}
                  >
                    Удалить
                  </button>
                ) : null}
              </div>
              <div className="ct-msg-grid">
                <div className="pf-field">
                  <span className="pf-label">Кому</span>
                  <select
                    className="field"
                    value={m.audience}
                    disabled={m.locked}
                    onChange={(e) => setMessage(m.key, { audience: e.target.value as Audience })}
                    aria-label={`Кому сообщение ${index + 1}`}
                  >
                    {AUDIENCES.filter(([key]) => key !== 'results' || m.audience === 'results' || !hasResults).map(([key, label]) => (
                      <option key={key} value={key}>
                        {label}
                      </option>
                    ))}
                  </select>
                  {m.audience === 'outsiders' ? (
                    <label className="ct-inline-check">
                      <input type="checkbox" checked={m.onlyEligible} disabled={m.locked} onChange={(e) => setMessage(m.key, { onlyEligible: e.target.checked })} />
                      <span>Только тем, кто уже подходит по условиям</span>
                    </label>
                  ) : null}
                </div>
                <div className="pf-field">
                  <span className="pf-label">Когда</span>
                  {m.audience === 'results' ? (
                    <span className="ct-when">Сразу после жеребьёвки</span>
                  ) : (
                    <input
                      className="field"
                      type="datetime-local"
                      value={m.sendAt}
                      disabled={m.locked}
                      onChange={(e) => setMessage(m.key, { sendAt: e.target.value })}
                      aria-label={`Когда отправить сообщение ${index + 1}`}
                    />
                  )}
                </div>
              </div>
              <textarea
                className="field ct-area"
                rows={phone ? 4 : 3}
                value={m.text}
                disabled={m.locked}
                onChange={(e) => setMessage(m.key, { text: e.target.value })}
                aria-label={`Текст сообщения ${index + 1}`}
              />
            </div>
          ))}
          {errors.messages ? <span className="err">{errors.messages}</span> : null}
          {canAdd ? (
            <button type="button" className={phone ? 'pf-quiet press' : 'b40 hov'} onClick={addMessage}>
              Добавить сообщение
            </button>
          ) : null}
        </fieldset>
      ) : (
        <p className="cap ct-lead">
          После создания появятся сообщения: объявление на старте, напоминание за день до итогов тем, кто не участвует, напоминание участникам за три часа и итоги. Их можно поправить на странице конкурса.
        </p>
      )}

      <div className="pf-actions">
        <button type="submit" className={phone ? 'np-ok press' : 'btn-main btn'} disabled={busy}>
          {busy ? 'Сохраняем…' : contest ? 'Сохранить' : 'Создать конкурс'}
        </button>
        <button type="button" className={phone ? 'np-cancel press' : 'btn-line hov'} onClick={onCancel} disabled={busy}>
          Отменить
        </button>
        {failure ? (
          <span className="err pf-fail" role="alert">
            {failure}
          </span>
        ) : null}
      </div>
    </form>
  );
}
