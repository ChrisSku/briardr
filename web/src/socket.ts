// The WebSocket under the live model (live.ts) and each terminal (terminal/controller.ts): it never looks
// alive when it isn't, and it comes back after it drops. What the messages mean, what a close code does and
// what the screen shows meanwhile stay with the caller.
//
// One connection at a time. open() replaces the connection before it and close() ends it; from then on
// none of that connection's events reach the caller, so nothing late from an old connection is taken for
// the new one's. Nothing is queued: send() while the connection isn't open sends nothing, then or later.
import { Backoff, onNetworkChange } from './backoff.ts';
import { networkKind, record } from './diag.ts';

/** The protocol both sockets speak (`?v=`); the server closes a connection that asks for another with 4400. */
export const PROTOCOL = 1;
/** A connection that's up is pinged this often, and lost when a pong takes longer than PONG_DEADLINE. */
const PING_EVERY = 10_000;
const PONG_DEADLINE = 5_000;

/** One connection's events. */
export interface Connection {
  opened?(): void;
  /** A JSON text message, pongs included. */
  message(msg: any): void;
  /** A binary message. */
  frame?(data: Uint8Array): void;
  /** It ended without close(): with its close code, or null when it stopped answering pings. */
  lost(code: number | null): void;
}

export interface SocketOptions {
  /** Starts its diagnostics' names: `<name>_health_fail`, `<name>_retry`. */
  name: string;
  /** The longest wait between attempts. */
  maxWait: number;
  /** Connects again (with open()) when a retry is due. */
  reconnect: () => void;
}

export class ReconnectingSocket {
  private opts: SocketOptions;
  private backoff: Backoff;
  private ws: WebSocket | null = null;
  private on: Connection | null = null;
  private retryTimer: ReturnType<typeof setTimeout> | null = null;
  private pingTimer: ReturnType<typeof setInterval> | null = null;
  private pongTimer: ReturnType<typeof setTimeout> | null = null;
  /** A retry was asked for and no connection has been up since: a returning network makes it now. */
  private retrying = false;
  private netOff: (() => void) | null = null;
  private onEnd: (() => void)[] = [];

  constructor(opts: SocketOptions) {
    this.opts = opts;
    this.backoff = new Backoff(500, opts.maxWait);
  }

  /** Connecting or open. */
  get connected(): boolean {
    return !!this.ws && this.ws.readyState <= WebSocket.OPEN;
  }

  get isOpen(): boolean {
    return this.ws?.readyState === WebSocket.OPEN;
  }

  /** Failed attempts since a connection was last up. */
  get attempts(): number {
    return this.backoff.attempts;
  }

  /** Connect, replacing the connection before this one, or the wait for it. */
  open(url: string, on: Connection) {
    this.cancelRetry();
    this.end();
    this.netOff ??= onNetworkChange(() => {
      this.backoff.reset();
      if (this.retrying) this.retryNow();
    });
    const ws = new WebSocket(url);
    ws.binaryType = 'arraybuffer';
    ws.onopen = () => on.opened?.();
    ws.onmessage = (e) => {
      if (typeof e.data !== 'string') return on.frame?.(new Uint8Array(e.data));
      let msg: any;
      try {
        msg = JSON.parse(e.data);
      } catch {
        return;
      }
      if (msg?.t === 'pong' && this.pongTimer) {
        clearTimeout(this.pongTimer);
        this.pongTimer = null;
      }
      on.message(msg);
    };
    ws.onclose = (e) => this.lose(e.code);
    this.ws = ws;
    this.on = on;
  }

  /** The connection works (it opened, or the server says it's live): start the waits over, and ping it. */
  up() {
    this.backoff.reset();
    this.retrying = false;
    this.stopPinging();
    this.pingTimer = setInterval(() => this.ping(), PING_EVERY);
  }

  /** Check now that the connection still answers. */
  ping() {
    if (!this.send({ t: 'ping', id: Date.now() })) return;
    if (this.pongTimer) clearTimeout(this.pongTimer);
    this.pongTimer = setTimeout(() => {
      // A dead network must never look live.
      record(`${this.opts.name}_health_fail`, { net: networkKind() });
      this.lose(null);
    }, PONG_DEADLINE);
  }

  /** Sends msg as JSON if the connection is open. Returns false, and keeps nothing, if it isn't. */
  send(msg: object): boolean {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return false;
    ws.send(JSON.stringify(msg));
    return true;
  }

  /** After a loss, connect again after the next wait, which is longer after each failed attempt. Returns the wait. */
  retry(): number {
    const delay = this.backoff.next();
    record(`${this.opts.name}_retry`, { delay_ms: delay, net: networkKind() });
    this.retrying = true;
    this.cancelRetry();
    this.retryTimer = setTimeout(() => {
      this.retryTimer = null;
      this.opts.reconnect();
    }, delay);
    return delay;
  }

  /** Start the waits over and connect again now. */
  retryNow() {
    this.backoff.reset();
    this.cancelRetry();
    this.opts.reconnect();
  }

  /** End the connection, or the wait for the next one: nothing more happens until open(). */
  close() {
    this.cancelRetry();
    this.retrying = false;
    this.end();
  }

  /** close(), and stop listening for the network. */
  dispose() {
    this.close();
    this.netOff?.();
    this.netOff = null;
  }

  /** Resolves when this connection ends (either side closes it, it's lost or replaced), or after ms. */
  ended(ms: number): Promise<void> {
    if (!this.ws) return Promise.resolve();
    return new Promise((resolve) => {
      const timer = setTimeout(resolve, ms);
      this.onEnd.push(() => {
        clearTimeout(timer);
        resolve();
      });
    });
  }

  /** The connection is gone without close(): the caller decides whether to retry(). */
  private lose(code: number | null) {
    const on = this.on!;
    this.retrying = false;
    this.end();
    on.lost(code);
  }

  /** Close the current connection and detach it: none of its events reach the caller any more. */
  private end() {
    const ws = this.ws;
    if (!ws) return;
    ws.onopen = ws.onmessage = ws.onclose = null;
    ws.close(1000);
    this.ws = this.on = null;
    this.stopPinging();
    const ended = this.onEnd;
    this.onEnd = [];
    ended.forEach((f) => f());
  }

  private stopPinging() {
    if (this.pingTimer) clearInterval(this.pingTimer);
    if (this.pongTimer) clearTimeout(this.pongTimer);
    this.pingTimer = this.pongTimer = null;
  }

  private cancelRetry() {
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
  }
}
