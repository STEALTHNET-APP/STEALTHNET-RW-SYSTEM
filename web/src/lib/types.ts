export type AdminDesign = 'side' | 'top';
export type AdminTheme = 'dark' | 'light' | 'system';
export type ClientDesign = 'accent' | 'mono';
export type ClientTheme = 'dark' | 'light' | 'client';

export type Appearance = {
  admin: AdminDesign;
  admin_theme: AdminTheme;
  client: ClientDesign;
  client_theme: ClientTheme;
};

export type Currency = 'USD' | 'RUB';

export type SiteDocument = { title: string; title_en?: string | null; url: string; page?: boolean };

export type Site = {
  brand_name: string;
  title?: string;
  logo_url?: string | null;
  currency: Currency;
  support_url: string | null;
  support_tickets?: boolean;
  support_ai?: { cabinet: boolean; miniapp: boolean };
  support_files?: { max_bytes: number; max_files: number } | null;
  bot_username: string | null;
  appearance: Appearance;
  connect?: { mode: 'ours' | 'remnawave'; page_config: string | null };
  autorenew_days_before?: number;
  trial?: { enabled: boolean; days: number };
  documents?: SiteDocument[];
  email_auth?: 'password' | 'code' | null;
  email_reset?: boolean;
  email_confirm?: boolean;
  captcha?: { provider: 'turnstile' | 'yandex'; site_key: string } | null;
  captcha_spare?: { provider: 'turnstile' | 'yandex'; site_key: string } | null;
  oauth?: { google: boolean; apple: boolean };
  client_access?: { miniapp: boolean; miniapp_fullscreen?: boolean; site_login: boolean; devices: boolean; reissue: boolean };
  languages?: { list: Array<'ru' | 'en'>; default: 'ru' | 'en' };
  miniapp?: unknown;
  cabinet?: { css?: string | null } | null;
  maintenance?: Maintenance | null;
};

export type Maintenance = { message: { ru: string; en: string }; until: string | null };

export type Permission =
  | 'clients_read'
  | 'clients_write'
  | 'money'
  | 'refunds'
  | 'tariffs'
  | 'infrastructure'
  | 'support'
  | 'marketing'
  | 'modules'
  | 'settings'
  | 'admins'
  | 'clients_login'
  | 'remna_sync'
  | 'backups'
  | 'audit_log';

export type AdminRole = 'owner' | 'admin' | 'support' | 'read_only';

export type ModuleMenuItem = {
  module: string;
  label: string;
  icon: string;
  order: number;
};

export type ExtMenuPage = { id: string; title: string };

export type Me = {
  admin: { admin_id: number; role: AdminRole; permissions: Permission[]; session_id: number };
  email: string | null;
  must_change_password?: boolean;
  permissions: Permission[];
  menu: ModuleMenuItem[];
  pages?: ExtMenuPage[];
  modules?: string[];
  currency: Currency;
  brand_name: string;
  logo_url?: string | null;
  appearance: Appearance;
  remnawave?: boolean;
  maintenance?: boolean;
};
