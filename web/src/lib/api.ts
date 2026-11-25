import { leavesPath } from './apiPath';
import { lang, t, type Lang } from './i18n';

export class ApiError extends Error {
  readonly status: number;
  readonly code: string;
  readonly details: Record<string, unknown>;

  constructor(status: number, code: string, message: string, details: Record<string, unknown> = {}) {
    super(message);
    this.status = status;
    this.code = code;
    this.details = details;
  }
}

type Method = 'GET' | 'POST' | 'PUT' | 'PATCH' | 'DELETE';

type Options = {
  method?: Method;
  body?: unknown;
  signal?: AbortSignal;
  keepalive?: boolean;
  lang?: Lang | null;
};

function errorFrom(status: number, data: unknown): ApiError {
  if (data && typeof data === 'object' && 'error' in data) {
    const err = (data as { error?: { code?: unknown; message?: unknown } }).error;
    const code = typeof err?.code === 'string' ? err.code : `http.${status}`;
    const message = typeof err?.message === 'string' ? err.message : t('Ошибка {status}', { status });
    return new ApiError(status, code, message, err && typeof err === 'object' ? (err as Record<string, unknown>) : {});
  }
  if (typeof data === 'string' && data.trim()) {
    return new ApiError(status, `http.${status}`, data.trim());
  }
  return new ApiError(status, `http.${status}`, t('Ошибка {status}', { status }));
}

function checkPath(path: string): void {
  if (leavesPath(path)) {
    throw new ApiError(0, 'path', t('Неверный адрес запроса'));
  }
}

export type Answer = (path: string, options: { method: Method; body?: unknown }) => Promise<unknown>;

let answer: Answer | null = null;

export function setAnswer(next: Answer | null): void {
  answer = next;
}

let bearer: string | null = null;

export function setBearer(token: string | null): void {
  bearer = token;
}

export async function api<T>(path: string, options: Options = {}): Promise<T> {
  checkPath(path);
  if (answer) {
    return (await answer(path, { method: options.method ?? 'GET', body: options.body })) as T;
  }
  const headers: Record<string, string> = { accept: 'application/json' };
  if (bearer && path.startsWith('/api/client')) {
    headers.authorization = `Bearer ${bearer}`;
  }
  if (path.startsWith('/api/client') || path.startsWith('/api/public')) {
    headers['x-sn-lang'] = options.lang ?? lang();
  }
  let body: string | undefined;
  if (options.body !== undefined) {
    headers['content-type'] = 'application/json';
    body = JSON.stringify(options.body);
  }
  let response: Response;
  try {
    response = await fetch(path, {
      method: options.method ?? 'GET',
      headers,
      body,
      credentials: 'same-origin',
      signal: options.signal,
      keepalive: options.keepalive,
    });
  } catch (cause) {
    if (cause instanceof DOMException && cause.name === 'AbortError') {
      throw cause;
    }
    throw new ApiError(0, 'network', t('Нет связи с сервером. Проверьте интернет и попробуйте ещё раз'));
  }
  return read<T>(response);
}

async function read<T>(response: Response): Promise<T> {
  const text = await response.text();
  let data: unknown = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = text;
    }
  }
  if (!response.ok) {
    throw errorFrom(response.status, data);
  }
  return data as T;
}

function clientHeaders(path: string, headers: Record<string, string>): Record<string, string> {
  if (bearer && path.startsWith('/api/client')) {
    headers.authorization = `Bearer ${bearer}`;
  }
  if (path.startsWith('/api/client')) {
    headers['x-sn-lang'] = lang();
  }
  return headers;
}

export async function upload<T>(path: string, file: Blob): Promise<T> {
  checkPath(path);
  if (answer) {
    return (await answer(path, { method: 'POST' })) as T;
  }
  let response: Response;
  try {
    response = await fetch(path, {
      method: 'POST',
      headers: clientHeaders(path, { accept: 'application/json', 'content-type': file.type || 'application/octet-stream' }),
      body: file,
      credentials: 'same-origin',
    });
  } catch {
    throw new ApiError(0, 'network', t('Нет связи с сервером. Проверьте интернет и попробуйте ещё раз'));
  }
  return read<T>(response);
}

export async function fetchBlob(path: string): Promise<Blob> {
  checkPath(path);
  if (answer) {
    return (await answer(path, { method: 'GET' })) as Blob;
  }
  let response: Response;
  try {
    response = await fetch(path, { headers: clientHeaders(path, {}), credentials: 'same-origin' });
  } catch {
    throw new ApiError(0, 'network', t('Нет связи с сервером. Проверьте интернет и попробуйте ещё раз'));
  }
  if (!response.ok) {
    await read<unknown>(response);
  }
  return response.blob();
}
