// Updates. A reconnect to the same process is a network recovery; a new process or a new build
// refreshes the app: the new offline shell is installed first, then the page reloads (keeping its
// route) as soon as nothing would be lost. While an upload runs, a sheet holds text that isn't saved
// anywhere, or the terminal was typed in moments ago, the reload waits and the "Update ready" bar
// (UpdateBar.svelte) offers it. The process is remembered across offline launches in this tab, without
// coupling separate tabs.
import { record } from './diag.ts';
import { state } from './reactive.svelte.ts';

function stored(key: string): string | null {
  try { return sessionStorage.getItem(key); } catch { return null; }
}
function store(key: string, value: string) {
  try { sessionStorage.setItem(key, value); } catch {}
}
let instance = stored('briardr.serverInstance');
let checking = false;
let checkedInstance: string | null = null;
const current = () => document.querySelector('meta[name="briardr-version"]')?.getAttribute('content') ?? null;

export function rememberServer(id: unknown) {
  if (typeof id !== 'string' || !id) return;
  instance = id;
  store('briardr.serverInstance', id);
}

// ---------- work a reload would lose ----------

/** When reloading won't lose work, or null if it won't now: a phrase that follows "It reloads once…". */
type Check = () => string | null;
const checks = new Set<Check>();

/** Holds back automatic updates while `check` returns a reason ("the upload finishes"). */
export function deferUpdatesWhile(check: Check): () => void {
  checks.add(check);
  return () => checks.delete(check);
}

/** Holds back automatic updates until the returned function is called. */
export function holdUpdates(reason: string): () => void {
  return deferUpdatesWhile(() => reason);
}

// Text typed into a field that doesn't keep its own draft (marked data-keeps-draft, like a note) is lost
// by a reload. The terminal's own input field doesn't count; recent typing there is checked separately.
const edited = new Set<HTMLInputElement | HTMLTextAreaElement>();
document.addEventListener(
  'input',
  (e) => {
    const t = e.target;
    if (!(t instanceof HTMLTextAreaElement || (t instanceof HTMLInputElement && /^(text|search|url|email)$/.test(t.type)))) return;
    if (!t.closest('[data-keeps-draft], .xterm')) edited.add(t);
  },
  true,
);
deferUpdatesWhile(() => {
  for (const f of edited) {
    if (!f.isConnected || !f.value) edited.delete(f);
    else return 'you’re done with the text you typed';
  }
  return null;
});

function busy(): string | null {
  for (const c of checks) {
    const reason = c();
    if (reason) return reason;
  }
  return null;
}

// ---------- applying an update ----------

export interface PendingUpdate {
  reason: 'server_restart' | 'new_build';
  /** Why it waits, while it does. */
  waiting: string | null;
}
/** An update that's installed and waiting for this page to reload. */
export const updateReady = state<PendingUpdate | null>(null);
let pending: { target: string; reason: PendingUpdate['reason'] } | null = null;
let poll: ReturnType<typeof setInterval> | null = null;

/** Reloads now, after the update's target is marked as tried (no reload loop). */
export function reloadNow() {
  if (!pending) return location.reload();
  store('briardr.lastAutoReload', pending.target);
  record('app_reload', { reason: pending.reason, mode: updateReady.value ? 'deferred' : 'auto' });
  pending = null;
  if (poll) clearInterval(poll);
  poll = null;
  location.reload();
}

function applyWhenIdle() {
  const waiting = busy();
  if (!waiting) return reloadNow();
  if (!updateReady.value) record('app_reload_deferred', { reason: pending!.reason });
  updateReady.value = { reason: pending!.reason, waiting };
  poll ??= setInterval(() => pending && applyWhenIdle(), 2000);
}

/** An update for this tab: `target` names it, so a tab reloads for it at most once. */
function offer(target: string, reason: PendingUpdate['reason']): boolean {
  // A stale proxy or failed deployment must not cause a reload loop.
  if (stored('briardr.lastAutoReload') === target) return false;
  pending = { target, reason };
  applyWhenIdle();
  return true;
}

/** Finish installing the new offline shell before navigation uses its shared plugin runtime. */
async function updateShell() {
  if (!('serviceWorker' in navigator)) return;
  const registration = await navigator.serviceWorker.getRegistration('/');
  if (!registration) return;
  await registration.update();
  const worker = registration.installing || registration.waiting;
  if (!worker || worker.state === 'activated') return;
  await new Promise<void>((resolve, reject) => {
    const finish = (error?: Error) => {
      clearTimeout(timer);
      worker.removeEventListener('statechange', changed);
      error ? reject(error) : resolve();
    };
    const changed = () => {
      if (worker.state === 'activated') finish();
      else if (worker.state === 'redundant') finish(new Error('App shell installation failed'));
    };
    const timer = setTimeout(() => finish(new Error('App shell installation timed out')), 15_000);
    worker.addEventListener('statechange', changed);
    changed();
  });
}

export async function checkForUpdate(id: unknown, reconnect = false) {
  if (typeof id !== 'string' || !id || checking || pending || (!reconnect && checkedInstance === id)) return;
  checkedInstance = null;
  checking = true;
  try {
    // Also catches an old cached shell launched offline, even without a previous server ID.
    const response = await fetch('/', { cache: 'no-store', signal: AbortSignal.timeout(8000) });
    if (!response.ok) return;
    const html = new DOMParser().parseFromString(await response.text(), 'text/html');
    const version = html.querySelector('meta[name="briardr-version"]')?.getAttribute('content');
    if (!version) return;
    const restarted = instance !== null && instance !== id;
    if (!restarted && version === current()) {
      rememberServer(id);
      checkedInstance = id;
      return;
    }
    if (stored('briardr.lastAutoReload') === `${id}:${version}`) {
      checkedInstance = id;
      return;
    }
    // If the new shell doesn't install, this build stays in use; the next reconnect tries again.
    await updateShell().catch((e) => {
      record('shell_install_failed', { error: /timed out/.test(String(e)) ? 'timeout' : 'redundant' });
      throw e;
    });
    rememberServer(id);
    checkedInstance = id;
    offer(`${id}:${version}`, restarted ? 'server_restart' : 'new_build');
  } catch {
    // Stay usable when the server or shell is temporarily unavailable; the next reconnect retries.
  } finally {
    checking = false;
  }
}

/** Code this page needs failed to load: maybe a newer build replaced it (more than one deployment ago). */
export function codeMissing() {
  void checkForUpdate(instance, true);
}

/** The service worker activated a build: pages on another one reload into it (same rules as above). */
export function shellActivated(version: unknown) {
  if (typeof version !== 'string' || !version || version === current() || pending) return;
  offer(`shell:${version}`, 'new_build');
}
