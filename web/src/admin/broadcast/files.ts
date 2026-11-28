export type MediaKind = 'photo' | 'video';

export type MediaLimits = { image_bytes: number; video_bytes: number };

type Picked = { type: string; name: string; size: number };

export const ACCEPT_PHOTO = 'image/png,image/jpeg';
export const ACCEPT_ANY = `${ACCEPT_PHOTO},video/mp4`;

export function kindOf(file: Pick<Picked, 'type' | 'name'>): MediaKind | 'mov' | null {
  if (file.type === 'image/png' || file.type === 'image/jpeg') {
    return 'photo';
  }
  if (file.type === 'video/mp4') {
    return 'video';
  }
  if (file.type === 'video/quicktime' || /\.mov$/i.test(file.name)) {
    return 'mov';
  }
  return null;
}

export function wholeMb(bytes: number): string {
  return `${Math.round(bytes / 1_000_000)} МБ`;
}

export function fileProblem(file: Picked, limits: MediaLimits, bot: boolean): string | null {
  const kind = kindOf(file);
  if (kind === 'mov') {
    return 'Это ролик MOV, а видео с проигрывателем Telegram показывает из MP4. Сохраните ролик в MP4 и загрузите снова';
  }
  if (kind === null || (kind === 'video' && !bot)) {
    return bot ? 'Нужна картинка JPEG или PNG или видео MP4' : 'В письмо можно приложить только картинку JPEG или PNG: видео уходит только в Telegram';
  }
  if (kind === 'photo' && file.size > limits.image_bytes) {
    return `Картинка больше ${wholeMb(limits.image_bytes)}: Telegram не примет её как фото`;
  }
  if (kind === 'video' && file.size > limits.video_bytes) {
    return `Видео больше ${wholeMb(limits.video_bytes)}: Telegram не даёт ботам отправлять файлы больше`;
  }
  return null;
}

export function clock(seconds: number | null): string {
  const total = Math.max(0, Math.round(seconds ?? 0));
  const hours = Math.floor(total / 3600);
  const minutes = Math.floor((total % 3600) / 60);
  const rest = (total % 60).toString().padStart(2, '0');
  return hours > 0 ? `${hours}:${minutes.toString().padStart(2, '0')}:${rest}` : `${minutes}:${rest}`;
}

export function megabytes(bytes: number): string {
  if (bytes < 1_000_000) {
    return `${Math.max(1, Math.round(bytes / 1000))} КБ`;
  }
  const fixed = (bytes / 1_000_000).toFixed(1);
  return `${(fixed.endsWith('.0') ? fixed.slice(0, -2) : fixed).replace('.', ',')} МБ`;
}
