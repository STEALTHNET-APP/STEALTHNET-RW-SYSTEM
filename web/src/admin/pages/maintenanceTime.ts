function two(n: number): string {
  return String(n).padStart(2, '0');
}

export function localInput(iso: string | null): string {
  if (!iso) {
    return '';
  }
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return `${date.getFullYear()}-${two(date.getMonth() + 1)}-${two(date.getDate())}T${two(date.getHours())}:${two(date.getMinutes())}`;
}

export function withOffset(local: string): string | null {
  const date = new Date(local);
  if (!local || Number.isNaN(date.getTime())) {
    return null;
  }
  const offset = -date.getTimezoneOffset();
  const sign = offset < 0 ? '-' : '+';
  const abs = Math.abs(offset);
  return `${local.slice(0, 16)}:00${sign}${two(Math.floor(abs / 60))}:${two(abs % 60)}`;
}

export function zoneLabel(local: string): string {
  const date = local ? new Date(local) : new Date();
  const offset = Number.isNaN(date.getTime()) ? -new Date().getTimezoneOffset() : -date.getTimezoneOffset();
  if (offset === 0) {
    return 'UTC';
  }
  const abs = Math.abs(offset);
  const minutes = abs % 60;
  return `UTC${offset < 0 ? '−' : '+'}${Math.floor(abs / 60)}${minutes ? `:${two(minutes)}` : ''}`;
}
