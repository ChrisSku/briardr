// Notifications on this device: three things must agree for alerts to arrive: the permission, this
// browser's push subscription, and the subscription the host sends to. They drift apart (permission
// changed in system settings, a subscription that expired, a host that lost or replaced its key), so
// they're compared on launch, on return to the app and when Settings opens, and repaired when that needs
// no permission prompt. Permission is only ever requested from a button.
import { api } from './api.ts';
import { record } from './diag.ts';
import { standalone } from './install.ts';
import { persisted, state } from './reactive.svelte.ts';

export type PushProblem =
  /** Notifications are blocked for Briardr in system or browser settings. */
  | 'blocked'
  /** Permission was reset to "ask", so alerts can't be shown until it's allowed again. */
  | 'permission'
  /** This browser has no subscription (it expired, or the browser dropped it). */
  | 'expired'
  /** The host has a different push key than this subscription was made for. */
  | 'key_changed';

export type PushStatus =
  | { state: 'unsupported' }
  /** iPhone and iPad deliver web alerts only to Home Screen apps. */
  | { state: 'needs_install' }
  /** Not checked yet, or the host couldn't be reached to compare. */
  | { state: 'unknown' }
  | { state: 'off' }
  | { state: 'on' }
  | { state: 'broken'; problem: PushProblem };

export const pushStatus = state<PushStatus>({ state: 'unknown' });
/** Whether the user turned notifications on here (null: not known, from before this was recorded). */
const wanted = persisted<boolean | null>('briardr.push.wanted', null);

export const pushSupported = 'serviceWorker' in navigator && 'PushManager' in window && 'Notification' in window;
const isApple = /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
let publicKey = '';

function keyBytes(b64: string): Uint8Array<ArrayBuffer> {
  const pad = '='.repeat((4 - (b64.length % 4)) % 4);
  const raw = atob((b64 + pad).replace(/-/g, '+').replace(/_/g, '/'));
  const out = new Uint8Array(new ArrayBuffer(raw.length));
  for (let i = 0; i < raw.length; i++) out[i] = raw.charCodeAt(i);
  return out;
}

function sameKey(sub: PushSubscription, key: string): boolean {
  const used = sub.options?.applicationServerKey;
  if (!used || !key) return true; // the browser doesn't say: assume it matches
  const a = new Uint8Array(used);
  const b = keyBytes(key);
  return a.length === b.length && a.every((x, i) => x === b[i]);
}

async function registration(): Promise<ServiceWorkerRegistration | null> {
  return (await navigator.serviceWorker.getRegistration('/')) ?? null;
}

/** This browser's subscription. Asked only when it matters, and never waited on for long (a browser
 *  without a push service may not answer). */
async function localSubscription(): Promise<PushSubscription | null> {
  const reg = await registration();
  if (!reg) return null;
  return Promise.race([reg.pushManager.getSubscription(), new Promise<never>((_, reject) => setTimeout(() => reject(new Error('timeout')), 5000))]);
}

async function register(sub: PushSubscription) {
  await api('POST', '/api/notifications/subscribe', { subscription: sub.toJSON() });
}

let checking: Promise<void> | null = null;

/** Compares the three and repairs what can be repaired without asking. */
export function checkPush(): Promise<void> {
  return (checking ??= reconcile().finally(() => (checking = null)));
}

async function reconcile() {
  if (!pushSupported) return void (pushStatus.value = { state: 'unsupported' });
  if (isApple && !standalone) return void (pushStatus.value = { state: 'needs_install' });
  let server: { subscribed: boolean; endpoint?: string | null; publicKey: string };
  try {
    server = await api('GET', '/api/notifications', undefined, { timeout: 8000 });
  } catch {
    if (pushStatus.value.state !== 'unknown') return; // keep what was known
    return void (pushStatus.value = { state: 'unknown' });
  }
  publicKey = server.publicKey;
  // Before this was recorded, the host knowing a subscription meant the user turned them on.
  wanted.value ??= server.subscribed;
  const permission = Notification.permission;
  const set = (s: PushStatus) => {
    const before = pushStatus.value;
    if (s.state === 'broken' && (before.state !== 'broken' || before.problem !== s.problem)) record('push_problem', { problem: s.problem });
    pushStatus.value = s;
  };

  if (!wanted.value) {
    // Turned off here but the host still sends (turning off didn't finish): finish it.
    if (server.subscribed) await api('DELETE', '/api/notifications/subscribe').catch(() => {});
    return set({ state: 'off' });
  }
  if (permission === 'denied') return set({ state: 'broken', problem: 'blocked' });
  if (permission !== 'granted') return set({ state: 'broken', problem: 'permission' });
  let local: PushSubscription | null;
  try {
    local = await localSubscription();
  } catch {
    return set({ state: 'unknown' });
  }
  if (!local) return set({ state: 'broken', problem: 'expired' });
  if (!sameKey(local, server.publicKey)) return set({ state: 'broken', problem: 'key_changed' });
  if (!server.subscribed || (server.endpoint && server.endpoint !== local.endpoint)) {
    // This browser's subscription is fine, but the host doesn't have it (or has an older one): send it.
    try {
      await register(local);
      record('push_repaired', { reason: server.subscribed ? 'endpoint' : 'missing' });
    } catch {
      return set({ state: 'unknown' });
    }
  }
  set({ state: 'on' });
}

/** Turns notifications on, or repairs them: asks for permission if needed, then subscribes afresh. */
export async function enablePush(): Promise<'on' | 'denied' | 'failed'> {
  try {
    const p = Notification.permission === 'granted' ? 'granted' : await Notification.requestPermission();
    if (p !== 'granted') {
      await checkPush();
      return 'denied';
    }
    const reg = await navigator.serviceWorker.ready;
    if (!publicKey) publicKey = (await api<{ publicKey: string }>('GET', '/api/notifications')).publicKey;
    // A subscription for another key (or one the host lost) is replaced by a new one.
    const old = await reg.pushManager.getSubscription();
    if (old && !sameKey(old, publicKey)) await old.unsubscribe();
    const sub = (old && sameKey(old, publicKey) ? old : null) ?? (await reg.pushManager.subscribe({ userVisibleOnly: true, applicationServerKey: keyBytes(publicKey) }));
    await register(sub);
    wanted.value = true;
    record('push_subscribed', {});
    await checkPush();
    return 'on';
  } catch {
    await checkPush();
    return 'failed';
  }
}

/** Turns notifications off on this device: here and on the host. */
export async function disablePush(): Promise<boolean> {
  wanted.value = false;
  let ok = true;
  try {
    const sub = await localSubscription();
    if (sub && !(await sub.unsubscribe())) ok = false;
  } catch {
    ok = false;
  }
  try {
    await api('DELETE', '/api/notifications/subscribe');
  } catch {
    ok = false; // checkPush finishes it once the host is reachable
  }
  await checkPush();
  return ok;
}

let watching = false;
/** Checks on launch, whenever Briardr comes back to the foreground, and when the permission changes. */
export function watchPush() {
  if (watching || !pushSupported) return;
  watching = true;
  void checkPush();
  document.addEventListener('visibilitychange', () => document.visibilityState === 'visible' && void checkPush());
  navigator.permissions
    ?.query({ name: 'notifications' as PermissionName })
    .then((p) => p.addEventListener('change', () => void checkPush()))
    .catch(() => {});
}
