// The live connection to the Mac: workspace/agent state, Mac summary, attention transitions.
import { api, ApiError, wsUrl } from './api.ts';
import { Backoff, onNetworkChange } from './backoff.ts';
import { networkKind, record } from './diag.ts';
import { clockSkew, conn, herdr, host, mac, model, pushBanner, session, visibleStreamId, prefs } from './store.ts';
import type { Model } from './types.ts';
import { theHost } from './host.ts';
import { checkForUpdate } from './updates.ts';

export const PROTOCOL = 1;
const PING_EVERY = 10_000;
const PONG_DEADLINE = 5_000;

type Listener = (msg: any) => void;

class Live {
  private ws: WebSocket | null = null;
  private backoff = new Backoff(500, 30_000);
  private retryTimer: number | null = null;
  private pingTimer: number | null = null;
  private pongTimer: number | null = null;
  private stopped = false;
  private openedAt = 0;
  private serverInstance: unknown = null;
  private listeners = new Set<Listener>();

  private started = false;

  start() {
    if (this.started) return this.retryNow(); // e.g. paired again after being signed out
    this.started = true;
    this.stopped = false;
    onNetworkChange((kind) => {
      record('network_change', { kind, net: networkKind() });
      this.backoff.reset();
      if (conn.value.state !== 'online' && !this.stopped) this.connectNow();
    });
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible' && !this.stopped) {
        this.backoff.reset();
        if (conn.value.state !== 'online') this.connectNow();
        else this.ping(); // verify the connection survived the background quickly
      }
    });
    this.connect();
  }

  on(fn: Listener) {
    this.listeners.add(fn);
    return () => this.listeners.delete(fn);
  }

  /** Stop retrying after an authentication or protocol failure; the UI shows the recovery action. */
  private stop(category: 'auth' | 'protocol', message: string) {
    this.stopped = true;
    this.clearTimers();
    record('live_stopped', { category });
    conn.value = { ...conn.value, state: 'stopped', error: { category, message }, retryAt: null };
    if (category === 'auth') session.value = { status: 'signed_out', reason: message };
  }

  retryNow() {
    this.stopped = false;
    this.backoff.reset();
    this.connectNow();
  }

  private connectNow() {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    this.connect();
  }

  private clearTimers() {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    if (this.pingTimer) clearInterval(this.pingTimer);
    if (this.pongTimer) clearTimeout(this.pongTimer);
    this.retryTimer = this.pingTimer = this.pongTimer = null;
  }

  private connect() {
    if (this.ws && (this.ws.readyState === WebSocket.OPEN || this.ws.readyState === WebSocket.CONNECTING)) return;
    conn.value = { ...conn.value, state: 'connecting', retryAt: null };
    const t0 = performance.now();
    record('live_connect', { attempt: this.backoff.attempts, net: networkKind() });
    const ws = new WebSocket(wsUrl(`/ws/live?v=${PROTOCOL}`));
    this.ws = ws;
    ws.onopen = () => {
      this.openedAt = Date.now();
      this.backoff.reset();
      record('live_open', { ms: Math.round(performance.now() - t0) });
      conn.value = { state: 'online', error: null, retryAt: null, lastOnlineAt: Date.now() };
      this.pingTimer = window.setInterval(() => this.ping(), PING_EVERY);
    };
    ws.onmessage = (e) => {
      let msg: any;
      try {
        msg = JSON.parse(e.data);
      } catch {
        return;
      }
      this.onMessage(msg);
    };
    ws.onclose = (e) => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.clearTimers();
      record('live_close', { code: e.code, held_s: this.openedAt ? Math.round((Date.now() - this.openedAt) / 1000) : 0 });
      this.openedAt = 0;
      if (e.code === 4401) return this.stop('auth', 'This device was signed out of Briardr.');
      if (e.code === 4400) return this.stop('protocol', `Briardr on ${theHost()} was updated. Reload to continue.`);
      this.afterFailure();
    };
  }

  private async afterFailure() {
    if (this.stopped) return;
    // A failed upgrade hides the HTTP status; ask the API why, so auth problems stop retrying.
    try {
      await api('GET', '/api/session', undefined, { timeout: 5000 });
    } catch (e) {
      if (e instanceof ApiError && e.category === 'auth') return this.stop('auth', 'This device isn’t signed in to Briardr.');
    }
    const delay = this.backoff.next();
    record('live_retry', { delay_ms: delay, net: networkKind() });
    conn.value = { ...conn.value, state: 'offline', error: { category: 'network', message: navigator.onLine ? `Can’t reach ${theHost()}.` : 'This device is offline.' }, retryAt: Date.now() + delay };
    this.retryTimer = window.setTimeout(() => {
      this.retryTimer = null;
      this.connect();
    }, delay);
  }

  private ping() {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return;
    try {
      ws.send(JSON.stringify({ t: 'ping', id: Date.now() }));
    } catch {}
    if (this.pongTimer) clearTimeout(this.pongTimer);
    this.pongTimer = window.setTimeout(() => {
      record('live_health_fail', { net: networkKind() });
      ws.close(); // a dead network must never look live
      if (this.ws === ws) {
        this.ws = null;
        this.clearTimers();
        this.afterFailure();
      }
    }, PONG_DEADLINE);
  }

  private onMessage(msg: any) {
    if (msg.t === 'hello') {
      this.serverInstance = msg.instanceId;
      void checkForUpdate(msg.instanceId, true);
      return;
    }
    if (msg.t === 'pong') {
      // Retry a shell check that failed while this tab was coming back online.
      void checkForUpdate(this.serverInstance);
      if (this.pongTimer) clearTimeout(this.pongTimer);
      this.pongTimer = null;
      if (typeof msg.now === 'number') clockSkew.value = msg.now - Date.now();
      return;
    }
    if (msg.t === 'model') {
      if (typeof msg.serverNow === 'number') clockSkew.value = msg.serverNow - Date.now();
      herdr.value = msg.herdr;
      if (msg.model) model.value = msg.model as Model;
    } else if (msg.t === 'mac') {
      mac.value = msg.mac;
      if (msg.host) host.value = msg.host;
    } else if (msg.t === 'transition') {
      if (msg.to === 'blocked' && msg.kind === 'agent' && visibleStreamId.value !== msg.streamId) {
        const s = model.value?.streams.find((x) => x.id === msg.streamId);
        pushBanner({ id: `att:${msg.streamId}`, kind: 'attention', title: `${s?.name || 'An agent'} needs you`, streamId: msg.streamId });
      }
    } else if (msg.t === 'battery') {
      if (prefs.value.lowBatteryInApp) pushBanner({ id: 'battery', kind: 'battery', title: msg.sleeping ? 'Mac battery low: going to sleep' : `Mac battery at ${msg.percent}%`, body: msg.sleeping ? `${msg.percent}% left. The Mac sleeps in 30 seconds unless it’s plugged in.` : `${msg.percent}% left.` }, msg.sleeping ? 30_000 : 10_000);
    }
    this.listeners.forEach((l) => l(msg));
  }
}

export const liveConn = new Live();
