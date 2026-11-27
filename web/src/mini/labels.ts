import { msg } from '../lib/i18n';
import type { SectionKey } from './config';

export const TAB_LABELS: Record<SectionKey, string> = {
  home: msg('Главная'),
  plans: msg('Тарифы'),
  connect: msg('Подключить'),
  profile: msg('Профиль'),
  friends: msg('Друзья'),
  support: msg('Поддержка'),
  roulette: msg('Рулетка'),
  gifts: msg('Подарки'),
  promo: msg('Промокод'),
  payments: msg('Оплаты'),
  topup: msg('Баланс'),
  addons: msg('Докупить'),
  guide: msg('Инструкции'),
  trial: msg('Попробовать бесплатно'),
  renew: msg('Продлить'),
  devices: msg('Устройства'),
};

const ROWS: Partial<Record<SectionKey, string>> = {
  addons: msg('Докупить устройства или трафик'),
  gifts: msg('Подарить подписку'),
  payments: msg('История оплат'),
};

const ACTIONS: Partial<Record<SectionKey, string>> = {
  topup: msg('Пополнить'),
  gifts: msg('Подарить'),
};

export const EMPTY_TEXTS = {
  title: msg('Выберите тариф'),
  line: msg('Подписки пока нет'),
  button: msg('Выбрать тариф'),
};

export const FRIENDS_TEXTS = {
  title: msg('Друзья'),
  text: msg('Ваш процент с каждой оплаты'),
};

export function tabText(key: SectionKey): string {
  return TAB_LABELS[key];
}

export function rowText(key: SectionKey): string {
  return ROWS[key] ?? TAB_LABELS[key];
}

export function actionText(key: SectionKey): string {
  return ACTIONS[key] ?? TAB_LABELS[key];
}
