import { useCallback, useEffect, useRef, useState } from 'react';
import { api, ApiError } from '../lib/api';
import { timeHM } from '../lib/format';
import { t } from '../lib/i18n';
import { Icon } from '../lib/icons';
import { QrCode } from '../lib/qr';
import { useCabinet } from './CabinetApp';

type Start = { poll_token: string; link: string; expires_at: string };
type Status = { status: 'pending' | 'rejected' | 'expired' } | { status: 'linked'; account: string };

const PATH = '/api/client/security/logins/telegram';
const PAUSE = 2500;

function passing(err: unknown): boolean {
  return !(err instanceof ApiError) || err.status === 0 || err.status === 429 || err.status >= 500;
}

export function TelegramLink({ onLinked, onStart }: { onLinked: (account: string) => void; onStart: () => void }) {
  const { phone } = useCabinet();
  const [wait, setWait] = useState<Start | null>(null);
  const [over, setOver] = useState<'rejected' | 'expired' | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const current = useRef<string | null>(null);
  const timer = useRef<number | null>(null);

  const settle = useCallback(() => {
    current.current = null;
    if (timer.current !== null) {
      window.clearTimeout(timer.current);
      timer.current = null;
    }
    setWait(null);
  }, []);

  useEffect(
    () => () => {
      current.current = null;
      if (timer.current !== null) {
        window.clearTimeout(timer.current);
      }
    },
    [],
  );

  const poll = (start: Start) => {
    const again = () => {
      if (current.current === start.poll_token) {
        poll(start);
      }
    };
    timer.current = window.setTimeout(async () => {
      try {
        const res = await api<Status>(`${PATH}/status`, { method: 'POST', body: { poll_token: start.poll_token } });
        if (current.current !== start.poll_token) {
          return;
        }
        if (res.status === 'pending') {
          again();
        } else if (res.status === 'linked') {
          settle();
          onLinked(res.account);
        } else {
          settle();
          setOver(res.status);
        }
      } catch (err) {
        if (current.current !== start.poll_token) {
          return;
        }
        if (!passing(err)) {
          settle();
          setError(err instanceof ApiError ? err.message : t('Привязка не завершилась, попробуйте ещё раз'));
        } else if (Date.now() > Date.parse(start.expires_at) + 30000) {
          settle();
          setOver('expired');
        } else {
          again();
        }
      }
    }, PAUSE);
  };

  const begin = async () => {
    setBusy(true);
    setError(null);
    setOver(null);
    onStart();
    try {
      const start = await api<Start>(PATH, { method: 'POST', body: {} });
      current.current = start.poll_token;
      setWait(start);
      poll(start);
    } catch (err) {
      setError(err instanceof ApiError ? err.message : t('Привязка не началась'));
    } finally {
      setBusy(false);
    }
  };

  const stop = () => {
    const token = current.current;
    settle();
    if (token) {
      void api(`${PATH}/cancel`, { method: 'POST', body: { poll_token: token } }).catch(() => undefined);
    }
  };

  if (wait) {
    return (
      <div className="cb-tglink">
        <p role="status">{t('Откройте бота и нажмите «Привязать». Страница обновится сама.')}</p>
        <span className="cb-btns">
          <a className="cb-main" href={wait.link} target="_blank" rel="noreferrer">
            {t('Открыть Telegram')}
          </a>
          <button type="button" className="cb-cancel" onClick={stop}>
            {t('Отменить')}
          </button>
        </span>
        {phone ? null : (
          <>
            <QrCode text={wait.link} size={168} label={t('QR-код для привязки Telegram')} />
            <span className="cb-small cb-muted">{t('Если Telegram только на телефоне, отсканируйте код.')}</span>
          </>
        )}
        <span className="cb-small cb-muted">{t('Ссылка действует до {time}.', { time: timeHM(wait.expires_at) })}</span>
      </div>
    );
  }

  return (
    <div className="cb-tglink">
      <p className="cb-note">{t('С Telegram можно входить без почты и пароля, а подписка откроется и в боте.')}</p>
      {over === 'rejected' ? <p className="cb-note">{t('Привязка отменена в боте.')}</p> : null}
      {over === 'expired' ? <p className="cb-note">{t('Время на подтверждение вышло, начните заново.')}</p> : null}
      {error ? (
        <p className="cb-error" role="alert">
          {error}
        </p>
      ) : null}
      <span className="cb-btns">
        <button type="button" className="cb-second cb-oauth-btn" onClick={() => void begin()} disabled={busy}>
          <Icon name="send" size={20} />
          {t('Привязать {provider}', { provider: 'Telegram' })}
        </button>
      </span>
    </div>
  );
}
