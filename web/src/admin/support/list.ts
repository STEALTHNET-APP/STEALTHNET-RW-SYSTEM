export type Filter = 'waiting' | 'answered' | 'closed';

export type Cursor = { key: number; id: number };

export type Paged<T> = { items: T[]; next: number | null; next_id?: number | null };

type Keyed = { ticket: { id: number } };

export const PAGE = 50;
export const PAGE_MAX = 200;
export const PICK_MAX = 500;
export const DAYS_MAX = 3650;

export function readFilter(value: string | null): Filter {
  return value === 'answered' || value === 'closed' ? value : 'waiting';
}

export function cursorOf(page: { next: number | null; next_id?: number | null }): Cursor | null {
  return typeof page.next === 'number' && typeof page.next_id === 'number' ? { key: page.next, id: page.next_id } : null;
}

export function listPath(filter: Filter, query: string, after: Cursor | null, limit: number): string {
  const search = new URLSearchParams({ filter, limit: String(Math.min(Math.max(limit, 1), PAGE_MAX)) });
  if (query) {
    search.set('q', query);
  }
  if (after) {
    search.set('after', String(after.key));
    search.set('after_id', String(after.id));
  }
  return `/api/admin/support/tickets?${search.toString()}`;
}

export function appendPage<T extends Keyed>(current: T[], incoming: T[]): T[] {
  const seen = new Set(current.map((item) => item.ticket.id));
  return current.concat(incoming.filter((item) => !seen.has(item.ticket.id)));
}

export async function collect<T extends Keyed, P extends Paged<T>>(
  fetchPage: (after: Cursor | null, limit: number) => Promise<P>,
  want: number,
): Promise<{ items: T[]; next: Cursor | null; last: P }> {
  let items: T[] = [];
  let after: Cursor | null = null;
  let last: P;
  do {
    last = await fetchPage(after, Math.min(PAGE_MAX, Math.max(want - items.length, 1)));
    items = appendPage(items, last.items);
    after = cursorOf(last);
  } while (after && items.length < want);
  return { items, next: after, last };
}

export function readDays(text: string): number | null {
  if (!/^\d{1,4}$/.test(text.trim())) {
    return null;
  }
  const value = Number(text.trim());
  return value >= 1 && value <= DAYS_MAX ? value : null;
}
