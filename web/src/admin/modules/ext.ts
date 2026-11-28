import type { Permission } from '../../lib/types';
import type { SettingField } from '../SchemaFields';

export type Change = 'same' | 'added' | 'removed';

export type ExtEvent = { kind: string; title: string; about: string; change: Change };

export type ExtPermission = { key: Permission; title: string; about: string; sensitive: boolean; change: Change };

export type ExtPage = { url: string; path: string | null; title: string; permission: Permission; permission_title: string };

export type ExtClientCard = { title: string; permission: Permission; permission_title: string };

export type ExtBot = { button: string };

export type ExtSummary = {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string | null;
  enabled: boolean;
  has_settings: boolean;
  page: ExtPage | null;
  warning: string | null;
  managed: boolean;
  state: string | null;
};

export type KeyState = 'active' | 'missing' | 'revoked' | 'expired' | 'owner_disabled';

export type ExtCard = {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string | null;
  site: string | null;
  webhook: string | null;
  enabled: boolean;
  events: ExtEvent[];
  permissions: ExtPermission[];
  page: ExtPage | null;
  client_card?: ExtClientCard | null;
  bot?: ExtBot | null;
  inbound?: string | null;
  has_settings: boolean;
  key: { prefix: string | null; state: KeyState; owner: string | null; last_used_at: string | null };
  installed_by: string;
  installed_at: string;
  updated_at: string;
  stats: { pending: number; failed: number; last_ok: boolean | null; last_at: string | null };
  warning: string | null;
  managed: boolean;
  takes_events: boolean;
};

export type Problem = { path: string; message: string };

export type Moved = { address: 'webhook' | 'page'; before: string | null; after: string | null };

export type Preview = {
  id: string;
  name: string;
  version: string;
  author: string;
  description: string | null;
  site: string | null;
  webhook: string | null;
  page: ExtPage | null;
  client_card?: ExtClientCard | null;
  bot?: ExtBot | null;
  inbound?: string | null;
  settings: string[];
  events: ExtEvent[];
  permissions: ExtPermission[];
  sensitive: boolean;
  installed: { version: string; enabled: boolean } | null;
  moved: Moved[];
};

export type PreviewResult = { errors: Problem[]; preview: Preview | null };

export type Credentials = { api_key: string; signing_secret: string };

export type InstallResult = { module: ExtCard; created: boolean; credentials: Credentials | null };

export type ExtConfig = { module: string; schema: SettingField[]; config: Record<string, unknown> };

export type ExtConfigSaved = ExtConfig & { event_queued: boolean };

export type Attempt = {
  id: number;
  delivery_id: number;
  kind: string;
  title: string;
  event_id: number | null;
  attempt: number;
  at: string;
  http_status: number | null;
  duration_ms: number;
  error: string | null;
  response: string | null;
  status: 'pending' | 'done' | 'failed' | 'skipped';
  next_attempt_at: string | null;
  can_retry: boolean;
};

export type Journal = { attempts: Attempt[]; stats: ExtCard['stats'] };

export type PingResult = { ok: boolean; http_status: number | null; duration_ms: number; error: string | null; response: string | null };

export type PageToken = { url: string; origin: string; token: string; expires_at: string; proxied: boolean; title: string; module: string };

export type HelperAvailability = 'ready' | 'no_helper' | 'helper_off' | 'outdated' | 'modules_off';

export type HelperState = { state: HelperAvailability; why: string | null; busy: boolean };

export type InstallInfo = {
  owner: boolean;
  totp_required: boolean;
  packages: HelperState;
  reason: string;
  processor: string | null;
  max_bytes: number;
};

export type Volume = { name: string; path: string };

export type RunView = {
  port: number;
  memory_mb: number;
  cpus: string;
  internet: boolean;
  volumes: Volume[];
  health_path: string;
  events_path: string | null;
  page_path: string | null;
};

export type Signature = { state: 'verified'; signer: string; fingerprint: string } | { state: 'unknown'; fingerprint: string } | { state: 'unsigned' };

export type PackageView = {
  bytes: number;
  sha256: string;
  signature: Signature;
  platforms: string[];
  processor: string;
  image_bytes: number;
};

export type PackagePreview = Preview & { run: RunView; previous_run: RunView | null; package: PackageView };

export type PackageCheck = { errors: Problem[]; preview: PackagePreview | null };

export type Uploaded = { id: string; size: number };

export type RuntimeState =
  | 'running'
  | 'starting'
  | 'unhealthy'
  | 'restarting'
  | 'stopped'
  | 'missing'
  | 'failed'
  | 'unknown'
  | 'installing'
  | 'updating'
  | 'stopping'
  | 'removing';

export type JobStep = { key: string; state: 'pending' | 'running' | 'done' | 'failed'; seconds: number | null };

export type RuntimeJob = {
  id: string;
  action: string;
  state: 'waiting' | 'running' | 'done' | 'failed' | 'rolled_back';
  steps: JobStep[];
  error: string | null;
  restored: boolean | null;
  requested_at: string | null;
  finished_at: string | null;
};

export type ContainerInfo = {
  state: string;
  started_at: string | null;
  restarts: number | null;
  image: string | null;
  memory_bytes: number | null;
  data_bytes: number | null;
  checked_at: string | null;
};

export type Health = { ok: boolean; at: string; error: string | null; since: string | null };

export type InstalledPackage = { sha256: string; bytes: number; signature: Signature; platforms?: string[]; image_bytes?: number; installed_at?: string };

export type RuntimeView = {
  module: string;
  state: RuntimeState;
  enabled: boolean;
  version: string;
  run: RunView | null;
  job: RuntimeJob | null;
  container: ContainerInfo | null;
  health: Health | null;
  package: InstalledPackage | null;
  helper: HelperState;
  reason: string;
  owner: boolean;
  totp_required: boolean;
  can_logs: boolean;
};

export type PackageInstalled = { module: ExtCard; runtime: RuntimeView };

export type Reissued = { credentials: Credentials | null; runtime: RuntimeView | null };

export type LogsView = { lines: string[]; at: string | null };

export const KEY_TEXT: Record<KeyState, string> = {
  active: 'работает',
  missing: 'нет ключа',
  revoked: 'отозван',
  expired: 'срок истёк',
  owner_disabled: 'не работает: администратор отключён',
};

export const MANIFEST_LIMIT = 64 * 1024;

export function extLink(id: string): string {
  return `/admin/modules/ext/${encodeURIComponent(id)}`;
}

export function extPageLink(id: string): string {
  return `/admin/ext/${encodeURIComponent(id)}`;
}

export function savedNote(eventQueued: boolean): string {
  return eventQueued ? 'Настройки сохранены, модуль получит событие об изменении' : 'Настройки сохранены';
}

export function stateNote(card: Pick<ExtCard, 'enabled' | 'takes_events'>): string {
  if (!card.enabled) {
    return 'События не уходят, ключ API не принимается.';
  }
  return card.takes_events ? 'Получает события, ключ API работает.' : 'Ключ API работает.';
}

export function installedNote(card: Pick<ExtCard, 'takes_events'>): string {
  const start = card.takes_events ? 'Он уже включён: события начнут уходить на его адрес.' : 'Он уже включён.';
  return `${start} Передайте модулю ключ и секрет — без них он не сможет проверить подпись и обратиться к API.`;
}

export function noEventsNote(run: Pick<RunView, 'events_path'>): string {
  return run.events_path === null ? 'Нет: модуль не принимает события.' : 'Нет: только «Проверка связи» и «Настройки изменены».';
}
