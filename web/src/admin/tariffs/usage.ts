export type TariffHolder = {
  client_id: number;
  subscription_id: number;
  name: string | null;
  telegram_username: string | null;
  email: string | null;
  expire_at: string | null;
  active: boolean;
};

export type TariffUsage = { active: number; ended: number; holders: TariffHolder[] };

function word(n: number, one: string, few: string, many: string): string {
  const tens = Math.abs(n) % 100;
  const units = tens % 10;
  if (tens > 10 && tens < 20) {
    return many;
  }
  if (units === 1) {
    return one;
  }
  return units > 1 && units < 5 ? few : many;
}

function counted(n: number, one: string, few: string, many: string): string {
  return `${n} ${word(n, one, few, many)}`;
}

export function heldText(usage: Pick<TariffUsage, 'active' | 'ended'>): string {
  const { active, ended } = usage;
  if (ended === 0) {
    return counted(active, 'действующая подписка', 'действующие подписки', 'действующих подписок');
  }
  if (active === 0) {
    return counted(ended, 'закончившаяся подписка', 'закончившиеся подписки', 'закончившихся подписок');
  }
  return `${counted(active + ended, 'подписка', 'подписки', 'подписок')}, из них ${word(ended, 'закончилась', 'закончились', 'закончились')} ${ended}`;
}

export function holderName(holder: Pick<TariffHolder, 'client_id' | 'name' | 'telegram_username' | 'email'>): string {
  return holder.name?.trim() || (holder.telegram_username ? `@${holder.telegram_username}` : null) || holder.email || `Клиент ${holder.client_id}`;
}
