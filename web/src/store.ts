// App state. Server-sourced state carries its "as of" time; device-only preferences, caches,
// seen markers and review checkmarks persist in this browser only.
import type { Snippet } from 'svelte';
import { derived, load, persisted, save, State, state } from './reactive.svelte.ts';
import type { Group, HerdrHealth, HostCaps, MacSummary, Model } from './types.ts';

export { persisted };

// ---------- session & connection ----------
export type SessionState = { status: 'unknown' } | { status: 'signed_out'; reason?: string } | { status: 'signed_in'; device: { id: string; name: string } };
/** The session; whether this device was signed in is remembered, so a relaunch opens on the cached state. */
class Session extends State<SessionState> {
  override get value(): SessionState {
    return super.value;
  }
  override set value(v: SessionState) {
    super.value = v;
    if (v.status !== 'unknown') save('briardr.cache.signedIn', v.status === 'signed_in');
  }
}
export const session = new Session({ status: 'unknown' });
export const wasSignedIn = load<boolean>('briardr.cache.signedIn', false);

export type ConnState = 'connecting' | 'online' | 'offline' | 'stopped';
export interface ConnError {
  category: 'auth' | 'protocol' | 'network' | 'herdr' | 'server';
  message: string;
}
type Conn = { state: ConnState; error: ConnError | null; retryAt: number | null; lastOnlineAt: number | null };
/** The connection; when it was last online is remembered across relaunches. */
class Connection extends State<Conn> {
  override get value(): Conn {
    return super.value;
  }
  override set value(v: Conn) {
    super.value = v;
    save('briardr.cache.lastOnline', v.lastOnlineAt);
  }
}
export const conn = new Connection({ state: 'connecting', error: null, retryAt: null, lastOnlineAt: load<number | null>('briardr.cache.lastOnline', null) });
save('briardr.cache.lastOnline', conn.value.lastOnlineAt);

/** serverNow - Date.now(), so "as of" times use the Mac's clock. */
export const clockSkew = state(0);
export const now = state(Date.now());
setInterval(() => (now.value = Date.now()), 1000);

// ---------- cached Mac state ----------
// Saved at most once a second: the model is about 150 KB of JSON at 500 streams, and live updates can
// replace it many times a second.
export const model = persisted<Model | null>('briardr.cache.model', null, 1000);
export const herdr = persisted<HerdrHealth | null>('briardr.cache.herdr', null, 1000);
export const mac = persisted<MacSummary | null>('briardr.cache.mac', null, 1000);
/** What the host supports (host.ts); before sign-in only its noun is known. */
export const host = persisted<Partial<HostCaps> | null>('briardr.cache.host', null);
export const live = derived(() => conn.value.state === 'online');
/** True when what's on screen may be out of date. */
export const stale = derived(() => conn.value.state !== 'online' || !herdr.value?.ok);

// ---------- device-only preferences ----------
export const prefs = persisted('briardr.prefs', {
  fontSize: 13,
  bracketedPaste: true,
  screenReaderTerminal: false,
  lowBatteryInApp: true,
});
/** Files: line wrapping (null: on for phones, off otherwise) and the width of the folder pane. */
export const agentFilter = persisted<Group | null>('briardr.agents.filter', null);
/** Agents groups showing every row instead of the first few (Needs you always shows every row). */
export const expanded = persisted<Record<string, boolean>>('briardr.agents.expanded', {});
export const expandedNodes = persisted<Record<string, boolean>>('briardr.tree.expanded', {});
export const lastPath = persisted<string>('briardr.nav.last', '/agents');
export const listPane = persisted<'agents' | 'workspaces'>('briardr.nav.listPane', 'agents');

/** Local "seen" markers for finished work: streamId -> the state marker that was seen. Never sent to Herdr. */
export const seen = persisted<Record<string, string>>('briardr.seen', {});
export function seenKey(s: { terminalId: string; stateChangeSeq: number | null; stateSince: number | null }) {
  return `${s.terminalId}|${s.stateChangeSeq ?? s.stateSince ?? 0}`;
}

/** When this device last opened each stream (Mac clock, ms): orders Jump search by recent activity. */
export const lastOpened = persisted<Record<string, number>>('briardr.lastOpened', {});
export function markOpened(id: string) {
  const next = { ...lastOpened.value, [id]: Date.now() + clockSkew.value };
  const ids = Object.keys(next);
  if (ids.length > 300) for (const k of ids.sort((a, b) => next[a] - next[b]).slice(0, ids.length - 300)) delete next[k];
  lastOpened.value = next;
}

/** In-app banners (e.g. a stream that isn't on screen now needs you). */
export interface Banner {
  id: string;
  kind: 'attention' | 'battery' | 'info' | 'error';
  title: string;
  body?: string;
  streamId?: string;
}
export const banners = state<Banner[]>([]);
/** Content a screen shows in the overlay above the header (rendered there by App, with the screen's own state). */
export const topOverlay = state<Snippet | null>(null);
export function pushBanner(b: Banner, ttl = 6000) {
  banners.value = [...banners.value.filter((x) => x.id !== b.id), b].slice(-3);
  if (ttl) setTimeout(() => dismissBanner(b.id), ttl);
}
export function dismissBanner(id: string) {
  banners.value = banners.value.filter((b) => b.id !== id);
}

export const toasts = state<{ id: number; text: string }[]>([]);
let toastId = 0;
export function toast(text: string, ms = 2600) {
  const id = ++toastId;
  toasts.value = [...toasts.value, { id, text }];
  setTimeout(() => (toasts.value = toasts.value.filter((t) => t.id !== id)), ms);
}

/** The stream currently on screen (for deciding whether to show attention banners). */
export const visibleStreamId = state<string | null>(null);
/** The open stream fills the window: no header, surface tabs, sidebar or list beside it. Leaving the stream ends it. */
export const fullScreen = state(false);

/** Whether a physical keyboard is in use (Mac, or iPad with a keyboard). */
export const hardwareKeyboard = state<boolean>(matchMedia('(pointer: fine)').matches && !('ontouchstart' in window && navigator.maxTouchPoints > 1));
