type Money = { minor: number; currency: string };

export type ReferralPerson = {
  client_id: number;
  name: string | null;
  telegram_username: string | null;
  telegram_id?: string | null;
  email?: string | null;
};

export type Inviter = {
  client: ReferralPerson;
  blocked: boolean;
  deleted: boolean;
  linked_at: string;
  source: string | null;
  code: string | null;
  manual: { at: string; by: string | null } | null;
};

export type InvitedRow = {
  client: ReferralPerson;
  joined_at: string;
  manual_at: string | null;
  blocked: boolean;
  deleted: boolean;
  paid?: boolean;
  bonus?: Money;
};

export type ReferralMoney = { invited_paid: number; earned: Money; withdrawn: Money; pending: Money; balance: Money };

export type InvitedPage = { items: InvitedRow[]; next: number | null };

export type ReferralSection = {
  module_enabled: boolean;
  inviter: Inviter | null;
  invited_count: number;
  money?: ReferralMoney;
  invited: InvitedPage;
};

export type Refusal = 'itself' | 'cycle' | 'current';

export type Candidate = { client: ReferralPerson; joined_at: string; blocked: boolean; refusal: Refusal | null };

export type Fmt = { money: (value: Money) => string; date: (iso: string) => string };

export const FIRST_DESK = 5;
export const FIRST_PHONE = 3;
export const MORE_PAGE = 20;
export const SEARCH_MIN = 2;

const VIA: Record<string, string> = {
  bot: 'по ссылке в боте',
  miniapp: 'через мини-приложение',
  web: 'по ссылке на сайт',
  import: 'перенос из прежней панели',
  migration: 'перенос из прежней панели',
};

const REFUSALS: Record<Refusal, string> = {
  itself: 'Это сам клиент',
  cycle: 'Нельзя: в числе приглашённых этого клиента, получится круг',
  current: 'Уже пригласивший',
};

function plural(n: number, one: string, few: string, many: string): string {
  const abs = Math.abs(n);
  const mod10 = abs % 10;
  const mod100 = abs % 100;
  if (mod10 === 1 && mod100 !== 11) {
    return one;
  }
  if (mod10 >= 2 && mod10 <= 4 && (mod100 < 12 || mod100 > 14)) {
    return few;
  }
  return many;
}

function digits(n: number): string {
  return String(n).replace(/\B(?=(\d{3})+(?!\d))/g, ' ');
}

export function personTitle(person: ReferralPerson): string {
  const name = person.name?.trim();
  if (name) {
    return name;
  }
  if (person.telegram_id) {
    return `id ${person.telegram_id}`;
  }
  return person.email?.trim() || `Клиент ${person.client_id}`;
}

export function personContact(person: ReferralPerson): string | null {
  if (person.telegram_username) {
    return `@${person.telegram_username}`;
  }
  const email = person.email?.trim();
  return email && email !== personTitle(person) ? email : null;
}

function since(iso: string, fmt: Fmt): string {
  const shown = fmt.date(iso);
  return /^2\D/.test(shown) || shown === '2' ? `со ${shown}` : `с ${shown}`;
}

export function inviterLine(inviter: Inviter, fmt: Fmt): string {
  const parts: string[] = [];
  if (inviter.manual) {
    parts.push(`привязка вручную ${fmt.date(inviter.manual.at)}`);
    if (inviter.manual.by) {
      parts.push(inviter.manual.by);
    }
  } else {
    parts.push(fmt.date(inviter.linked_at));
    const via = inviter.source ? VIA[inviter.source] : undefined;
    if (via) {
      parts.push(via);
    }
    if (inviter.code) {
      parts.push(`код ${inviter.code}`);
    }
  }
  return parts.join(', ');
}

export function inviterWarning(inviter: Inviter): string | null {
  if (inviter.deleted) {
    return 'Клиент удалён: бонусы с новых оплат никому не начисляются';
  }
  if (inviter.blocked) {
    return 'Клиент заблокирован: пока блокировка не снята, бонусы с новых оплат не начисляются';
  }
  return null;
}

export function invitedLine(row: InvitedRow, fmt: Fmt): string {
  const parts = [since(row.joined_at, fmt)];
  if (row.paid !== undefined) {
    parts.push(row.paid ? 'есть оплаты' : 'оплат нет');
  }
  if (row.manual_at) {
    parts.push(`привязка вручную ${fmt.date(row.manual_at)}`);
  }
  if (row.deleted) {
    parts.push('удалён');
  } else if (row.blocked) {
    parts.push('заблокирован');
  }
  return parts.join(' · ');
}

export function leadText(section: ReferralSection, fmt: Fmt): string {
  const invited = section.invited_count;
  const head = invited > 0 ? `${digits(invited)} ${plural(invited, 'приглашённый', 'приглашённых', 'приглашённых')}` : 'Приглашённых пока нет';
  const money = section.money;
  if (!money) {
    return `${head}.`;
  }
  const sentences: string[] = [];
  if (invited > 0) {
    sentences.push(money.invited_paid > 0 ? `${head}, у ${digits(money.invited_paid)} есть оплаты.` : `${head}, оплат у них пока нет.`);
  } else {
    sentences.push(`${head}.`);
  }
  const tail: string[] = [];
  if (money.withdrawn.minor > 0) {
    tail.push(`выведено ${fmt.money(money.withdrawn)}`);
  }
  if (money.pending.minor > 0) {
    tail.push(`ждёт выплаты ${fmt.money(money.pending)}`);
  }
  if (money.balance.minor !== 0) {
    tail.push(`на реферальном балансе ${fmt.money(money.balance)}`);
  }
  if (money.earned.minor > 0) {
    sentences.push(tail.length ? `Бонусов начислено ${fmt.money(money.earned)}: ${tail.join(', ')}.` : `Бонусов начислено ${fmt.money(money.earned)}.`);
  } else if (tail.length) {
    const text = tail.join(', ');
    sentences.push(`${text.charAt(0).toUpperCase()}${text.slice(1)}.`);
  }
  return sentences.join(' ');
}

export function emptyText(section: ReferralSection): string | null {
  return section.inviter || section.invited_count > 0 ? null : 'Пригласившего нет, приглашённых нет.';
}

export function hidden(section: ReferralSection): boolean {
  return !section.module_enabled && !section.inviter && section.invited_count === 0;
}

export function moreText(section: ReferralSection, shown: number): string | null {
  const left = section.invited_count - shown;
  return section.invited.next !== null && left > 0 ? `Показать ещё ${digits(left)}` : section.invited.next !== null ? 'Показать ещё' : null;
}

export function refusalText(refusal: Refusal | null): string | null {
  return refusal ? REFUSALS[refusal] : null;
}

export function candidateLine(candidate: Candidate, fmt: Fmt): string {
  const refusal = refusalText(candidate.refusal);
  if (refusal) {
    return refusal;
  }
  const parts = [`№${candidate.client.client_id}`, `с нами ${since(candidate.joined_at, fmt)}`];
  if (candidate.blocked) {
    parts.push('заблокирован, бонусы не начисляются');
  }
  return parts.join(', ');
}

export function askSetText(candidate: Candidate, hadInviter: boolean): { head: string; tail: string } {
  return {
    head: `Пригласившим станет ${personTitle(candidate.client)}.`,
    tail: hadInviter
      ? ' Уже начисленные бонусы не пересчитываются, бонусы с новых оплат получит новый пригласивший. Изменение попадёт в журнал действий.'
      : ' Бонусы с новых оплат клиента будут начисляться по правилам программы. Изменение попадёт в журнал действий.',
  };
}

export function referralCode(startParam: unknown): string | null {
  if (typeof startParam !== 'string') {
    return null;
  }
  const part = startParam
    .split('__')
    .map((p) => p.trim())
    .find((p) => p.startsWith('ref_') && /^[A-Za-z0-9]+$/.test(p.slice(4)));
  return part ? part.slice(4).toUpperCase() : null;
}

type AuditLike = { action: string; entity_id: number | null; details: Record<string, unknown> };

function text(value: unknown): string | null {
  return typeof value === 'string' && value.trim() ? value.trim() : null;
}

function id(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null;
}

function field(details: Record<string, unknown>, side: 'from' | 'to'): string | null {
  const part = details[side];
  return part && typeof part === 'object' ? text((part as Record<string, unknown>).referrer) : null;
}

export function referralHistory(item: AuditLike, clientId: number, actor: string): { what: string; note: string } | null {
  if (item.action !== 'client.referrer_set' && item.action !== 'client.referrer_removed') {
    return null;
  }
  const details = item.details ?? {};
  const before = field(details, 'from');
  const after = item.action === 'client.referrer_set' ? field(details, 'to') : null;
  const invitee = text(details.invitee) ?? (item.entity_id !== null ? `№${item.entity_id}` : 'клиент');
  if (item.entity_id === clientId) {
    if (after && before) {
      return { what: 'Сменён пригласивший', note: `${actor}: было ${before}, стало ${after}` };
    }
    if (after) {
      return { what: 'Назначен пригласивший', note: `${actor}: ${after}` };
    }
    return { what: 'Убран пригласивший', note: before ? `${actor}: было ${before}` : actor };
  }
  if (after && id(details.referrer_id) === clientId) {
    return { what: 'Добавлен приглашённый', note: before ? `${actor}: ${invitee}, раньше ${before}` : `${actor}: ${invitee}` };
  }
  if (id(details.previous_referrer_id) === clientId) {
    return { what: 'Отвязан приглашённый', note: after ? `${actor}: ${invitee}, теперь ${after}` : `${actor}: ${invitee}` };
  }
  return null;
}
