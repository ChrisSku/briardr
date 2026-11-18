// The live connection to the Mac: workspace/agent state, Mac summary, attention transitions.
import { api, ApiError, wsUrl } from './api.ts';
import { onNetworkChange } from './backoff.ts';
import { networkKind, record } from './diag.ts';
import { PROTOCOL, ReconnectingSocket } from './socket.ts';
import { clockSkew, conn, herdr, host, mac, model, pushBanner, session, visibleStreamId, prefs } from './store.ts';
import type { Model } from './types.ts';
import { theHost } from './host.ts';
import { checkForUpdate } from './updates.ts';

type Listener = (msg: any) => void;

class Live {
  private socket = new ReconnectingSocket({ name: 'live', maxWait: 30_000, reconnect: () => this.connect() });
  private stopped = false;
  private openedAt = 0;
  private serverInstance: unknown = null;
  private listeners = new Set<Listener>();

  private started = false;

  start() {
    if (this.started) return this.retryNow(); // e.g. paired again after being signed out
    this.started = true;
    this.stopped = false;
    onNetworkChange((kind) => record('network_change', { kind, net: networkKind() })); // the socket reconnects by itself
    document.addEventListener('visibilitychange', () => {
      if (document.visibilityState === 'visible' && !this.stopped) {
        if (conn.value.state !== 'online') this.socket.retryNow();
        else this.socket.ping(); // verify the connection survived the background quickly
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
    this.socket.close();
    record('live_stopped', { category });
    conn.value = { ...conn.value, state: 'stopped', error: { category, message }, retryAt: null };
    if (category === 'auth') session.value = { status: 'signed_out', reason: message };
  }

  retryNow() {
    this.stopped = false;
    this.socket.retryNow();
  }

  private connect() {
    if (this.socket.connected) return;
    conn.value = { ...conn.value, state: 'connecting', retryAt: null };
    const t0 = performance.now();
    record('live_connect', { attempt: this.socket.attempts, net: networkKind() });
    this.socket.open(wsUrl(`/ws/live?v=${PROTOCOL}`), {
      opened: () => {
        this.openedAt = Date.now();
        this.socket.up();
        record('live_open', { ms: Math.round(performance.now() - t0) });
        conn.value = { state: 'online', error: null, retryAt: null, lastOnlineAt: Date.now() };
      },
      message: (msg) => this.onMessage(msg),
      lost: (code) => {
        record('live_close', { code, held_s: this.openedAt ? Math.round((Date.now() - this.openedAt) / 1000) : 0 });
        this.openedAt = 0;
        if (code === 4401) return this.stop('auth', 'This device was signed out of Briardr.');
        if (code === 4400) return this.stop('protocol', `Briardr on ${theHost()} was updated. Reload to continue.`);
        this.afterFailure();
      },
    });
  }

  private async afterFailure() {
    if (this.stopped) return;
    // A failed upgrade hides the HTTP status; ask the API why, so auth problems stop retrying.
    try {
      await api('GET', '/api/session', undefined, { timeout: 5000 });
    } catch (e) {
      if (e instanceof ApiError && e.category === 'auth') return this.stop('auth', 'This device isn’t signed in to Briardr.');
    }
    const delay = this.socket.retry();
    conn.value = { ...conn.value, state: 'offline', error: { category: 'network', message: navigator.onLine ? `Can’t reach ${theHost()}.` : 'This device is offline.' }, retryAt: Date.now() + delay };
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
