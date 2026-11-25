import { useCallback, useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import type { ModuleConfig, ModulesData } from '../modules/model';
import { Switch } from '../ui';
import '../form.css';
import './admins.css';
import { SettingsLayout } from './SettingsLayout';
import { TelegramButtons } from './TelegramButtons';

type Access = { miniapp: boolean; miniapp_fullscreen: boolean; menu_button: boolean; site_login: boolean; self_devices: boolean; self_reissue: boolean };
type Core = { settings?: { public_url?: string | null } };

const LOGIN_TEXT: Record<string, string> = {
  password: 'по почте и паролю',
  password_confirm: 'по почте и паролю, при регистрации почта подтверждается кодом из письма',
  password_closed: 'по почте и паролю; регистрация по почте закрыта, пока письма не отправляются',
  code: 'по почте, кодом из письма',
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

function Toggle({ title, text, on, busy, onToggle }: { title: string; text: string; on: boolean; busy: boolean; onToggle: () => void }) {
  return (
    <div className="row adm-row">
      <span className="adm-who">
        <span className="adm-email">{title}</span>
        <span className="adm-status">{text}</span>
      </span>
      <span className="adm-acts">
        <Switch on={on} onToggle={onToggle} label={title} disabled={busy} />
      </span>
    </div>
  );
}

export function SettingsClientAccess() {
  const [access, setAccess] = useState<Access | null>(null);
  const [publicUrl, setPublicUrl] = useState<string | null>(null);
  const [mailMode, setMailMode] = useState<string>('password');
  const [oauthOn, setOauthOn] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [turn, setTurn] = useState(0);

  const load = useCallback(async () => {
    try {
      const [next, core, mail, modules] = await Promise.all([
        api<Access>('/api/admin/settings/client_access'),
        api<Core>('/api/admin/settings/core'),
        api<ModuleConfig>('/api/admin/modules/mail/config').catch(() => null),
        api<ModulesData>('/api/admin/modules').catch(() => null),
      ]);
      setAccess(next);
      setPublicUrl(core.settings?.public_url ?? null);
      const mailOn = modules?.modules.find((m) => m.info.id === 'mail')?.active ?? false;
      const transport = String(mail?.config.transport ?? 'none');
      const login = mail?.config.login;
      const sends = mailOn && transport !== 'none';
      const confirm = mail?.config.confirm_signup === true && login === 'password';
      setMailMode(sends && login !== 'password' ? 'code' : confirm ? (sends ? 'password_confirm' : 'password_closed') : 'password');
      setOauthOn(modules?.modules.find((m) => m.info.id === 'oauth')?.active ?? false);
      setError(null);
    } catch (err) {
      setError(failText(err, 'Настройки не загрузились'));
    }
  }, []);

  useEffect(() => {
    void load();
  }, [load]);

  const flip = async (key: keyof Access) => {
    if (!access || busy) {
      return;
    }
    setBusy(true);
    setError(null);
    try {
      setAccess(await api<Access>('/api/admin/settings/client_access', { method: 'PUT', body: { ...access, [key]: !access[key] } }));
      if (key === 'miniapp' || key === 'menu_button') {
        setTurn((value) => value + 1);
      }
    } catch (err) {
      setError(failText(err, 'Не сохранилось'));
    } finally {
      setBusy(false);
    }
  };

  if (!access) {
    return (
      <SettingsLayout>
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : (
          <p className="muted">Загружаем…</p>
        )}
      </SettingsLayout>
    );
  }

  const base = (publicUrl ?? '').trim().replace(/\/+$/, '');
  const httpsReady = base.startsWith('https://');
  const appUrl = `${base}/app`;
  const miniText = !access.miniapp
    ? 'Выключено: в боте нет кнопок приложения, а открыть его по ссылке нельзя.'
    : httpsReady
      ? `Кнопка «Открыть приложение» в меню бота ведёт на ${appUrl}`
      : 'Включено, но у сайта нет адреса с https: без него Telegram не откроет приложение.';

  return (
    <SettingsLayout>
      <div className="sec-stack">
        {error ? (
          <p className="err" role="alert">
            {error}
          </p>
        ) : null}
        <section className="sec" aria-labelledby="mini-h">
          <div className="sec-head">
            <h2 id="mini-h" className="section-title">
              Мини-приложение
            </h2>
            <span className="muted">Открывается только внутри Telegram, из бота. Вход без пароля: Telegram сам подтверждает клиента.</span>
          </div>
          <Toggle title="Мини-приложение в Telegram" text={miniText} on={access.miniapp} busy={busy} onToggle={() => void flip('miniapp')} />
          {access.miniapp ? (
            <Toggle
              title="Открывать на весь экран"
              text={
                access.miniapp_fullscreen
                  ? 'На телефоне приложение само просит полный экран при каждом входе, нужен Telegram с ноября 2024 года или новее. На компьютере — как выбрано в @BotFather.'
                  : 'Выключено: приложение открывается так, как выбрано в @BotFather.'
              }
              on={access.miniapp_fullscreen}
              busy={busy}
              onToggle={() => void flip('miniapp_fullscreen')}
            />
          ) : null}
          {access.miniapp ? (
            <Toggle
              title="Кнопка рядом с полем ввода"
              text={
                access.menu_button
                  ? 'Открывает приложение прямо из чата с ботом.'
                  : 'Выключена: на её месте обычное «Меню» с командами. Приложение открывается кнопкой в меню бота.'
              }
              on={access.menu_button}
              busy={busy}
              onToggle={() => void flip('menu_button')}
            />
          ) : null}
          <TelegramButtons turn={turn} miniapp={access.miniapp} appUrl={httpsReady ? appUrl : null} />
          <div className="row">
            <span className="cap">
              Баннеры, порядок блоков, меню внизу и профиль настраиваются в{' '}
              <Link className="link" to="/admin/settings/miniapp">
                «Мини-приложении»
              </Link>
              .
            </span>
          </div>
          {access.miniapp && !httpsReady ? (
            <div className="row">
              <span className="cap">
                Адрес сайта задаётся на сервере командой <code className="mono">stealthnet domain</code>, текущий виден в{' '}
                <Link className="link" to="/admin/settings/general">
                  «Общих»
                </Link>
                .
              </span>
            </div>
          ) : null}
        </section>
        <section className="sec" aria-labelledby="site-h">
          <div className="sec-head">
            <h2 id="site-h" className="section-title">
              Вход на сайт
            </h2>
            <span className="muted">Кабинет на сайте: подписка, оплата и поддержка в браузере.</span>
          </div>
          <Toggle
            title="Пускать в кабинет на сайте"
            text={access.site_login ? 'Клиенты входят на сайте. Уже открытые сессии работают.' : 'Выключено: вход на сайте закрыт, открытые сессии кабинета завершены.'}
            on={access.site_login}
            busy={busy}
            onToggle={() => void flip('site_login')}
          />
          {access.site_login ? (
            <div className="row">
              <span className="rc">
                <span className="cap">Через Telegram: бот присылает запрос на вход, клиент подтверждает его.</span>
                <span className="cap">
                  По почте: {LOGIN_TEXT[mailMode] ?? LOGIN_TEXT.password}.{' '}
                  <Link className="link" to="/admin/settings/mail">
                    Изменить
                  </Link>
                </span>
                <span className="cap">
                  Через Google и Apple: {oauthOn ? 'включено' : 'выключено'}.{' '}
                  <Link className="link" to="/admin/modules/oauth">
                    {oauthOn ? 'Настроить' : 'Включить'}
                  </Link>
                </span>
              </span>
            </div>
          ) : null}
          <div className="row">
            <span className="cap">
              Свой CSS кабинета настраивается в{' '}
              <Link className="link" to="/admin/settings/cabinet">
                «Кабинете на сайте»
              </Link>
              .
            </span>
          </div>
        </section>
        <section className="sec" aria-labelledby="self-h">
          <div className="sec-head">
            <h2 id="self-h" className="section-title">
              Что клиент может сам
            </h2>
            <span className="muted">Выключенное действие пропадает в боте, кабинете и мини-приложении. Сделать его сможет только поддержка из админки.</span>
          </div>
          <Toggle
            title="Отключать свои устройства"
            text="Освободить место под новое устройство без обращения в поддержку."
            on={access.self_devices}
            busy={busy}
            onToggle={() => void flip('self_devices')}
          />
          <Toggle
            title="Выпускать новую ссылку подписки"
            text="Старая ссылка перестаёт работать, все устройства подключаются заново."
            on={access.self_reissue}
            busy={busy}
            onToggle={() => void flip('self_reissue')}
          />
        </section>
      </div>
    </SettingsLayout>
  );
}
