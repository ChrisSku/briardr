// An HTTPS proxy in front of a test server, for poor-connection checks: it passes requests and
// WebSockets through, or holds every request (a stalled network), answers with an error status (401, like a
// proxy's sign-in page, or 502, 503, 504), or stops listening (the host is unreachable). Uses the test server's own certificate.
import fs from 'node:fs';
import path from 'node:path';
import type { Server, ServerWebSocket } from 'bun';

export type ProxyMode = 'pass' | 'stall' | 'down' | 401 | 502 | 503 | 504;

interface Socket {
  url: string;
  headers: Record<string, string>;
  up?: WebSocket;
  queue: (string | Buffer)[];
}

export interface Proxy {
  readonly base: string;
  readonly port: number;
  /** Navigations (page loads) seen, in every mode. */
  readonly navigations: number;
  set(mode: ProxyMode): Promise<void>;
  close(): void;
}

export async function startProxy(upstream: string, dataDir: string): Promise<Proxy> {
  const tls = { cert: fs.readFileSync(path.join(dataDir, 'secrets', 'tls', 'server.crt')), key: fs.readFileSync(path.join(dataDir, 'secrets', 'tls', 'server.key')) };
  const target = new URL(upstream);
  let mode: ProxyMode = 'pass';
  let release: () => void = () => {};
  let held = new Promise<void>((r) => (release = r));
  let navigations = 0;
  const sockets = new Set<ServerWebSocket<Socket>>();
  let server: Server<Socket> | null = null;
  let port = 0;

  const listen = () => {
    server = Bun.serve<Socket>({
      port,
      tls,
      async fetch(req, srv) {
        if (req.headers.get('sec-fetch-mode') === 'navigate') navigations++;
        if (mode === 'stall') await held;
        if (typeof mode === 'number') return new Response(`proxy error ${mode}`, { status: mode, headers: { 'content-type': 'text/plain' } });
        const url = new URL(req.url);
        const to = `${target.origin}${url.pathname}${url.search}`;
        const headers: Record<string, string> = {};
        req.headers.forEach((v, k) => {
          if (!/^(connection|upgrade|sec-websocket-.*|content-length|accept-encoding)$/.test(k)) headers[k] = v;
        });
        if (req.headers.get('upgrade')?.toLowerCase() === 'websocket') {
          return srv.upgrade(req, { data: { url: to.replace(/^https/, 'wss'), headers, queue: [] } }) ? undefined : new Response('upgrade failed', { status: 400 });
        }
        try {
          const r = await fetch(to, { method: req.method, headers, body: req.body, redirect: 'manual', tls: { rejectUnauthorized: false }, decompress: true } as RequestInit);
          const out = new Headers(r.headers);
          out.delete('content-encoding');
          out.delete('content-length');
          return new Response(r.body, { status: r.status, headers: out });
        } catch {
          return new Response('upstream unavailable', { status: 502 });
        }
      },
      websocket: {
        open(ws) {
          sockets.add(ws);
          const up = new WebSocket(ws.data.url, { headers: ws.data.headers, tls: { rejectUnauthorized: false } } as any);
          up.binaryType = 'arraybuffer';
          ws.data.up = up;
          up.onopen = () => {
            for (const m of ws.data.queue) up.send(m);
            ws.data.queue = [];
          };
          up.onmessage = (e) => ws.send(typeof e.data === 'string' ? e.data : Buffer.from(e.data as ArrayBuffer));
          up.onclose = (e) => ws.close(e.code >= 3000 || e.code === 1000 ? e.code : 1011, e.reason);
          up.onerror = () => ws.close(1011);
        },
        message(ws, m) {
          if (ws.data.up?.readyState === WebSocket.OPEN) ws.data.up.send(m);
          else ws.data.queue.push(m);
        },
        close(ws) {
          sockets.delete(ws);
          ws.data.up?.close();
        },
      },
    });
    port = server.port!;
  };
  listen();

  return {
    get base() {
      return `https://localhost:${port}`;
    },
    get port() {
      return port;
    },
    get navigations() {
      return navigations;
    },
    async set(next: ProxyMode) {
      if (mode === 'stall' && next !== 'stall') {
        release();
        held = new Promise<void>((r) => (release = r));
      }
      // A changed network drops the connections that were open.
      if (next !== 'pass') for (const ws of sockets) ws.close(1011);
      if (next === 'down' && server) {
        server.stop(true);
        server = null;
      }
      mode = next;
      if (next !== 'down' && !server) listen();
    },
    close() {
      release();
      server?.stop(true);
    },
  };
}
