// Client-side connection diagnostics, kept on this device across restarts.
// Two rotating segments of ~512 KB each (~1 MB total). Values are restricted to numbers, booleans and
// short slugs, and IDs are hashed, so addresses, names, commands, terminal text and credentials can't
// be recorded.
import './rename.ts'; // before the first read of this device's storage

const KEY_A = 'briardr.diag.0';
const KEY_B = 'briardr.diag.1';
const SEGMENT = 512 * 1024;
const KEY = /^[a-z][a-z0-9_]{0,31}$/;
const SLUG = /^[a-z][a-z0-9_-]{0,31}$/;
const STRING_KEYS = new Set(['category', 'reason', 'kind', 'mode', 'net', 'provider', 'method', 'error', 'problem', 'stream', 'device', 'plugin']); // plugin: which plugin recorded it (PluginContext.record)

let salt = '';
try {
  salt = localStorage.getItem('briardr.diag.salt') || '';
  if (!salt) {
    salt = Array.from(crypto.getRandomValues(new Uint8Array(12)), (b) => b.toString(16).padStart(2, '0')).join('');
    localStorage.setItem('briardr.diag.salt', salt);
  }
} catch {}

/** Small, stable, non-reversible ID (FNV-1a over salt+id). */
export function anon(id: string | null | undefined): string | null {
  if (!id) return null;
  let h = 0x811c9dc5;
  const s = salt + id;
  for (let i = 0; i < s.length; i++) {
    h ^= s.charCodeAt(i);
    h = Math.imul(h, 0x01000193);
  }
  return 'a' + (h >>> 0).toString(16).padStart(8, '0');
}

export function clean(v: unknown, key = 'category'): number | boolean | string | null | undefined {
  if (v === null) return null;
  if (typeof v === 'boolean') return v;
  if (typeof v === 'number') return Number.isFinite(v) ? Math.round(v * 1000) / 1000 : undefined;
  if (typeof v === 'string') return STRING_KEYS.has(key) && SLUG.test(v) ? v : 'redacted';
  return undefined;
}

export function record(ev: string, fields: Record<string, unknown> = {}): void {
  if (!KEY.test(ev)) return;
  const entry: Record<string, unknown> = { t: new Date().toISOString(), src: 'client', ev };
  for (const [k, v] of Object.entries(fields)) {
    if (!KEY.test(k)) continue;
    const c = clean(v, k);
    if (c !== undefined) entry[k] = c;
  }
  const line = JSON.stringify(entry) + '\n';
  try {
    let cur = localStorage.getItem(KEY_B) || '';
    if (cur.length + line.length > SEGMENT) {
      localStorage.setItem(KEY_A, cur);
      cur = '';
    }
    localStorage.setItem(KEY_B, cur + line);
  } catch {}
}

export function exportClientLog(): string {
  try {
    return (localStorage.getItem(KEY_A) || '') + (localStorage.getItem(KEY_B) || '');
  } catch {
    return '';
  }
}

export function networkKind(): string {
  const c = (navigator as any).connection;
  if (!c) return navigator.onLine ? 'online' : 'offline';
  return String(c.type || c.effectiveType || (navigator.onLine ? 'online' : 'offline')).toLowerCase().replace(/[^a-z0-9_-]/g, '');
}
