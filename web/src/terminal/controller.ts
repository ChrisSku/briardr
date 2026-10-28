// TerminalController: one xterm.js view of one Herdr pane, with its live connection.
//
// Ownership: 'controlling' and 'watching' are distinct states. Opening asks for control without
// takeover; if another client controls the pane, the view falls back to watching and the user may
// confirm Take Over. Leaving, locking, or backgrounding releases control; returning reconnects.
//
// No replay: each WebSocket is one generation. Nothing typed, pasted, uploaded or gestured is
// queued while disconnected or connecting; it's dropped (and the user told), never sent later.
import { SerializeAddon } from '@xterm/addon-serialize';
import { Unicode11Addon } from '@xterm/addon-unicode11';
import { WebglAddon } from '@xterm/addon-webgl';
import { Terminal, type ITheme } from '@xterm/xterm';
import { randomId, wsUrl } from '../api.ts';
import { state, type State } from '../reactive.svelte.ts';
import { Backoff, onNetworkChange } from '../backoff.ts';
import { anon, networkKind, record } from '../diag.ts';
import { idbGet, idbSet, type CachedScreen } from '../idb.ts';
import { viewportInset } from '../keyboard.svelte.ts';
import { PROTOCOL } from '../live.ts';
import { activeMods, applyModsToText, consumeMods, fromKeyEvent, type Outgoing } from './keys.ts';
import { linkAt, registerLinkProvider } from './links.ts';
import { theHost, TheHost } from '../host.ts';
import { deferUpdatesWhile } from '../updates.ts';

// An update doesn't reload the app out from under someone typing in a terminal (web/src/updates.ts).
let lastInput = 0;
deferUpdatesWhile(() => (Date.now() - lastInput < 30_000 ? 'you stop typing in the terminal' : null));

export type Ownership =
  | 'idle' // not attached (restored, backgrounded, or not started)
  | 'connecting'
  | 'controlling'
  | 'watching'
  | 'reconnecting'
  | 'gone' // pane no longer exists
  | 'error';

export interface TermStatus {
  ownership: Ownership;
  /** Why we're watching instead of controlling, if relevant. */
  watchReason: 'peek' | 'busy' | 'taken_over' | 'released' | 'chosen' | null;
  message: string | null;
  /** Mac's terminal size as last reported. */
  width: number;
  height: number;
  /** Time the screen content was last confirmed live (ms, device clock). */
  liveAt: number | null;
  cachedAt: number | null;
  retryAt: number | null;
}

const PING_EVERY = 10_000;
const PONG_DEADLINE = 5_000;
const RESIZE_DEBOUNCE = 250;

/** xterm's colours: the --term-* tokens in styles.css (xterm needs the values themselves). */
export function themeFor(dark: boolean): ITheme {
  return dark
    ? { background: '#0a0a09', foreground: '#e7e3de', cursor: '#e7e3de', cursorAccent: '#0a0a09', selectionBackground: '#3a4a6b',
        black: '#1c1c1e', red: '#ff6b61', green: '#6edc9a', yellow: '#f2c46b', blue: '#7ab2ff', magenta: '#d7a1f5', cyan: '#64d2ff', white: '#d1d1d6',
        brightBlack: '#6e6e73', brightRed: '#ff8a80', brightGreen: '#7ee787', brightYellow: '#ffe066', brightBlue: '#8cbcff', brightMagenta: '#e5aaff', brightCyan: '#8fe3ff', brightWhite: '#ffffff' }
    : { background: '#ffffff', foreground: '#1c1917', cursor: '#1c1917', cursorAccent: '#ffffff', selectionBackground: '#b4d2ff',
        black: '#1c1917', red: '#c9372c', green: '#18794a', yellow: '#9a5800', blue: '#1d5fd1', magenta: '#8e3fb8', cyan: '#0b7285', white: '#d1d1d6',
        brightBlack: '#6e6e73', brightRed: '#e0453a', brightGreen: '#2a9d48', brightYellow: '#b07800', brightBlue: '#2f7bf0', brightMagenta: '#a352d6', brightCyan: '#1491a8', brightWhite: '#ffffff' };
}

export interface ControllerOptions {
  streamId: string;
  mode: 'control' | 'observe';
  fontSize: number;
  screenReader: boolean;
  onLink: (url: string) => void;
  onLongPress: (info: { x: number; y: number; col: number; row: number; link: string | null }) => void;
  onNotice: (text: string) => void;
}

export class TerminalController {
  readonly term: Terminal;
  readonly status: State<TermStatus>;
  readonly generation = state(0);
  readonly opts: ControllerOptions;
  private serialize = new SerializeAddon();
  private ws: WebSocket | null = null;
  private clientSession = randomId(10);
  private backoff = new Backoff(500, 15_000);
  private retryTimer: number | null = null;
  private pingTimer: number | null = null;
  private pongTimer: number | null = null;
  private inputId = 0;
  private host: HTMLElement | null = null;
  private screen: HTMLElement | null = null;
  private resizeObs: ResizeObserver | null = null;
  private resizeTimer: number | null = null;
  private lastSentSize = '';
  private wantTakeover = false;
  private wantMode: 'control' | 'observe';
  private saveTimer: number | null = null;
  private disposed = false;
  private netOff: (() => void) | null = null;
  private webgl: WebglAddon | null = null;
  private desired = { cols: 80, rows: 24 };
  private scheme = matchMedia('(prefers-color-scheme: dark)');
  private onScheme = (e: MediaQueryListEvent) => (this.term.options.theme = themeFor(e.matches));

  constructor(opts: ControllerOptions) {
    this.opts = opts;
    this.wantMode = opts.mode;
    this.status = state<TermStatus>({ ownership: 'idle', watchReason: opts.mode === 'observe' ? 'peek' : null, message: null, width: 0, height: 0, liveAt: null, cachedAt: null, retryAt: null });
    this.term = new Terminal({
      scrollback: 0, // Herdr and the program own history; there is no separate local history layer
      fontFamily: 'ui-monospace, "SF Mono", SFMono-Regular, Menlo, Monaco, monospace',
      fontSize: opts.fontSize,
      lineHeight: 1.12,
      cursorBlink: false,
      allowProposedApi: true,
      convertEol: false,
      disableStdin: opts.mode === 'observe',
      screenReaderMode: opts.screenReader,
      macOptionIsMeta: false,
      theme: themeFor(this.scheme.matches),
      linkHandler: {
        activate: (_e, uri) => this.opts.onLink(uri),
        allowNonHttpProtocols: false,
      },
    });
    this.term.loadAddon(this.serialize);
    this.term.loadAddon(new Unicode11Addon());
    this.term.unicode.activeVersion = '11';
    // Removed in dispose(): a listener left on the media query keeps the whole terminal reachable.
    this.scheme.addEventListener('change', this.onScheme);

    // Typed text (on-screen keyboard or hardware keyboard) arrives here.
    this.term.onData((data) => this.onTyped(data));
    this.term.attachCustomKeyEventHandler((e) => this.onKey(e));
    registerLinkProvider(this.term, (url) => this.opts.onLink(url));
  }

  // ---------- view ----------

  mount(host: HTMLElement, screen: HTMLElement) {
    this.host = host;
    this.screen = screen;
    (host as any).briardrTerminal = this; // lets automated UI checks locate cells; no effect otherwise
    this.term.open(screen);
    if (this.opts.mode === 'control') {
      try {
        this.webgl = new WebglAddon();
        this.webgl.onContextLoss(() => {
          this.webgl?.dispose();
          this.webgl = null;
        });
        this.term.loadAddon(this.webgl);
      } catch {
        this.webgl = null;
      }
    }
    const ta = this.term.textarea;
    if (ta) {
      ta.setAttribute('autocapitalize', 'off');
      ta.setAttribute('autocorrect', 'off');
      ta.setAttribute('autocomplete', 'off');
      ta.setAttribute('spellcheck', 'false');
      ta.setAttribute('enterkeyhint', 'enter');
      ta.setAttribute('aria-label', 'Terminal input');
      ta.addEventListener('paste', (e) => this.onPasteEvent(e), true);
    }
    this.resizeObs = new ResizeObserver(() => this.scheduleResize());
    this.resizeObs.observe(host);
    this.loadCache();
  }

  /** Visible cell metrics (CSS px). */
  cell(): { w: number; h: number } {
    const dims = (this.term as any)._core?._renderService?.dimensions?.css?.cell;
    if (dims?.width && dims?.height) return { w: dims.width, h: dims.height };
    return { w: this.opts.fontSize * 0.6, h: this.opts.fontSize * 1.12 * 1.2 };
  }

  /** Columns and rows that fit the visible area (above the on-screen keyboard while it's up). */
  fit(): { cols: number; rows: number } {
    if (!this.host) return this.desired;
    const { w, h } = this.cell();
    const r = this.host.getBoundingClientRect();
    const cols = Math.max(20, Math.floor((r.width - 4) / w));
    const rows = Math.max(6, Math.floor((r.height - 2) / h));
    return { cols, rows };
  }

  private scheduleResize() {
    if (this.resizeTimer) clearTimeout(this.resizeTimer);
    this.resizeTimer = window.setTimeout(() => {
      this.resizeTimer = null;
      // A hidden terminal (another surface is showing) has no real size; never resize the Mac for it.
      const r = this.host?.getBoundingClientRect();
      if (!r || r.width < 80 || r.height < 60) return;
      const want = this.fit();
      this.desired = want;
      // Rotation, a window resize, or the on-screen keyboard opening or closing.
      if (this.status.value.ownership === 'controlling') this.sendResize(want.cols, want.rows);
    }, RESIZE_DEBOUNCE);
  }

  private sendResize(cols: number, rows: number) {
    const key = `${cols}x${rows}`;
    if (key === this.lastSentSize) return;
    this.lastSentSize = key;
    this.send({ t: 'resize', cols, rows });
    record('term_resize', { cols, rows });
  }

  /** Grid position for a point in client coordinates. */
  cellAt(clientX: number, clientY: number): { col: number; row: number } {
    const el = this.term.element?.querySelector('.xterm-screen') as HTMLElement | null;
    const r = (el || this.screen!).getBoundingClientRect();
    const { w, h } = this.cell();
    const scaleX = el ? r.width / (this.term.cols * w) || 1 : 1;
    const scaleY = el ? r.height / (this.term.rows * h) || 1 : 1;
    const col = Math.max(0, Math.min(this.term.cols - 1, Math.floor((clientX - r.left) / (w * scaleX))));
    const row = Math.max(0, Math.min(this.term.rows - 1, Math.floor((clientY - r.top) / (h * scaleY))));
    return { col, row };
  }

  linkAt(col: number, row: number): string | null {
    return linkAt(this.term, col, this.term.buffer.active.viewportY + row);
  }

  setFontSize(px: number) {
    this.opts.fontSize = px;
    this.term.options.fontSize = px;
    this.scheduleResize();
  }

  focus() {
    if (this.status.value.ownership === 'controlling') this.term.focus();
  }

  blur() {
    this.term.blur();
    (this.term.textarea as HTMLTextAreaElement | undefined)?.blur();
  }

  // ---------- cache (offline screens) ----------

  private async loadCache() {
    const c = await idbGet<CachedScreen>(`screen:${this.opts.streamId}`);
    if (!c || this.status.value.liveAt || this.disposed) return;
    this.term.resize(Math.max(2, c.cols), Math.max(2, c.rows));
    this.term.write(c.data);
    this.status.value = { ...this.status.value, cachedAt: c.savedAt, width: c.cols, height: c.rows };
  }

  private scheduleSave() {
    if (this.saveTimer) return;
    this.saveTimer = window.setTimeout(() => {
      this.saveTimer = null;
      this.saveNow();
    }, 2000);
  }

  private saveNow() {
    if (!this.status.value.liveAt) return;
    try {
      const data = this.serialize.serialize({ scrollback: 0 });
      idbSet(`screen:${this.opts.streamId}`, { data, cols: this.term.cols, rows: this.term.rows, savedAt: Date.now() } satisfies CachedScreen);
    } catch {}
  }

  // ---------- connection ----------

  /** Attach (or re-attach). takeover=true only after the user confirmed Take Over. */
  connect(mode: 'control' | 'observe' = this.wantMode, takeover = false) {
    if (this.disposed) return;
    this.wantMode = mode;
    this.wantTakeover = takeover;
    this.closeSocket();
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    if (!this.netOff)
      this.netOff = onNetworkChange(() => {
        this.backoff.reset();
        if (this.status.value.ownership === 'reconnecting') this.connect(this.wantMode, false);
      });
    const gen = this.generation.value + 1;
    this.generation.value = gen;
    const { cols, rows } = this.fit();
    this.desired = { cols, rows };
    this.lastSentSize = mode === 'control' ? `${cols}x${rows}` : '';
    const q = new URLSearchParams({ v: String(PROTOCOL), pane: this.opts.streamId, mode, session: this.clientSession, cols: String(cols), rows: String(rows), ...(takeover ? { takeover: '1' } : {}) });
    const t0 = performance.now();
    this.status.value = { ...this.status.value, ownership: this.status.value.ownership === 'reconnecting' ? 'reconnecting' : 'connecting', message: null, retryAt: null };
    record('term_connect', { mode, takeover, stream: anon(this.opts.streamId), net: networkKind() });
    const ws = new WebSocket(wsUrl(`/ws/term?${q}`));
    ws.binaryType = 'arraybuffer';
    this.ws = ws;
    let gotLive = false;
    ws.onmessage = (e) => {
      if (this.ws !== ws || gen !== this.generation.value) return;
      if (typeof e.data !== 'string') return this.onFrame(new Uint8Array(e.data));
      let msg: any;
      try {
        msg = JSON.parse(e.data);
      } catch {
        return;
      }
      switch (msg.t) {
        case 'live':
          gotLive = true;
          this.backoff.reset();
          record('term_live', { mode: msg.mode, ms: Math.round(performance.now() - t0) });
          this.status.value = {
            ...this.status.value,
            ownership: msg.mode === 'control' ? 'controlling' : 'watching',
            watchReason: msg.mode === 'control' ? null : this.status.value.watchReason || (this.opts.mode === 'observe' ? 'peek' : 'chosen'),
            liveAt: Date.now(),
            width: msg.width,
            height: msg.height,
            message: null,
          };
          this.startPing();
          break;
        case 'pong':
          if (this.pongTimer) clearTimeout(this.pongTimer);
          this.pongTimer = null;
          break;
        case 'closed':
          this.onClosed(msg.category, msg.reason);
          break;
        case 'input_rejected':
          if (msg.reason === 'connecting' || msg.reason === 'not_controlling' || msg.reason === 'watching') this.opts.onNotice('Not sent — this stream isn’t under your control right now.');
          else if (msg.reason !== 'invalid') this.opts.onNotice(`${TheHost()} didn’t accept that input.`);
          break;
      }
    };
    ws.onclose = (e) => {
      if (this.ws !== ws) return;
      this.ws = null;
      this.stopPing();
      if (gen !== this.generation.value || this.disposed) return;
      if (e.code === 4401) {
        this.status.value = { ...this.status.value, ownership: 'error', message: 'This device was signed out of Briardr.' };
        return;
      }
      if (e.code === 4400) {
        this.status.value = { ...this.status.value, ownership: 'error', message: `Briardr on ${theHost()} was updated. Reload to continue.` };
        return;
      }
      const o = this.status.value.ownership;
      if (o === 'gone' || o === 'idle' || o === 'error') return;
      // Network loss or a dead connection: retry with backoff in the same mode, never with takeover.
      record('term_drop', { live: gotLive, code: e.code });
      this.scheduleReconnect();
    };
  }

  private scheduleReconnect() {
    const delay = this.backoff.next();
    record('term_retry', { delay_ms: delay, net: networkKind() });
    this.status.value = { ...this.status.value, ownership: 'reconnecting', retryAt: Date.now() + delay, message: navigator.onLine ? `Reconnecting to ${theHost()}…` : 'This device is offline.' };
    this.retryTimer = window.setTimeout(() => {
      this.retryTimer = null;
      this.connect(this.wantMode, false);
    }, delay);
  }

  retryNow() {
    this.backoff.reset();
    this.connect(this.wantMode, false);
  }

  private onClosed(category: string, reason: string) {
    record('term_closed', { category });
    const s = this.status.value;
    if (category === 'busy') {
      // Another client controls this pane. Don't take it; show it read-only instead.
      this.status.value = { ...s, ownership: 'connecting', watchReason: 'busy', message: reason };
      this.connect('observe', false);
      this.status.value = { ...this.status.value, watchReason: 'busy', message: reason };
      return;
    }
    if (category === 'taken_over') {
      this.status.value = { ...s, ownership: 'connecting', watchReason: 'taken_over', message: reason };
      this.connect('observe', false);
      this.status.value = { ...this.status.value, watchReason: 'taken_over', message: reason };
      return;
    }
    if (category === 'released') {
      this.status.value = { ...s, ownership: 'idle', message: reason };
      return;
    }
    if (category === 'pane_gone') {
      this.status.value = { ...s, ownership: 'gone', message: reason };
      this.closeSocket();
      return;
    }
  }

  /** Hand control back to the Mac (pane returns to its Mac size) and keep watching. */
  async handBack() {
    if (this.status.value.ownership !== 'controlling') return;
    this.send({ t: 'release' });
    record('term_hand_back', {});
    await new Promise((r) => setTimeout(r, 250));
    this.status.value = { ...this.status.value, watchReason: 'released' };
    this.connect('observe', false);
    this.status.value = { ...this.status.value, watchReason: 'released' };
  }

  /** Release and detach (locking, backgrounding). */
  suspend(reason: string, save = true) {
    if (this.status.value.ownership === 'controlling') this.send({ t: 'release' });
    record('term_suspend', { reason });
    if (save) this.saveNow();
    this.generation.value++;
    this.closeSocket();
    if (this.retryTimer) clearTimeout(this.retryTimer);
    this.retryTimer = null;
    this.status.value = { ...this.status.value, ownership: 'idle', retryAt: null };
  }

  /**
   * The view closes (leaving the stream, closing Peek): release control at once, and save the screen
   * and free xterm after the next screen has painted. Those take several milliseconds, which the
   * screen the user goes to shouldn't wait for.
   */
  dispose() {
    if (this.disposed) return;
    this.suspend('leave', false);
    this.disposed = true;
    let freed = false;
    const free = () => {
      if (freed) return;
      freed = true;
      if (this.saveTimer) clearTimeout(this.saveTimer);
      if (this.resizeTimer) clearTimeout(this.resizeTimer);
      this.saveNow();
      this.netOff?.();
      this.scheme.removeEventListener('change', this.onScheme);
      this.resizeObs?.disconnect();
      this.webgl?.dispose();
      this.term.dispose();
    };
    requestAnimationFrame(() => setTimeout(free));
    setTimeout(free, 1000); // no animation frames while the page is hidden
  }

  private closeSocket() {
    this.stopPing();
    const ws = this.ws;
    this.ws = null;
    if (ws && ws.readyState <= WebSocket.OPEN) ws.close(1000);
  }

  private startPing() {
    this.stopPing();
    this.pingTimer = window.setInterval(() => {
      const ws = this.ws;
      if (!ws || ws.readyState !== WebSocket.OPEN) return;
      ws.send(JSON.stringify({ t: 'ping', id: Date.now() }));
      if (this.pongTimer) clearTimeout(this.pongTimer);
      this.pongTimer = window.setTimeout(() => {
        // A dead network must never leave the terminal looking live.
        record('term_health_fail', { net: networkKind() });
        if (this.ws === ws) {
          this.ws = null;
          ws.close();
          this.stopPing();
          this.scheduleReconnect();
        }
      }, PONG_DEADLINE);
    }, PING_EVERY);
  }

  private stopPing() {
    if (this.pingTimer) clearInterval(this.pingTimer);
    if (this.pongTimer) clearTimeout(this.pongTimer);
    this.pingTimer = this.pongTimer = null;
  }

  private onFrame(buf: Uint8Array) {
    if (buf[0] !== 1 || buf.length < 10) return;
    const dv = new DataView(buf.buffer, buf.byteOffset, buf.byteLength);
    const width = dv.getUint16(6);
    const height = dv.getUint16(8);
    if (width && height && (width !== this.term.cols || height !== this.term.rows)) this.term.resize(width, height);
    this.term.write(buf.subarray(10));
    const s = this.status.value;
    if (s.width !== width || s.height !== height || !s.liveAt || Date.now() - s.liveAt > 1000)
      this.status.value = { ...s, width, height, liveAt: Date.now(), cachedAt: null };
    this.scheduleSave();
  }

  // ---------- input (only while controlling; never queued) ----------

  get canInput(): boolean {
    return this.status.value.ownership === 'controlling' && this.ws?.readyState === WebSocket.OPEN;
  }

  private send(msg: { t: string; [k: string]: unknown }): boolean {
    const ws = this.ws;
    if (!ws || ws.readyState !== WebSocket.OPEN) return false;
    ws.send(JSON.stringify(msg));
    if (msg.t === 'input') lastInput = Date.now();
    return true;
  }

  /** Send keystrokes/text. Returns false (and nothing is sent or kept) if not controlling. */
  input(out: Outgoing): boolean {
    if (!this.canInput) {
      this.opts.onNotice(this.status.value.ownership === 'watching' ? 'You’re watching. Take control to type.' : `Not sent — not connected to ${theHost()}.`);
      return false;
    }
    return this.send({ t: 'input', id: ++this.inputId, ...out });
  }

  /** Insert text as one paste. Never followed by Enter. */
  paste(text: string, bracketed: boolean, generation?: number): boolean {
    if (generation !== undefined && generation !== this.generation.value) {
      this.opts.onNotice(`Not inserted: the connection to ${theHost()} changed since you picked it.`);
      return false;
    }
    if (!this.canInput) {
      this.opts.onNotice('Not inserted — you aren’t controlling this stream.');
      return false;
    }
    return this.send({ t: 'input', id: ++this.inputId, paste: { text, bracketed } });
  }

  /** One scroll-wheel event per row crossed, anchored at the gesture's starting cell. */
  scroll(direction: 'up' | 'down', col: number, row: number): boolean {
    if (!this.canInput) return false; // gestures while connecting are discarded, never replayed
    return this.send({ t: 'scroll', direction, column: col, row });
  }

  private onTyped(data: string) {
    if (this.opts.mode === 'observe') return;
    // Keyboard Return (on-screen) is a line feed; the key bar's Enter is a carriage return.
    if (data === '\r') return void this.input({ text: '\n' });
    const m = activeMods();
    if (m.length) {
      this.input(applyModsToText(data, m));
      consumeMods();
      return;
    }
    this.input({ text: data });
  }

  private onKey(e: KeyboardEvent): boolean {
    if (this.opts.mode === 'observe') return false;
    if (e.type !== 'keydown') return e.type === 'keypress' ? true : false;
    if (e.isComposing || e.key === 'Process' || e.key === 'Unidentified') return true;
    if (e.metaKey) return false; // Command is not sent; let the browser handle ⌘C/⌘V etc.
    const hardware = isHardwareKeyEvent(e);
    // Hardware Return submits like a real terminal; on-screen Return stays a line feed (via onData).
    if (e.key === 'Enter' && !hardware && !e.shiftKey) return true;
    const out = fromKeyEvent(e, activeMods());
    if (!out) return true; // printable text → onData
    e.preventDefault();
    this.input(out);
    consumeMods();
    return false;
  }

  private onPasteEvent(e: ClipboardEvent) {
    // Keep pastes explicit: text goes through as a single paste (never submitted); images and files
    // are offered to the review flow by the stream screen.
    const items = e.clipboardData?.items;
    if (items) for (const it of Array.from(items)) if (it.kind === 'file') {
      e.preventDefault();
      e.stopPropagation();
      window.dispatchEvent(new CustomEvent('briardr:paste-file', { detail: it.getAsFile() }));
      return;
    }
    const text = e.clipboardData?.getData('text/plain');
    e.preventDefault();
    e.stopPropagation();
    if (text) window.dispatchEvent(new CustomEvent('briardr:paste-text', { detail: text }));
  }
}

/** Heuristic: a keydown while the on-screen keyboard isn't showing came from a hardware keyboard. */
export function isHardwareKeyEvent(_e: KeyboardEvent): boolean {
  if (!('ontouchstart' in window)) return true;
  if (!window.visualViewport) return false;
  return viewportInset() < 120;
}
