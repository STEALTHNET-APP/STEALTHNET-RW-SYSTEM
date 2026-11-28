import type { Change, ContainerInfo, Health, InstallInfo, PackagePreview, RunView, RuntimeJob, Signature, Volume } from './ext';

const NBSP = ' ';
const KB = 1024;
const MB = KB * 1024;
const GB = MB * 1024;
const MONTHS = ['января', 'февраля', 'марта', 'апреля', 'мая', 'июня', 'июля', 'августа', 'сентября', 'октября', 'ноября', 'декабря'];

export const UPLOAD_PART = 32 * MB;

export const RUNTIME_POLL_MS = 1500;

const STEP_TITLE: Record<string, string> = {
  check: 'Проверка архива',
  load: 'Загрузка образа',
  start: 'Запуск',
  verify: 'Проверка работы',
  stop: 'Остановка',
  clean: 'Удаление',
  logs: 'Журнал',
};

const STATE_TITLE: Record<string, string> = {
  running: 'Работает',
  starting: 'Запускается',
  unhealthy: 'Не отвечает',
  restarting: 'Перезапускается снова и снова',
  stopped: 'Остановлен',
  missing: 'Нет на сервере',
  failed: 'Не запустился',
  unknown: 'Состояние неизвестно',
  installing: 'Устанавливается',
  updating: 'Обновляется',
  stopping: 'Останавливается',
  removing: 'Удаляется',
};

const ACTION_FAILED: Record<string, string> = {
  'module-install': 'Установка не прошла',
  'module-update': 'Обновление не прошло',
  'module-start': 'Запуск не прошёл',
  'module-restart': 'Перезапуск не прошёл',
  'module-stop': 'Остановка не прошла',
  'module-secrets': 'Новые ключ и секрет не применились',
  'module-remove': 'Удаление не прошло',
  'module-logs': 'Журнал не обновился',
};

export function stepTitle(key: string): string {
  return STEP_TITLE[key] ?? key;
}

export function stateTitle(state: string | null | undefined): string {
  return (state && STATE_TITLE[state]) || 'Состояние неизвестно';
}

export function jobActive(job: RuntimeJob | null | undefined): boolean {
  return job?.state === 'waiting' || job?.state === 'running';
}

export function jobFailure(job: RuntimeJob | null | undefined): string | null {
  if (!job || (job.state !== 'failed' && job.state !== 'rolled_back')) {
    return null;
  }
  const head = job.state === 'rolled_back' ? 'Новая версия не заработала, вернулась прежняя' : (ACTION_FAILED[job.action] ?? 'Заявка не выполнилась');
  return job.error ? `${head}: ${job.error}` : head;
}

function plural(n: number, one: string, few: string, many: string): string {
  const a = Math.abs(n) % 100;
  const b = a % 10;
  if (b === 1 && a !== 11) {
    return one;
  }
  if (b >= 2 && b <= 4 && (a < 12 || a > 14)) {
    return few;
  }
  return many;
}

function decimal(value: number): string {
  const text = value < 100 ? value.toFixed(1) : Math.round(value).toString();
  return text.replace(/\.0$/, '').replace('.', ',');
}

export function sizeText(bytes: number): string {
  if (bytes <= 0) {
    return `0${NBSP}КБ`;
  }
  if (bytes < MB) {
    return `${Math.max(1, Math.round(bytes / KB))}${NBSP}КБ`;
  }
  if (bytes < GB) {
    return `${decimal(bytes / MB)}${NBSP}МБ`;
  }
  return `${decimal(bytes / GB)}${NBSP}ГБ`;
}

export function memoryText(mb: number): string {
  return mb >= 1024 ? `${decimal(mb / 1024)}${NBSP}ГБ` : `${mb}${NBSP}МБ`;
}

function cpuValue(cpus: string): number {
  const n = Number.parseFloat(cpus);
  return Number.isFinite(n) ? n : 0;
}

function cpuNumber(n: number): string {
  return (Number.isInteger(n) ? String(n) : n.toFixed(1)).replace('.', ',');
}

export function cpuShare(cpus: string): string {
  const n = cpuValue(cpus);
  if (n === 0.5) {
    return 'половину ядра';
  }
  if (n === 1) {
    return 'одно ядро';
  }
  if (n === 2) {
    return 'два ядра';
  }
  return `${cpuNumber(n)}${NBSP}ядра`;
}

export function cpuName(cpus: string): string {
  const n = cpuValue(cpus);
  if (n === 0.5) {
    return 'половина ядра';
  }
  if (n === 1) {
    return 'одно ядро';
  }
  if (n === 2) {
    return 'два ядра';
  }
  return `${cpuNumber(n)}${NBSP}ядра`;
}

export function cpuLimit(cpus: string): string {
  const n = cpuValue(cpus);
  if (n === 0.5) {
    return 'до половины ядра';
  }
  if (Number.isInteger(n)) {
    return `до ${n}${NBSP}${plural(n, 'ядра', 'ядер', 'ядер')}`;
  }
  return `до ${cpuNumber(n)}${NBSP}ядра`;
}

export function joinAnd(items: string[]): string {
  if (items.length <= 1) {
    return items[0] ?? '';
  }
  return `${items.slice(0, -1).join(', ')} и ${items.at(-1) ?? ''}`;
}

function quoted(items: string[]): string[] {
  return items.map((item) => `«${item}»`);
}

export function capital(text: string): string {
  return text ? text.charAt(0).toUpperCase() + text.slice(1) : text;
}

export function kept<T extends { change: Change }>(items: T[]): T[] {
  return items.filter((item) => item.change !== 'removed');
}

export function volumesText(volumes: Volume[]): string | null {
  if (volumes.length === 0) {
    return null;
  }
  const names = quoted(volumes.map((v) => v.name)).join(', ');
  return volumes.length === 1 ? `папка ${names}` : `папки ${names}`;
}

export function processorName(platform: string): string {
  if (platform === 'linux/amd64') {
    return 'x86_64';
  }
  if (platform === 'linux/arm64') {
    return 'ARM64';
  }
  return platform;
}

export function platformsText(platforms: string[]): string {
  const names = platforms.map(processorName);
  return names.length === 1 ? `Сборка для ${names[0] ?? ''}` : `Сборки для ${joinAnd(names)}`;
}

export function signatureNote(signature: Signature, author: string): { text: string; ok: boolean } {
  if (signature.state === 'verified') {
    return { text: signature.signer === author ? 'подпись проверена' : `подпись ${signature.signer} проверена`, ok: true };
  }
  return { text: signature.state === 'unknown' ? 'подписан неизвестным ключом' : 'без подписи', ok: false };
}

export function signatureLine(signature: Signature): string {
  if (signature.state === 'verified') {
    return `Ключ подписи: ed25519 ${signature.fingerprint}`;
  }
  if (signature.state === 'unknown') {
    return `Подписан неизвестным ключом: ed25519 ${signature.fingerprint}`;
  }
  return 'Без подписи';
}

function rightsSentence(permissions: string[], events: string[]): string {
  const rights = `${permissions.length === 1 ? 'право' : 'права'} ${joinAnd(quoted(permissions))}`;
  const kinds = `${events.length === 1 ? 'будет приходить событие' : 'будут приходить события'} ${joinAnd(quoted(events))}`;
  if (permissions.length === 0 && events.length === 0) {
    return 'В панели он ничего не читает и не меняет: у его ключа нет прав, события ему не приходят.';
  }
  if (events.length === 0) {
    return `В панели его ключ получит ${rights}, события ему не приходят.`;
  }
  if (permissions.length === 0) {
    return `В панели у его ключа нет прав, но ему ${kinds}.`;
  }
  return `В панели его ключ получит ${rights}, и ему ${kinds}.`;
}

export function plainSummary(preview: PackagePreview): string {
  const run = preview.run;
  const net = run.internet ? 'с выходом в интернет' : 'без выхода в интернет';
  const parts = [
    `На этом сервере модуль будет работать отдельной программой: до ${memoryText(run.memory_mb)} памяти и ${cpuShare(run.cpus)} процессора, ${net}.`,
    rightsSentence(
      kept(preview.permissions).map((p) => p.title),
      kept(preview.events).map((e) => e.title),
    ),
  ];
  if (preview.page) {
    parts.push(`Его страница «${preview.page.title}» появится у администраторов с правом «${preview.page.permission_title}».`);
  }
  if (preview.client_card) {
    parts.push(`В карточке клиента появится раздел «${preview.client_card.title}» для тех, у кого есть право «${preview.client_card.permission_title}».`);
  }
  if (preview.bot) {
    parts.push(`В главном меню бота появится кнопка «${preview.bot.button}».`);
  }
  if (preview.inbound) {
    parts.push('Модуль принимает запросы сторонних сервисов через адрес панели.');
  }
  return parts.join(' ');
}

export type ChangeLine = { label: string; before: string; after: string };

function namesOf(volumes: Volume[]): string {
  return quoted(volumes.map((v) => v.name).sort()).join(', ');
}

function titlesOf(items: Array<{ title: string; change: Change }>, skip: Change): string {
  return joinAnd(quoted(items.filter((i) => i.change !== skip).map((i) => i.title)));
}

export function runChanges(preview: PackagePreview): ChangeLine[] {
  const out: ChangeLine[] = [];
  const before: RunView | null = preview.previous_run;
  const after = preview.run;
  if (preview.installed && preview.installed.version !== preview.version) {
    out.push({ label: 'Версия', before: preview.installed.version, after: preview.version });
  }
  if (before) {
    if (before.memory_mb !== after.memory_mb) {
      out.push({ label: 'Память', before: `до ${memoryText(before.memory_mb)}`, after: `до ${memoryText(after.memory_mb)}` });
    }
    if (cpuValue(before.cpus) !== cpuValue(after.cpus)) {
      out.push({ label: 'Процессор', before: cpuName(before.cpus), after: cpuName(after.cpus) });
    }
    if (before.internet !== after.internet) {
      out.push({ label: 'Интернет', before: before.internet ? 'есть' : 'нет', after: after.internet ? 'есть' : 'нет' });
    }
    const was = namesOf(before.volumes);
    const now = namesOf(after.volumes);
    if (was !== now) {
      out.push({ label: 'Папки данных', before: was || 'нет', after: now || 'нет' });
    }
  }
  if (preview.permissions.some((p) => p.change !== 'same')) {
    out.push({ label: 'Права ключа API', before: titlesOf(preview.permissions, 'added') || 'нет', after: titlesOf(preview.permissions, 'removed') || 'нет' });
  }
  if (preview.events.some((e) => e.change !== 'same')) {
    out.push({ label: 'События', before: titlesOf(preview.events, 'added') || 'нет', after: titlesOf(preview.events, 'removed') || 'нет' });
  }
  return out;
}

export type FileKind = 'zip' | 'json';

export function fileKind(name: string, type: string): FileKind | null {
  const lower = name.toLowerCase();
  if (lower.endsWith('.zip') || type === 'application/zip' || type === 'application/x-zip-compressed') {
    return 'zip';
  }
  if (lower.endsWith('.json') || type === 'application/json') {
    return 'json';
  }
  return null;
}

export type BlockedStep = { text: string; command?: string };

export type Blocked = { title: string; about: string; steps: BlockedStep[] };

const JSON_INSTEAD: BlockedStep = { text: 'Или запустите модуль по инструкции его автора и загрузите сюда файл описания модуля (JSON).' };

export function zipBlocked(info: InstallInfo): Blocked | null {
  if (!info.owner) {
    return {
      title: 'Модули из архива ставит владелец',
      about: 'Архив запускает свой код на этом сервере, поэтому его ставит только владелец панели. Модуль, который работает на другом сервере, можно подключить файлом описания JSON.',
      steps: [],
    };
  }
  if (info.packages.state === 'ready') {
    return null;
  }
  const about = `Архив устанавливает помощник панели — служба на сервере, которая запускает модули и обновления. ${capital(info.reason)}.`;
  const title = 'Архив модуля на этом сервере не поставить';
  switch (info.packages.state) {
    case 'helper_off':
      return { title, about, steps: [{ text: 'Включите помощника командой на сервере и обновите эту страницу:', command: 'sudo stealthnet helper on' }, JSON_INSTEAD] };
    case 'outdated':
      return { title, about, steps: [{ text: 'Обновите систему командой на сервере и обновите эту страницу:', command: 'sudo stealthnet update' }, JSON_INSTEAD] };
    case 'modules_off':
      return { title, about, steps: [{ text: 'Разрешите установку модулей командой на сервере и обновите эту страницу:', command: 'sudo stealthnet modules on' }, JSON_INSTEAD] };
    default:
      return {
        title,
        about,
        steps: [
          { text: 'Если у вас есть доступ root, включите помощника командой на сервере и обновите эту страницу:', command: 'sudo stealthnet helper on' },
          { text: 'Если root нет или на сервере нет systemd, запустите модуль по инструкции его автора и загрузите сюда файл описания модуля (JSON).' },
        ],
      };
  }
}

export function uptimeText(seconds: number): string {
  const days = Math.floor(seconds / 86_400);
  if (days >= 1) {
    return `${days}${NBSP}${plural(days, 'день', 'дня', 'дней')}`;
  }
  const hours = Math.floor(seconds / 3600);
  if (hours >= 1) {
    return `${hours}${NBSP}${plural(hours, 'час', 'часа', 'часов')}`;
  }
  const minutes = Math.max(1, Math.floor(seconds / 60));
  return `${minutes}${NBSP}${plural(minutes, 'минуту', 'минуты', 'минут')}`;
}

function clock(date: Date): string {
  return `${String(date.getHours()).padStart(2, '0')}:${String(date.getMinutes()).padStart(2, '0')}`;
}

function sameDay(a: Date, b: Date): boolean {
  return a.getFullYear() === b.getFullYear() && a.getMonth() === b.getMonth() && a.getDate() === b.getDate();
}

export function momentText(iso: string, now: Date): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return sameDay(date, now) ? `в ${clock(date)}` : `${date.getDate()}${NBSP}${MONTHS[date.getMonth()] ?? ''} в ${clock(date)}`;
}

export function sinceText(iso: string, now: Date): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) {
    return '';
  }
  return sameDay(date, now) ? `с ${clock(date)}` : `с ${date.getDate()}${NBSP}${MONTHS[date.getMonth()] ?? ''}, ${clock(date)}`;
}

export function runningNote(state: string, container: ContainerInfo | null, health: Health | null, now: Date): string {
  const parts: string[] = [];
  const restarts = container?.restarts ?? 0;
  if (container?.state === 'running' && container.started_at) {
    const up = uptimeText(Math.max(0, (now.getTime() - Date.parse(container.started_at)) / 1000));
    parts.push(restarts ? `работает ${up}` : `${up} без перезапусков`);
  }
  if (restarts) {
    parts.push(`перезапусков: ${restarts}`);
  }
  if (state === 'running' && health?.ok) {
    parts.push('отвечает на проверку');
  }
  return parts.join(' · ');
}

export function healthProblem(state: string, health: Health | null, now: Date): string | null {
  if (state !== 'unhealthy' || !health || health.ok) {
    return null;
  }
  const since = sinceText(health.since ?? health.at, now);
  return `Не отвечает на проверку ${since}${health.error ? `: ${health.error}` : ''}`;
}

export function memoryLine(run: RunView, container: ContainerInfo | null): string {
  const limit = memoryText(run.memory_mb);
  const used = container?.memory_bytes;
  const memory = used !== null && used !== undefined ? `${sizeText(used)} из ${limit}` : `до ${limit}`;
  return `${memory} · процессор ${cpuLimit(run.cpus)}`;
}

export function dataLine(run: RunView, container: ContainerInfo | null): string {
  const folders = volumesText(run.volumes);
  if (!folders) {
    return 'модуль ничего не хранит';
  }
  const size = container?.data_bytes;
  return size !== null && size !== undefined ? `${folders}, ${sizeText(size)}` : folders;
}
