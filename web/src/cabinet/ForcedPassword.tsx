import { useState, type FormEvent } from 'react';
import { api, ApiError } from '../lib/api';
import { t } from '../lib/i18n';

export function ForcedPassword({ onDone, onLogout }: { onDone: () => void; onLogout: () => void }) {
  const [fresh, setFresh] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async (event: FormEvent) => {
    event.preventDefault();
    setBusy(true);
    setError(null);
    try {
      await api('/api/client/security/password', { method: 'POST', body: { password: fresh } });
      onDone();
    } catch (err) {
      setError(err instanceof ApiError ? err.message : t('Пароль не сохранился'));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="cb-solo" data-part="login">
      <form className="cb-solo-box" onSubmit={submit}>
        <h1>{t('Задайте свой пароль')}</h1>
        <p>{t('Вы вошли с паролем от администратора. Он одноразовый: придумайте свой пароль, чтобы открыть кабинет.')}</p>
        <label className="cb-field">
          <span>{t('Новый пароль')}</span>
          <input
            type="password"
            value={fresh}
            onChange={(e) => setFresh(e.target.value)}
            autoComplete="new-password"
            placeholder={t('Не короче 10 символов')}
            autoFocus
          />
        </label>
        {error ? (
          <p className="cb-error" role="alert">
            {error}
          </p>
        ) : null}
        <button type="submit" className="cb-main" disabled={busy || fresh.length < 10}>
          {busy ? t('Сохраняем…') : t('Сохранить пароль')}
        </button>
        <button type="button" className="cb-second" onClick={onLogout} disabled={busy}>
          {t('Выйти')}
        </button>
      </form>
    </div>
  );
}
