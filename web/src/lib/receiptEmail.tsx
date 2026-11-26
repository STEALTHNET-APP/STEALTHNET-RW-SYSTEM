import { useState } from 'react';
import { ApiError } from './api';
import { t } from './i18n';

const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@.]+$/;

type Needy = { needs_email?: boolean } | null | undefined;
type Owner = { email?: string | null; receipt_email?: string | null };

export function wantsEmail(method: Needy, me: Owner, forced: boolean): boolean {
  return !me.email && (forced || !!method?.needs_email);
}

export function emailProblem(value: string): string | null {
  const v = value.trim();
  if (!v) {
    return t('Укажите почту: на неё придёт чек');
  }
  return EMAIL.test(v) ? null : t('Похоже, в почте опечатка: нужен адрес вида name@example.com');
}

export function emailRefused(err: unknown): boolean {
  return err instanceof ApiError && (err.code === 'payment.email_required' || err.code === 'receipt.email_invalid');
}

export function useReceiptEmail(me: Owner) {
  const [email, setEmail] = useState(me.receipt_email ?? '');
  const [forced, setForced] = useState(false);
  const [error, setError] = useState<string | null>(null);
  return {
    email,
    error,
    change: (value: string) => {
      setEmail(value);
      setError(null);
    },
    asks: (method: Needy) => wantsEmail(method, me, forced),
    take: (method: Needy): string | null | false => {
      if (!wantsEmail(method, me, forced)) {
        return null;
      }
      const problem = emailProblem(email);
      if (problem) {
        setError(problem);
        return false;
      }
      return email.trim();
    },
    refused: (err: unknown): boolean => {
      if (!emailRefused(err) || !(err instanceof ApiError)) {
        return false;
      }
      setForced(true);
      setError(err.code === 'receipt.email_invalid' ? err.message : t('Укажите почту: на неё придёт чек'));
      return true;
    },
  };
}

type Props = {
  value: string;
  onChange: (value: string) => void;
  error: string | null;
  disabled: boolean;
  mini?: boolean;
};

export function ReceiptEmail({ value, onChange, error, disabled, mini }: Props) {
  return (
    <label className={mini ? 'mn-receipt' : 'cb-receipt'}>
      <span className={mini ? 'mn-receipt-label' : 'cb-receipt-label'}>{t('Почта для чека')}</span>
      <input
        type="email"
        inputMode="email"
        autoComplete="email"
        autoCapitalize="off"
        spellCheck={false}
        value={value}
        onChange={(e) => onChange(e.target.value)}
        placeholder="name@example.com"
        maxLength={254}
        disabled={disabled}
        aria-invalid={error ? true : undefined}
      />
      <span className={mini ? 'mn-receipt-hint' : 'cb-small cb-muted'}>{error ?? t('На неё придут чек и письма об оплате')}</span>
    </label>
  );
}
