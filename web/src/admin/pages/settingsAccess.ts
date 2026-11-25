import type { Permission } from '../../lib/types';

export type SettingsSection = { label: string; to: string; need: Permission[] };

const SETTINGS: Permission[] = ['settings'];

export const SECTIONS: SettingsSection[] = [
  { label: 'Общие', to: '/admin/settings/general', need: SETTINGS },
  { label: 'Remnawave', to: '/admin/settings/remnawave', need: SETTINGS },
  { label: 'Оформление', to: '/admin/settings/look', need: SETTINGS },
  { label: 'Оплата', to: '/admin/settings/payments', need: SETTINGS },
  { label: 'Бот', to: '/admin/settings/bot', need: SETTINGS },
  { label: 'Документы', to: '/admin/settings/documents', need: SETTINGS },
  { label: 'Лендинг', to: '/admin/settings/landing', need: SETTINGS },
  { label: 'Кабинет на сайте', to: '/admin/settings/cabinet', need: SETTINGS },
  { label: 'Мини-приложение', to: '/admin/settings/miniapp', need: SETTINGS },
  { label: 'Кабинет и мини-приложение', to: '/admin/settings/client', need: SETTINGS },
  { label: 'Языки', to: '/admin/settings/languages', need: SETTINGS },
  { label: 'Почта и уведомления', to: '/admin/settings/mail', need: SETTINGS },
  { label: 'Уведомления в Telegram', to: '/admin/settings/alerts', need: SETTINGS },
  { label: 'Администраторы', to: '/admin/settings/admins', need: ['admins'] },
  { label: 'API-ключи', to: '/admin/settings/api-keys', need: SETTINGS },
  { label: 'Обновления', to: '/admin/settings/updates', need: SETTINGS },
  { label: 'Резервные копии', to: '/admin/settings/backups', need: ['backups'] },
  { label: 'Перенос из другого бота', to: '/admin/settings/import', need: ['settings', 'money'] },
];

const PAGES: Array<{ to: string; need: Permission[] }> = [{ to: '/admin/settings/remnawave/sync', need: ['remna_sync'] }];

export function firstSection(can: (permission: Permission) => boolean): SettingsSection | undefined {
  return SECTIONS.find((section) => section.need.every(can));
}

function under(pathname: string, to: string): boolean {
  return pathname === to || pathname.startsWith(`${to}/`);
}

export function pageNeed(pathname: string): Permission[] {
  const page = PAGES.find((p) => under(pathname, p.to));
  if (page) {
    return page.need;
  }
  return SECTIONS.find((s) => under(pathname, s.to))?.need ?? SETTINGS;
}
