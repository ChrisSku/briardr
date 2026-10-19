// Fixture server for the frontend benchmark: serves a built web client (any dist directory) and answers
// every API route and WebSocket the client uses with deterministic fixture data. No Herdr, Git, usage
// providers or push services are involved, so both implementations get identical responses.
// Static files are served the way the server serves them (server/src/static_files.rs, mirrored in scripts/static.ts): same cache headers, and
// the build's compressed copies for browsers that accept them.
import fs from 'node:fs';
import path from 'node:path';
import type { Server, ServerWebSocket } from 'bun';
import * as F from './fixtures.ts';
import { staticFile } from '../scripts/static.ts';
import type { Model } from '../web/src/types.ts';

interface Note {
  id: string;
  text: string;
  revision: number;
  createdAt: number;
  updatedAt: number;
}

type WsData = { kind: 'live' } | { kind: 'term'; pane: string; mode: string; cols: number; rows: number };

export interface LoggedRequest {
  t: number;
  method: string;
  path: string;
  body?: any;
}

export class FixtureServer {
  readonly dist: string;
  readonly fixture: F.Fixture;
  /** The server's clock; fixed for screenshot comparisons so every "ago" label matches. */
  readonly clock: () => number;
  readonly now: number;
  model: Model;
  notes = new Map<string, Note[]>();
  requests: LoggedRequest[] = [];
  termInputs: any[] = [];
  private live = new Set<ServerWebSocket<WsData>>();
  private term = new Set<ServerWebSocket<WsData>>();
  private server: Server<WsData> | null = null;
  private seq = 0;

  /** Streams another client controls: asking for control gets "busy", and the client watches. */
  readonly busy: Set<string>;

  constructor(dist: string, size: number, opts: { fixedNow?: number; busy?: string[] } = {}) {
    this.dist = path.resolve(dist);
    this.busy = new Set(opts.busy || []);
    this.clock = opts.fixedNow ? () => opts.fixedNow! : () => Date.now();
    this.now = this.clock();
    this.fixture = F.makeFixture(size, this.now);
    this.model = structuredClone(this.fixture.model);
    const t = this.fixture.picks.terminal;
    this.notes.set(t, Array.from({ length: 5 }, (_, i) => ({ id: `n${String(i).padStart(15, '0')}`, text: F.noteText(i), revision: 1, createdAt: this.now - (10 - i) * 60_000, updatedAt: this.now - (10 - i) * 60_000 })));
  }

  get url(): string {
    return `http://127.0.0.1:${this.server!.port}`;
  }

  start(port = 0): this {
    this.server = Bun.serve<WsData>({
      hostname: '127.0.0.1',
      port,
      fetch: (req, server) => this.handle(req, server),
      websocket: {
        open: (ws) => this.onOpen(ws),
        message: (ws, msg) => this.onMessage(ws, msg),
        close: (ws) => {
          this.live.delete(ws);
          this.term.delete(ws);
        },
      },
    });
    return this;
  }

  stop() {
    this.server?.stop(true);
  }

  /** Counts of open sockets on the server side (to check a client released its connections). */
  openSockets() {
    return { live: this.live.size, term: this.term.size };
  }

  // ---------- control (called by the benchmark runner) ----------

  /** Replace the model (after `mutate`) and broadcast it; `bench` marks the message for latency checks. */
  pushModel(mutate: (m: Model) => void, bench: string) {
    const m = structuredClone(this.model);
    mutate(m);
    m.asOf = this.clock();
    this.model = m;
    this.broadcast({ t: 'model', model: m, herdr: F.herdrHealth(this.clock()), serverNow: this.clock(), version: 'bench', bench });
  }

  broadcast(msg: object) {
    const data = JSON.stringify(msg);
    for (const ws of this.live) ws.send(data);
  }

  /** Send `count` full-screen frames to every controlling terminal socket, `everyMs` apart. */
  async termBurst(count: number, everyMs: number, onSent?: (i: number) => void) {
    const socks = [...this.term].filter((w) => (w.data as any).mode === 'control');
    for (let i = 0; i < count; i++) {
      for (const ws of socks) {
        const d = ws.data as Extract<WsData, { kind: 'term' }>;
        ws.sendBinary(F.encodeFrame(++this.seq, d.cols, d.rows, F.screen(d.cols, d.rows, i)));
      }
      onSent?.(i);
      if (everyMs) await Bun.sleep(everyMs);
    }
  }

  resetLog() {
    this.requests = [];
    this.termInputs = [];
  }

  // ---------- HTTP ----------

  private json(body: unknown, status = 200) {
    return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json; charset=utf-8', 'cache-control': 'no-store' } });
  }

  private async handle(req: Request, server: Server<WsData>): Promise<Response | undefined> {
    const url = new URL(req.url);
    if (req.headers.get('upgrade')?.toLowerCase() === 'websocket') {
      if (url.pathname === '/ws/live') return server.upgrade(req, { data: { kind: 'live' } }) ? undefined : new Response(null, { status: 400 });
      if (url.pathname === '/ws/term') {
        const q = url.searchParams;
        const data: WsData = { kind: 'term', pane: q.get('pane') || '', mode: q.get('mode') || 'observe', cols: Number(q.get('cols')) || 80, rows: Number(q.get('rows')) || 24 };
        return server.upgrade(req, { data }) ? undefined : new Response(null, { status: 400 });
      }
      return new Response(null, { status: 403 });
    }
    if (url.pathname.startsWith('/api/')) {
      let body: any;
      if (req.method !== 'GET' && req.method !== 'HEAD') body = await req.text().then((t) => (t ? JSON.parse(t) : undefined)).catch(() => undefined);
      this.requests.push({ t: Date.now(), method: req.method, path: url.pathname + url.search, body });
      return this.api(req.method, url, body);
    }
    return this.serveStatic(req, url);
  }

  private async serveStatic(req: Request, url: URL): Promise<Response> {
    let rel = decodeURIComponent(url.pathname);
    if (rel === '/' || !path.extname(rel)) rel = '/index.html';
    const file = path.resolve(this.dist, '.' + rel);
    const st = file.startsWith(this.dist + path.sep) ? await fs.promises.stat(file).catch(() => null) : null;
    if (!st?.isFile()) return new Response('Not found', { status: 404 });
    return staticFile(file, st, rel, req.headers.get('accept-encoding'));
  }

  private api(method: string, url: URL, body: any): Response {
    const p = url.pathname;
    const now = this.clock();
    const m = p.match(/^\/api\/streams\/([^/]+)(\/.*)?$/);
    if (method === 'GET' && p === '/api/session') return this.json({ device: { id: 'd1', name: 'Bench browser' }, version: 'bench', protocol: 1 });
    if (method === 'GET' && p === '/api/mac') return this.json({ mac: F.mac(now) });
    if (method === 'GET' && p === '/api/state') return this.json({ t: 'model', model: this.model, herdr: F.herdrHealth(now), serverNow: now });
    if (method === 'GET' && p === '/api/settings') return this.json(F.settings);
    if (method === 'PUT' && p === '/api/settings') return this.json({ ok: true });
    if (method === 'GET' && p === '/api/devices') return this.json(F.devices(now));
    if (method === 'GET' && p === '/api/notifications') return this.json({ publicKey: 'BENCHKEY', subscribed: false, settings: F.settings.notifications });
    if (method === 'GET' && p === '/api/usage') return this.json({ usage: F.usage(this.now), now });
    if (method === 'GET' && p === '/api/usage/statusline') return this.json({ installed: false, preview: null });
    if (method === 'POST' && p === '/api/diagnostics/client') return this.json({ ok: true });
    if (method === 'DELETE' && p.startsWith('/api/previews/')) return this.json({ ok: true });
    if (m) {
      const sid = decodeURIComponent(m[1]);
      const rest = m[2] || '';
      if (!this.model.streams.some((s) => s.id === sid)) return this.json({ error: 'stream_gone', message: 'That stream no longer exists.' }, 404);
      if (method === 'GET' && rest === '/changes') return this.json(F.changes(now));
      if (method === 'GET' && rest === '/changes/diff') return this.json({ ...F.diff(url.searchParams.get('path') || ''), signature: 'sig' });
      if (method === 'GET' && rest === '/docs') return this.json(F.docsList(now));
      if (method === 'GET' && rest === '/docs/file') {
        const text = F.docText(url.searchParams.get('path') || 'README.md');
        return this.json({ text, size: text.length });
      }
      if (method === 'GET' && rest === '/info') return this.json({ agent: { name: 'claude' } });
      if (method === 'GET' && rest === '/history') return this.json({ text: Array.from({ length: 1000 }, (_, i) => `line ${i} ${F.screen(80, 1, i).slice(9)}`).join('\n'), truncated: true, asOf: now });
      const notes = this.notes.get(sid) || [];
      if (method === 'GET' && rest === '/notes') return this.json({ notes: [...notes].sort((a, b) => b.createdAt - a.createdAt) });
      const nm = rest.match(/^\/notes\/([a-f0-9n]{16})$/);
      if (nm) {
        const id = nm[1];
        const cur = notes.find((n) => n.id === id) || null;
        if (method === 'GET') return cur ? this.json({ note: cur }) : this.json({ error: 'note_gone', message: 'That note no longer exists.' }, 404);
        if (method === 'PUT') {
          const base = Number(body?.baseRevision);
          if ((cur ? cur.revision : 0) !== base) return this.json({ error: 'conflict', message: 'This note changed on another device.', note: cur }, 409);
          const next: Note = cur ? { ...cur, text: String(body.text), revision: cur.revision + 1, updatedAt: now } : { id, text: String(body.text), revision: 1, createdAt: now, updatedAt: now };
          this.notes.set(sid, [...notes.filter((n) => n.id !== id), next]);
          this.broadcast({ t: 'notes', streamId: sid, noteId: id, revision: next.revision });
          return this.json({ note: next });
        }
        if (method === 'DELETE') {
          this.notes.set(sid, notes.filter((n) => n.id !== id));
          this.broadcast({ t: 'notes', streamId: sid, noteId: id, revision: 0 });
          return this.json({ ok: true });
        }
      }
    }
    return this.json({ error: 'not_found' }, 404);
  }

  // ---------- WebSockets ----------

  private onOpen(ws: ServerWebSocket<WsData>) {
    const d = ws.data;
    if (d.kind === 'live') {
      this.live.add(ws);
      ws.send(JSON.stringify({ t: 'model', model: this.model, herdr: F.herdrHealth(this.clock()), serverNow: this.clock(), version: 'bench' }));
      ws.send(JSON.stringify({ t: 'mac', mac: F.mac(this.clock()) }));
      return;
    }
    if (!this.model.streams.some((s) => s.id === d.pane)) {
      ws.send(JSON.stringify({ t: 'closed', category: 'pane_gone', reason: 'That stream no longer exists.' }));
      return ws.close(4004, 'pane_gone');
    }
    if (d.mode === 'control' && this.busy.has(d.pane)) {
      ws.send(JSON.stringify({ t: 'closed', category: 'busy', reason: 'Another client is controlling this pane.' }));
      return ws.close(4009, 'busy');
    }
    this.term.add(ws);
    // Control gets the client's size (the Mac pane is resized for it); Peek keeps the Mac's size.
    if (d.mode !== 'control') {
      d.cols = 120;
      d.rows = 40;
    }
    ws.send(JSON.stringify({ t: 'hello', conn: 'bench', mode: d.mode, protocol: 1 }));
    ws.send(JSON.stringify({ t: 'live', mode: d.mode, width: d.cols, height: d.rows }));
    ws.sendBinary(F.encodeFrame(++this.seq, d.cols, d.rows, F.screen(d.cols, d.rows, 0)));
  }

  private onMessage(ws: ServerWebSocket<WsData>, raw: string | Buffer) {
    let msg: any;
    try {
      msg = JSON.parse(String(raw));
    } catch {
      return;
    }
    if (msg.t === 'ping') return ws.send(JSON.stringify({ t: 'pong', id: msg.id, now: this.clock() }));
    if (ws.data.kind === 'term') {
      if (msg.t === 'input') {
        this.termInputs.push(msg);
        ws.send(JSON.stringify({ t: 'input_ok', id: msg.id }));
      }
      if (msg.t === 'resize') {
        ws.data.cols = msg.cols;
        ws.data.rows = msg.rows;
      }
    }
  }
}

if (import.meta.main) {
  // Manual use: bun bench/server.ts <dist> [streams] [port]
  const s = new FixtureServer(process.argv[2] || 'web/dist', Number(process.argv[3] || 100)).start(Number(process.argv[4] || 0));
  console.log(`fixture server for ${s.dist} with ${s.fixture.size} streams on ${s.url}`);
}
