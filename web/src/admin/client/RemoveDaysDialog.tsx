import { useId, useState } from 'react';
import { dateLong, days } from '../../lib/format';
import { Dialog } from './dialogs';
import { removedUntil } from './removeDays';

const PRESETS = [1, 7, 30];

type Props = {
  until: string | null;
  onRemove: (days: number) => Promise<void>;
  onClose: () => void;
};

export function RemoveDaysDialog({ until, onRemove, onClose }: Props) {
  const [choice, setChoice] = useState<number | 'custom'>(7);
  const [custom, setCustom] = useState('');
  const [busy, setBusy] = useState(false);
  const daysField = useId();
  const customDays = Number(custom);
  const customValid = Number.isInteger(customDays) && customDays >= 1 && customDays <= 3650;
  const chosen = choice === 'custom' ? (customValid ? customDays : 0) : choice;
  const end = until ? Date.parse(until) : Number.NaN;
  const ended = !Number.isFinite(end) || end <= Date.now();
  const result = removedUntil(until, chosen, Date.now());

  const note = ended
    ? 'Подписка уже закончилась, убирать нечего.'
    : !result
      ? 'Введите число дней от 1 до 3650.'
      : result.ends
        ? 'Подписка закончится сразу: дней осталось меньше.'
        : `Подписка будет действовать до ${dateLong(result.until)}.`;

  const submit = async () => {
    if (!result) {
      return;
    }
    setBusy(true);
    await onRemove(chosen);
    onClose();
  };

  return (
    <Dialog title="Убрать дни" busy={busy} onClose={onClose}>
      <div className="field-row">
        <span style={{ fontWeight: 600 }}>Сколько убрать</span>
        <div role="group" aria-label="Сколько дней убрать" className="segs" style={{ alignSelf: 'flex-start' }}>
          {PRESETS.map((n) => (
            <button key={n} type="button" aria-pressed={choice === n} onClick={() => setChoice(n)} disabled={busy || ended}>
              {days(n)}
            </button>
          ))}
          <button type="button" aria-pressed={choice === 'custom'} onClick={() => setChoice('custom')} disabled={busy || ended}>
            Своё число
          </button>
        </div>
        {choice === 'custom' && !ended ? (
          <span className="unit-field">
            <label htmlFor={daysField} className="sr-only">
              Сколько дней убрать
            </label>
            <input
              id={daysField}
              className="field num"
              inputMode="numeric"
              value={custom}
              onChange={(event) => setCustom(event.target.value)}
              disabled={busy}
              autoFocus
            />
            <span className="muted">дней, до 3650</span>
          </span>
        ) : null}
      </div>
      <p className={result?.ends || ended ? 'err' : 'cap'}>{note}</p>
      <p className="cap">Клиенту сообщение не придёт, изменение попадёт в историю.</p>
      <div className="dialog-actions">
        <button type="button" className="btn-bad" onClick={() => void submit()} disabled={busy || !result}>
          {busy ? 'Убираем…' : result ? `Убрать ${days(chosen)}` : 'Убрать'}
        </button>
        <button type="button" className="btn-line hov" onClick={onClose} disabled={busy}>
          Не убирать
        </button>
      </div>
    </Dialog>
  );
}
