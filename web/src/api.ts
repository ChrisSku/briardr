// HTTP API client. Every request has a deadline; failures are categorized so the UI can show a
// precise message and the right recovery action.
import { theHost, TheHost } from './host.ts';
import { holdUpdates } from './updates.ts';

export type FailureCategory = 'auth' | 'protocol' | 'network' | 'timeout' | 'herdr' | 'server' | 'client';

export class ApiError extends Error {
  status: number;
  code: string;
  category: FailureCategory;
  /** The response body, when there was one. */
  data: any;
  constructor(status: number, code: string, message: string, category: FailureCategory, data: any = null) {
    super(message);
    this.status = status;
    this.code = code;
    this.category = category;
    this.data = data;
  }
}

const DEFAULT_TIMEOUT = 15_000;

export async function api<T = any>(method: string, path: string, body?: unknown, opts: { timeout?: number; signal?: AbortSignal; raw?: BodyInit; headers?: Record<string, string> } = {}): Promise<T> {
  const ctrl = new AbortController();
  const timer = setTimeout(() => ctrl.abort('timeout'), opts.timeout ?? DEFAULT_TIMEOUT);
  opts.signal?.addEventListener('abort', () => ctrl.abort('cancelled'));
  let res: Response;
  try {
    res = await fetch(path, {
      method,
      credentials: 'same-origin',
      headers: body !== undefined ? { 'content-type': 'application/json', ...opts.headers } : opts.headers,
      body: opts.raw ?? (body !== undefined ? JSON.stringify(body) : undefined),
      signal: ctrl.signal,
      cache: 'no-store',
    });
  } catch (e) {
    clearTimeout(timer);
    if (ctrl.signal.reason === 'timeout') throw new ApiError(0, 'timeout', `${TheHost()} didn’t answer in time.`, 'timeout');
    if (ctrl.signal.reason === 'cancelled') throw new ApiError(0, 'cancelled', 'Cancelled.', 'client');
    throw new ApiError(0, 'network', `Can’t reach ${theHost()}.`, 'network');
  }
  clearTimeout(timer);
  const type = res.headers.get('content-type') || '';
  const data: any = type.includes('json') ? await res.json().catch(() => ({})) : await res.text();
  if (!res.ok) {
    const code = data?.error || `http_${res.status}`;
    const message = data?.message || (res.status === 401 ? 'This device isn’t signed in.' : `${TheHost()} returned an error (${res.status}).`);
    const category: FailureCategory = res.status === 401 ? 'auth' : String(code).startsWith('herdr_') ? 'herdr' : res.status >= 500 ? 'server' : 'client';
    throw new ApiError(res.status, code, message, category, data);
  }
  return data as T;
}

/** Upload with progress and cancellation (fetch has no upload progress). An update waits for it. */
export function upload(path: string, blob: Blob, onProgress: (sent: number, total: number) => void, signal: AbortSignal): Promise<any> {
  const release = holdUpdates('the upload finishes');
  return new Promise<any>((resolve, reject) => {
    const xhr = new XMLHttpRequest();
    xhr.open('POST', path);
    xhr.withCredentials = true;
    xhr.upload.onprogress = (e) => onProgress(e.loaded, e.total || blob.size);
    xhr.onload = () => {
      let data: any = {};
      try {
        data = JSON.parse(xhr.responseText);
      } catch {}
      if (xhr.status >= 200 && xhr.status < 300) resolve(data);
      else reject(new ApiError(xhr.status, data.error || 'upload_failed', data.message || `Upload failed (${xhr.status}).`, xhr.status === 401 ? 'auth' : 'server'));
    };
    xhr.onerror = () => reject(new ApiError(0, 'network', `The upload lost its connection to ${theHost()}.`, 'network'));
    xhr.onabort = () => reject(new ApiError(0, 'cancelled', 'Upload cancelled.', 'client'));
    signal.addEventListener('abort', () => xhr.abort());
    xhr.send(blob);
  }).finally(release);
}

export function wsUrl(path: string): string {
  return `${location.protocol === 'https:' ? 'wss' : 'ws'}://${location.host}${path}`;
}

export function randomId(n = 12): string {
  return Array.from(crypto.getRandomValues(new Uint8Array(n)), (b) => 'abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-_'[b & 63]).join('');
}
