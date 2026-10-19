// Whether Briardr opens offline on this device: the service worker's registration and its saved shell
// (sw.ts), whether the browser keeps Briardr's storage when space runs low, and a repair that rebuilds the
// shell. Repair only touches the service worker and its caches: notes, drafts, pairing (a cookie),
// preferences and cached screens stay.
import { record } from './diag.ts';
import { standalone } from './install.ts';
import { state } from './reactive.svelte.ts';

export type OfflineFailure =
  /** The browser refused the service worker: often an HTTPS certificate this device doesn't fully trust. */
  | 'untrusted'
  | 'register'
  /** The shell didn't install (a file failed to download, or a deployment happened meanwhile). */
  | 'install'
  /** The shell is missing files (the browser cleared some). */
  | 'incomplete';

export type OfflineState =
  | { state: 'unsupported' }
  | { state: 'preparing' }
  | { state: 'ready'; version: string }
  | { state: 'failed'; failure: OfflineFailure };

export const offline = state<OfflineState>('serviceWorker' in navigator ? { state: 'preparing' } : { state: 'unsupported' });
/** Whether the browser keeps Briardr's storage under pressure: true, false, or null (can't tell). */
export const persistent = state<boolean | null>(null);

function fail(failure: OfflineFailure) {
  if (offline.value.state !== 'failed' || offline.value.failure !== failure) record('offline_failed', { reason: failure });
  offline.value = { state: 'failed', failure };
}

/** Asks the active service worker whether its shell is complete. */
function shellStatus(worker: ServiceWorker): Promise<{ version: string; missing: number } | null> {
  return new Promise((resolve) => {
    const channel = new MessageChannel();
    const timer = setTimeout(() => resolve(null), 5000);
    channel.port1.onmessage = (e) => {
      clearTimeout(timer);
      resolve(e.data);
    };
    worker.postMessage({ t: 'status' }, [channel.port2]);
  });
}

let reg: ServiceWorkerRegistration | null = null;
let listening = false;

/** Brings the readiness state up to date. */
export async function checkOffline() {
  if (!reg) return;
  const installing = reg.installing || reg.waiting;
  if (!reg.active) return void (offline.value = installing ? { state: 'preparing' } : offline.value);
  const s = await shellStatus(reg.active);
  if (!s) return;
  if (s.missing === 0) offline.value = { state: 'ready', version: s.version };
  else if (installing) offline.value = { state: 'preparing' };
  else fail('incomplete');
}

function watch(worker: ServiceWorker | null) {
  worker?.addEventListener('statechange', () => {
    if (worker.state === 'redundant' && !reg?.active) fail('install');
    else if (worker.state === 'redundant') record('offline_failed', { reason: 'install' }); // the previous shell stays in use
    void checkOffline();
  });
}

/** Registers the service worker and follows its installation. */
export async function registerShell() {
  if (!('serviceWorker' in navigator)) return;
  try {
    reg = await navigator.serviceWorker.register('/sw.js', { scope: '/' });
  } catch (e) {
    return fail(e instanceof DOMException && e.name === 'SecurityError' ? 'untrusted' : 'register');
  }
  // Some browsers (and automation with service workers turned off) resolve without a registration.
  if (!reg) return void (offline.value = { state: 'unsupported' });
  const r = reg;
  watch(r.installing || r.waiting);
  r.addEventListener('updatefound', () => watch(r.installing));
  if (!listening) navigator.serviceWorker.addEventListener('controllerchange', () => void checkOffline());
  listening = true;
  await checkOffline();
  // An installed app asks to keep its storage (browsers decide; a refusal changes nothing else).
  if (standalone) void keepStorage(false);
  else void navigator.storage?.persisted?.().then((p) => (persistent.value = p), () => {});
}

/** Asks the browser to keep Briardr's storage when space runs low. `asked`: from a button. */
export async function keepStorage(asked = true): Promise<boolean | null> {
  if (!navigator.storage?.persist) return (persistent.value = null);
  try {
    const kept = (await navigator.storage.persisted()) || (await navigator.storage.persist());
    if (asked || !kept) record('storage_persist', { mode: asked ? 'asked' : 'auto', kind: kept ? 'granted' : 'denied' });
    return (persistent.value = kept);
  } catch {
    return (persistent.value = null);
  }
}

/** Storage in use and available to Briardr on this device, in bytes, where the browser says. */
export async function storageEstimate(): Promise<{ usage: number; quota: number } | null> {
  try {
    const e = await navigator.storage?.estimate?.();
    return e?.usage !== undefined && e.quota !== undefined ? { usage: e.usage, quota: e.quota } : null;
  } catch {
    return null;
  }
}

/**
 * Rebuilds the offline app: removes the service worker and its caches (the shell and plugin files, never
 * anything else), registers it again and waits for the new shell. Resolves with the outcome.
 */
export async function repairOffline(): Promise<OfflineState> {
  if (!('serviceWorker' in navigator)) return offline.value;
  record('offline_repair', {});
  offline.value = { state: 'preparing' };
  try {
    for (const r of await navigator.serviceWorker.getRegistrations()) await r.unregister();
    for (const k of await caches.keys()) if (/^(briardr|kelpie)-(shell-|plugins$)/.test(k)) await caches.delete(k);
  } catch {}
  reg = null;
  await registerShell();
  // Wait for the new shell (or its failure), up to a minute on a slow connection.
  for (let i = 0; i < 120 && offline.value.state === 'preparing'; i++) {
    await new Promise((r) => setTimeout(r, 500));
    await checkOffline();
  }
  return offline.value;
}
