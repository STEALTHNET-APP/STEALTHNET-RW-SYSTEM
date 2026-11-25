import { useEffect, useState } from 'react';
import { Link } from 'react-router';
import { api, ApiError } from '../../lib/api';
import { dateLong } from '../../lib/format';
import { Dialog } from '../client/dialogs';
import { heldText, holderName, type TariffUsage } from './usage';

type Props = {
  tariffId: number;
  onSale: boolean;
  onClose: () => void;
  onDeleted: () => Promise<void>;
  onHidden: () => Promise<void>;
};

function failText(err: unknown, fallback: string): string {
  return err instanceof ApiError ? err.message : fallback;
}

export function DeleteTariff({ tariffId, onSale, onClose, onDeleted, onHidden }: Props) {
  const [usage, setUsage] = useState<TariffUsage | null>(null);
  const [loadFailed, setLoadFailed] = useState(false);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let live = true;
    api<TariffUsage>(`/api/admin/tariffs/${tariffId}/usage`)
      .then((found) => live && setUsage(found))
      .catch(() => live && setLoadFailed(true));
    return () => {
      live = false;
    };
  }, [tariffId]);

  const act = async (run: () => Promise<void>, fallback: string) => {
    setBusy(true);
    setError(null);
    try {
      await run();
    } catch (err) {
      setError(failText(err, fallback));
      setBusy(false);
    }
  };

  const remove = () =>
    act(async () => {
      await api(`/api/admin/tariffs/${tariffId}`, { method: 'DELETE' });
      await onDeleted();
    }, 'Тариф не удалился');

  const hide = () =>
    act(async () => {
      await api(`/api/admin/tariffs/${tariffId}`, { method: 'PATCH', body: { on_sale: false } });
      await onHidden();
    }, 'Не получилось снять с продажи');

  const total = usage ? usage.active + usage.ended : 0;
  const blocked = usage !== null && total > 0;
  const more = usage ? total - usage.holders.length : 0;

  return (
    <Dialog title="Удалить тариф" busy={busy} onClose={onClose}>
      {usage === null && !loadFailed ? <p className="muted">Проверяем, есть ли на тарифе подписки…</p> : null}
      {usage !== null && !blocked ? <p className="muted">Тариф пропадёт из продажи и из списка. Оплаты останутся в отчётах.</p> : null}
      {loadFailed ? <p className="muted">Тариф пропадёт из продажи и из списка. Удалить его можно, только когда на нём нет подписок. Оплаты останутся в отчётах.</p> : null}
      {usage !== null && blocked ? (
        <>
          <p>
            На тарифе {heldText(usage)}. Пока они есть, удалить тариф нельзя: клиенты не смогут продлить подписку.
          </p>
          {usage.holders.length > 0 ? (
            <ul className="tf-holders">
              {usage.holders.map((holder) => (
                <li key={holder.subscription_id}>
                  <Link className="link" to={`/admin/clients/${holder.client_id}`}>
                    {holderName(holder)}
                  </Link>
                  <span className="muted">{holder.active ? (holder.expire_at ? `до ${dateLong(holder.expire_at, false)}` : 'без срока') : `закончилась ${dateLong(holder.expire_at, false)}`}</span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="muted">Список клиентов виден с правом «Клиенты: просмотр».</p>
          )}
          {usage.holders.length > 0 ? (
            <p className="muted tf-all">
              {more > 0 ? `И ещё ${more}. ` : ''}
              <Link className="link" to={`/admin/clients?tariff=${tariffId}`}>
                Все клиенты тарифа в «Клиентах»
              </Link>
            </p>
          ) : null}
          <p className="muted">
            Откройте клиента и нажмите «Сменить тариф» или «Удалить подписку». Чтобы тариф не покупали новые клиенты, снимите его с продажи: нынешние продлят как раньше.
          </p>
        </>
      ) : null}
      {error ? (
        <p className="err" role="alert">
          {error}
        </p>
      ) : null}
      <div className="dialog-actions">
        {blocked ? (
          onSale ? (
            <button type="button" className="btn-main" onClick={() => void hide()} disabled={busy}>
              {busy ? 'Выполняем…' : 'Снять с продажи'}
            </button>
          ) : null
        ) : (
          <button type="button" className="btn-bad" onClick={() => void remove()} disabled={busy || (usage === null && !loadFailed)}>
            {busy ? 'Выполняем…' : 'Удалить тариф'}
          </button>
        )}
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          {blocked ? 'Закрыть' : 'Отменить'}
        </button>
      </div>
    </Dialog>
  );
}
