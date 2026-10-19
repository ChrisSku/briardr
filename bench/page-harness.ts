// In-page instrumentation for the benchmark, installed with addInitScript before any app code runs.
// It only observes the DOM, WebSockets, timers and listeners, so it works the same for any framework.
//
// Timing model (all times are performance.now(), ms since navigation start):
//   t0    the triggering event reached the page (capture-phase listener on window, before app handlers),
//         or a marked WebSocket message arrived (listener registered before the app's own handler)
//   dom   a MutationObserver callback first found the expected DOM (DOM update complete)
//   frame the first animation frame after `dom` started (rAF callback; paint follows it)
//   after a task queued from that rAF callback ran (approximately after the frame was produced)
// `proc` (input processing) is when a microtask queued from a bubble-phase window listener ran: every
// handler of the event and the microtasks they queued (framework re-renders) had finished.

export interface PageHarnessOptions {
  /** Predicate source for "populated" on a fresh launch (evaluated as an expression). */
  launchPredicate: string;
}

export function pageHarness(opts: PageHarnessOptions) {
  const w = window as any;
  const B: any = (w.__bench = {
    longtasks: [] as [number, number][],
    longtaskSupported: false,
    recv: {} as Record<string, number>,
    sockets: [] as any[],
    binary: { count: 0, first: 0, last: 0 },
    launch: { populated: null as number | null, interactive: null as number | null, frame: null as number | null, lcp: null as number | null },
  });
  // Largest contentful paint (Chromium; null where unsupported).
  try {
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) B.launch.lcp = e.startTime;
    }).observe({ type: 'largest-contentful-paint', buffered: true } as any);
  } catch {}

  try {
    new PerformanceObserver((l) => {
      for (const e of l.getEntries()) B.longtasks.push([e.startTime, e.duration]);
    }).observe({ type: 'longtask', buffered: true } as any);
    B.longtaskSupported = true;
  } catch {}

  // ---------- WebSockets: receipt times of marked messages, open sockets ----------
  const WS = window.WebSocket;
  let wsId = 0;
  class BenchSocket extends WS {
    __id = ++wsId;
    __url: string;
    constructor(url: string | URL, protocols?: string | string[]) {
      super(url, protocols);
      this.__url = String(url).replace(/\?.*/, '').replace(/^wss?:\/\/[^/]+/, '');
      B.sockets.push(this);
      this.addEventListener('message', (e: MessageEvent) => {
        const t = performance.now();
        if (typeof e.data === 'string') {
          const m = /"bench":"([^"]+)"/.exec(e.data);
          if (m) B.recv[m[1]] = t;
        } else {
          if (!B.binary.count) B.binary.first = t;
          B.binary.count++;
          B.binary.last = t;
        }
      });
    }
  }
  w.WebSocket = BenchSocket;
  B.openSockets = () => B.sockets.filter((s: WebSocket) => s.readyState <= 1).map((s: any) => ({ id: s.__id, url: s.__url, state: s.readyState }));

  // ---------- listeners on window/document/visualViewport/media queries, intervals, observers ----------
  const reg = new Map<any, Map<string, Set<any>>>();
  const tracked = (t: any) => t === window || t === document || (window.visualViewport && t === window.visualViewport) || (typeof MediaQueryList !== 'undefined' && t instanceof MediaQueryList);
  const kindOf = (t: any) => (t === window ? 'window' : t === document ? 'document' : t === window.visualViewport ? 'visualViewport' : 'mediaQuery');
  const add = EventTarget.prototype.addEventListener;
  const remove = EventTarget.prototype.removeEventListener;
  const keyOf = (type: string, o: any) => `${type}|${typeof o === 'boolean' ? o : !!o?.capture}`;
  EventTarget.prototype.addEventListener = function (this: any, type: string, fn: any, o?: any) {
    if (fn && tracked(this)) {
      let m = reg.get(this);
      if (!m) reg.set(this, (m = new Map()));
      const k = keyOf(type, o);
      if (!m.has(k)) m.set(k, new Set());
      m.get(k)!.add(fn);
    }
    return add.call(this, type, fn, o);
  };
  EventTarget.prototype.removeEventListener = function (this: any, type: string, fn: any, o?: any) {
    reg.get(this)?.get(keyOf(type, o))?.delete(fn);
    return remove.call(this, type, fn, o);
  };
  B.listeners = () => {
    const out: Record<string, number> = { window: 0, document: 0, visualViewport: 0, mediaQuery: 0 };
    for (const [t, m] of reg) for (const s of m.values()) out[kindOf(t)] += s.size;
    return out;
  };
  const intervals = new Set<number>();
  const si = window.setInterval.bind(window);
  const ci = window.clearInterval.bind(window);
  (window as any).setInterval = (fn: any, ms?: number, ...a: any[]) => {
    const id = si(fn, ms, ...a);
    intervals.add(id);
    return id;
  };
  (window as any).clearInterval = (id?: number) => {
    if (id !== undefined) intervals.delete(id);
    return ci(id);
  };
  B.intervals = () => intervals.size;
  let observers = 0;
  const RO = window.ResizeObserver;
  if (RO)
    (window as any).ResizeObserver = class extends RO {
      __on = false;
      observe(t: Element, o?: ResizeObserverOptions) {
        if (!this.__on) observers++;
        this.__on = true;
        return super.observe(t, o);
      }
      disconnect() {
        if (this.__on) observers--;
        this.__on = false;
        return super.disconnect();
      }
    };
  B.resizeObservers = () => observers;

  // ---------- fresh launch: populated, then responsive ----------
  const launchPred = new Function('return (' + opts.launchPredicate + ')');
  const lmo = new MutationObserver(() => {
    if (B.launch.populated !== null) return;
    let ok = false;
    try {
      ok = !!launchPred();
    } catch {}
    if (!ok) return;
    B.launch.populated = performance.now();
    lmo.disconnect();
    // The populated list on screen: a task queued from the next animation frame.
    requestAnimationFrame(() => {
      const ch = new MessageChannel();
      ch.port1.onmessage = () => (B.launch.frame = performance.now());
      ch.port2.postMessage(0);
    });
    // Interactive: the first time after populated that a queued task runs within 50 ms.
    const probe = () => {
      const q = performance.now();
      const ch = new MessageChannel();
      ch.port1.onmessage = () => {
        const t = performance.now();
        if (t - q < 50) B.launch.interactive = q;
        else probe();
      };
      ch.port2.postMessage(0);
    };
    probe();
  });
  lmo.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });

  // ---------- measurements ----------
  const afterFrame = (r: any, done: () => void) =>
    requestAnimationFrame(() => {
      r.frame = performance.now();
      const ch = new MessageChannel();
      ch.port1.onmessage = () => {
        r.after = performance.now();
        done();
      };
      ch.port2.postMessage(0);
    });

  /**
   * Arm a measurement. `trigger` is an event type (first such event reaching window starts it) or
   * 'recv:<id>' (a marked WebSocket message). `preds` maps names to predicate sources; each records
   * the first `dom` time it holds after the trigger. Resolves when all hold (or on timeout).
   */
  B.arm = (trigger: string, preds: Record<string, string>, timeoutMs = 15000) => {
    const fns = Object.entries(preds).map(([name, src]) => ({ name, fn: new Function('return (' + src + ')') }));
    const r: any = { t0: null, proc: null, dom: {} as Record<string, number>, frame: null, after: null, error: null };
    let resolve!: (v: any) => void;
    B.pending = new Promise((res) => (resolve = res));
    let finished = false;
    const finish = (err?: string) => {
      if (finished) return;
      finished = true;
      mo.disconnect();
      document.removeEventListener('focusin', onFocus, true);
      clearTimeout(timer);
      if (err) {
        r.error = err;
        resolve(r);
      } else afterFrame(r, () => resolve(r));
    };
    const recvId = trigger.startsWith('recv:') ? trigger.slice(5) : null;
    const check = () => {
      if (finished) return;
      if (r.t0 === null) {
        // The receipt is stamped by the socket listener before the app's own handler runs.
        if (recvId && B.recv[recvId] !== undefined) r.t0 = B.recv[recvId];
        else return;
      }
      const t = performance.now();
      for (const p of fns) {
        if (r.dom[p.name] !== undefined) continue;
        let ok = false;
        try {
          ok = !!p.fn();
        } catch {}
        if (ok) r.dom[p.name] = t;
      }
      if (fns.every((p) => r.dom[p.name] !== undefined)) finish();
    };
    const mo = new MutationObserver(check);
    mo.observe(document, { subtree: true, childList: true, attributes: true, characterData: true });
    // Focus moves without mutating the DOM; check on focus changes, and once per frame as a fallback.
    const onFocus = () => check();
    document.addEventListener('focusin', onFocus, true);
    const poll = () => {
      if (finished) return;
      check();
      requestAnimationFrame(poll);
    };
    requestAnimationFrame(poll);
    const timer = setTimeout(() => finish('timeout'), timeoutMs);
    if (!recvId) {
      const on = () => {
        if (r.t0 !== null) return;
        r.t0 = performance.now();
        window.removeEventListener(trigger, on, true);
        // Input processing ends when the event has passed every handler (a bubble-phase listener on
        // window runs last) and the microtasks they queued, where both frameworks schedule re-renders.
        window.addEventListener(
          trigger,
          () =>
            queueMicrotask(() => {
              r.proc = performance.now();
              check(); // a change that needed no DOM mutation still counts
            }),
          { once: true },
        );
      };
      window.addEventListener(trigger, on, true);
    }
    return true;
  };

  /** Animation-frame gaps and long tasks between start() and stop(). */
  B.frames = {
    on: false,
    gaps: [] as number[],
    start() {
      this.on = true;
      this.gaps = [];
      this.t0 = performance.now();
      let last = performance.now();
      const loop = (t: number) => {
        if (!this.on) return;
        this.gaps.push(t - last);
        last = t;
        requestAnimationFrame(loop);
      };
      requestAnimationFrame((t) => {
        last = t;
        requestAnimationFrame(loop);
      });
    },
    stop() {
      this.on = false;
      const t1 = performance.now();
      const lt = B.longtasks.filter(([s, d]: [number, number]) => s + d >= this.t0 && s <= t1);
      return { gaps: this.gaps, longtasks: lt, t0: this.t0, t1 };
    },
  } as any;

  B.longtasksBetween = (a: number, b: number) => B.longtasks.filter(([s, d]: [number, number]) => s + d >= a && s <= b);
}
