import { api } from '../../lib/api';
import { count, dateLong, timeHM } from '../../lib/format';
import { Switch } from '../ui';
import { localInput, withOffset, zoneLabel } from './maintenanceTime';

export type MaintenanceView = {
  settings: {
    enabled: boolean;
    message_ru: string | null;
    message_en: string | null;
    until: string | null;
    offset_minutes: number;
    since: string | null;
  };
  standard: { ru: string; en: string };
  message_max: number;
  notice_minutes: number;
  english: boolean;
  telegram_id: number | null;
  testers: number;
};

export type MaintenanceForm = { enabled: boolean; ru: string; en: string; until: string; telegram: string };

const TELEGRAM = /^\d{1,16}$/;

function chars(text: string): number {
  return [...text.trim()].length;
}

export function maintenanceForm(view: MaintenanceView): MaintenanceForm {
  return {
    enabled: view.settings.enabled,
    ru: view.settings.message_ru ?? '',
    en: view.settings.message_en ?? '',
    until: localInput(view.settings.until),
    telegram: view.telegram_id === null ? '' : String(view.telegram_id),
  };
}

function settingsChanged(form: MaintenanceForm, base: MaintenanceForm): boolean {
  return form.enabled !== base.enabled || form.ru.trim() !== base.ru.trim() || form.en.trim() !== base.en.trim() || form.until !== base.until;
}

function telegramChanged(form: MaintenanceForm, base: MaintenanceForm): boolean {
  return form.telegram.trim() !== base.telegram.trim();
}

export function maintenanceChanged(form: MaintenanceForm, base: MaintenanceForm): boolean {
  return settingsChanged(form, base) || telegramChanged(form, base);
}

export function maintenanceProblem(form: MaintenanceForm, base: MaintenanceForm, view: MaintenanceView): string | null {
  if (chars(form.ru) > view.message_max || chars(form.en) > view.message_max) {
    return `Сообщение — не длиннее ${view.message_max} символов`;
  }
  if (form.until !== base.until && form.until) {
    const at = new Date(form.until);
    if (Number.isNaN(at.getTime())) {
      return 'Не получилось разобрать время окончания';
    }
    if (at.getTime() <= Date.now()) {
      return 'Время окончания уже прошло. Укажите время в будущем или очистите поле';
    }
  }
  if (form.telegram.trim() && !TELEGRAM.test(form.telegram.trim())) {
    return 'Telegram ID — только цифры, например 123456789';
  }
  return null;
}

export async function saveMaintenance(form: MaintenanceForm, base: MaintenanceForm): Promise<boolean> {
  if (settingsChanged(form, base)) {
    await api('/api/admin/settings/maintenance', {
      method: 'PUT',
      body: { enabled: form.enabled, message_ru: form.ru, message_en: form.en, until: form.until ? withOffset(form.until) : null },
    });
  }
  if (telegramChanged(form, base)) {
    const value = form.telegram.trim();
    await api('/api/admin/auth/telegram', { method: 'PUT', body: { telegram_id: value ? Number(value) : null } });
  }
  return form.enabled !== base.enabled;
}

type Props = {
  view: MaintenanceView;
  form: MaintenanceForm;
  onChange: (next: MaintenanceForm) => void;
  busy: boolean;
};

export function MaintenanceSection({ view, form, onChange, busy }: Props) {
  const edit = (patch: Partial<MaintenanceForm>) => onChange({ ...form, ...patch });
  const max = view.message_max;
  const ruLen = chars(form.ru);
  const enLen = chars(form.en);
  const telegramBad = form.telegram.trim() !== '' && !TELEGRAM.test(form.telegram.trim());
  const saved = view.settings;
  const state = saved.enabled
    ? `Включены${saved.since ? ` ${dateLong(saved.since)} в ${timeHM(saved.since)}` : ''}. Бот отвечает клиенту сообщением не чаще раза в ${view.notice_minutes} минут, на кнопки показывает окно с тем же текстом`
    : 'Клиенты пользуются ботом, кабинетом и мини-приложением как обычно';
  return (
    <section className="sec" id="maintenance" aria-labelledby="maint-h">
      <div className="sec-head">
        <h2 id="maint-h" className="section-title">
          Технические работы
        </h2>
        <span className="muted">
          Клиенты видят сообщение вместо бота, кабинета и мини-приложения, на лендинге — строку сверху. Оплаты, автопродление, синхронизация с Remnawave и админка работают как обычно
        </span>
      </div>
      <div className="row">
        <span className="rl">Режим</span>
        <div className="rc">
          <span className="gen-line" style={{ minHeight: 40 }}>
            <Switch on={form.enabled} onToggle={() => edit({ enabled: !form.enabled })} label="Технические работы" describedBy="maint-state" disabled={busy} />
            <span>{form.enabled ? 'Включены' : 'Выключены'}</span>
          </span>
          <span id="maint-state" className="cap">
            {state}
          </span>
        </div>
      </div>
      <div className="row">
        <label htmlFor="maint-ru" className="rl">
          Сообщение
        </label>
        <div className="rc">
          <textarea
            id="maint-ru"
            className="field"
            rows={3}
            maxLength={max + 50}
            value={form.ru}
            placeholder={view.standard.ru}
            aria-invalid={ruLen > max}
            aria-describedby="maint-ru-hint"
            onChange={(e) => edit({ ru: e.target.value })}
            disabled={busy}
          />
          <span id="maint-ru-hint" className="cap">
            <span className={ruLen > max ? 'seo-count over' : 'seo-count'}>
              {ruLen} из {max}
            </span>
            {' · '}Пусто — стандартный текст. Во всплывающем окне бота видно первые 200 символов
          </span>
        </div>
      </div>
      {view.english || form.en.trim() ? (
        <div className="row">
          <label htmlFor="maint-en" className="rl">
            На английском
          </label>
          <div className="rc">
            <textarea
              id="maint-en"
              className="field"
              rows={3}
              maxLength={max + 50}
              value={form.en}
              placeholder={view.standard.en}
              aria-invalid={enLen > max}
              aria-describedby="maint-en-hint"
              onChange={(e) => edit({ en: e.target.value })}
              disabled={busy}
            />
            <span id="maint-en-hint" className="cap">
              <span className={enLen > max ? 'seo-count over' : 'seo-count'}>
                {enLen} из {max}
              </span>
              {' · '}Для клиентов с английским языком. Пусто — стандартный текст
            </span>
          </div>
        </div>
      ) : null}
      <div className="row">
        <label htmlFor="maint-until" className="rl">
          Закончим примерно
        </label>
        <div className="rc">
          <span className="gen-line" style={{ flexWrap: 'wrap' }}>
            <input
              id="maint-until"
              type="datetime-local"
              className="field"
              value={form.until}
              aria-describedby="maint-until-hint"
              onChange={(e) => edit({ until: e.target.value })}
              disabled={busy}
              style={{ maxWidth: 240 }}
            />
            {form.until ? (
              <button type="button" className="btn-line hov" onClick={() => edit({ until: '' })} disabled={busy}>
                Очистить
              </button>
            ) : null}
          </span>
          <span id="maint-until-hint" className="cap">
            Необязательно. На сайте время покажется в часовом поясе клиента, в боте — в вашем, {zoneLabel(form.until)}. Когда время пройдёт, строка пропадёт сама
          </span>
        </div>
      </div>
      <div className="row">
        <label htmlFor="maint-tg" className="rl">
          Ваш Telegram ID
        </label>
        <div className="rc">
          <input
            id="maint-tg"
            className="fld num"
            inputMode="numeric"
            style={{ width: 180 }}
            value={form.telegram}
            placeholder="123456789"
            aria-invalid={telegramBad}
            aria-describedby="maint-tg-hint"
            onChange={(e) => edit({ telegram: e.target.value.replace(/\s/g, '').slice(0, 16) })}
            disabled={busy}
          />
          <span id="maint-tg-hint" className={telegramBad ? 'err' : 'cap'}>
            {telegramBad
              ? 'Только цифры, например 123456789'
              : `Администраторам с указанным Telegram ID бот отвечает как обычно, чтобы проверить работу.${view.testers > 0 ? ` ID указан ещё у ${count(view.testers, 'администратора', 'администраторов', 'администраторов')}` : ''}`}
          </span>
        </div>
      </div>
    </section>
  );
}
