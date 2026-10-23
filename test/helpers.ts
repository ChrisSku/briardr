// Test helpers: the Briardr server binary on free loopback ports, using a throwaway data directory (uploads
// too) and the test run's own Herdr session. Nothing here touches the developer's real panes, and runs in
// parallel (other worktrees, other agents) don't share anything.
// The binary is `BRIARDR_SERVER_BIN`, else server/target/debug/briardr (`mise run build:test-server`): a build
// with the loopback test hooks (/__test/*), which stand in for reaching into the server from a test.
import { execFileSync, spawn } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import net from 'node:net';
import os from 'node:os';
import path from 'node:path';
import tls from 'node:tls';

/** This run's own Herdr session, removed when the run ends (test/preload.ts). BRIARDR_TEST_SESSION picks one
 *  to keep instead, such as briardr-dev to watch the tests in Herdr. */
export const SESSION = process.env.BRIARDR_TEST_SESSION || `briardr-test-${process.pid}`;
const OWN_SESSION = !process.env.BRIARDR_TEST_SESSION;

export function tmpDir(prefix = 'briardr-test-'): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

export interface TestServer {
  run: Running;
  port: number;
  base: string;
  dataDir: string;
  cookie: string;
  deviceId: string;
  close(): Promise<void>;
}

function scrubbed(): NodeJS.ProcessEnv {
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('HERDR_')) env[k] = v;
  return env;
}

let started = false;

/** Make sure this run's Herdr session is running (a test fixture; it never touches the default session). */
export function ensureSession(): void {
  const running = () => {
    try {
      return /status:\s*running/.test(execFileSync('herdr', ['--session', SESSION, 'status'], { env: scrubbed(), encoding: 'utf8' }));
    } catch {
      return false;
    }
  };
  if (!started && OWN_SESSION) removeStaleSessions();
  started = true;
  if (running()) return;
  const child = spawn('herdr', ['--session', SESSION, 'server'], { env: scrubbed(), stdio: 'ignore', detached: true });
  child.unref();
  const until = Date.now() + 10_000;
  while (!running()) {
    if (Date.now() > until) throw new Error(`could not start the ${SESSION} Herdr session`);
    execFileSync('sleep', ['0.2']);
  }
}

/** When the run ends: stop and delete its session, unless BRIARDR_TEST_SESSION named one to keep. */
export function endSession(): void {
  if (OWN_SESSION && started) removeSession(SESSION);
}

function removeSession(name: string) {
  for (const command of ['stop', 'delete']) {
    try {
      execFileSync('herdr', ['session', command, name], { env: scrubbed(), stdio: 'ignore' });
    } catch {}
  }
}

/** Sessions left by runs that ended early (interrupted or killed): their process is gone. */
function removeStaleSessions() {
  let list = '';
  try {
    list = execFileSync('herdr', ['session', 'list'], { env: scrubbed(), encoding: 'utf8' });
  } catch {
    return;
  }
  for (const [, name, pid] of list.matchAll(/^(briardr-test-(\d+))\s/gm)) {
    try {
      process.kill(Number(pid), 0);
    } catch (e) {
      if ((e as NodeJS.ErrnoException).code === 'ESRCH') removeSession(name!);
    }
  }
}

/** A port for a server under test, free on 127.0.0.1 (where it listens: a port taken there can look free on
 *  all addresses) with its 16 preview ports after it. Below the ephemeral ranges (macOS 49152+, Linux 32768+),
 *  so a port the OS hands out, such as a test's dev server, never lands among them. */
export async function freePort(): Promise<number> {
  const free = (port: number) =>
    new Promise<boolean>((resolve) => {
      const s = net.createServer();
      s.once('error', () => resolve(false));
      s.listen(port, '127.0.0.1', () => s.close(() => resolve(true)));
    });
  for (let i = 0; i < 100; i++) {
    const port = 20000 + Math.floor(Math.random() * 12000);
    let ok = true;
    for (let p = port; ok && p <= port + 16; p++) ok = await free(p);
    if (ok) return port;
  }
  throw new Error('no free ports for a test server');
}

export const SERVER_BIN = process.env.BRIARDR_SERVER_BIN || path.resolve(import.meta.dir, '..', 'server', 'target', 'debug', 'briardr');

/** What tests reach through `s.run`: each member calls a test hook or the CLI. */
export interface Running {
  auth: {
    createPairingCode(): { code: string };
    pair(code: string, name: string, ip?: string): any;
    revoke(id: string): boolean;
  };
  herdr: { refresh(): Promise<unknown> };
  app: { listRoutes(): { method: string; pattern: string; public: boolean }[]; d: { previews: { open(device: string, port: number): Promise<any> } } };
  push: { sendImpl: (sub: unknown, body: string, opts: object) => unknown; readonly subject: string };
  hub: { readonly sent: unknown[] };
  readonly urls: string[];
}

/** `config`: written to config.json first (e.g. proxy mode). `platform`: run as that host (BRIARDR_PLATFORM). */
export function startServer(opts: { pair?: boolean; config?: object; platform?: string } = {}): Promise<TestServer> {
  if (!fs.existsSync(SERVER_BIN)) throw new Error(`${SERVER_BIN} is missing: run \`mise run build:test-server\``);
  return startBinary(SERVER_BIN, opts);
}

/**
 * The binary as a child process with its own env and data dir. Tests reach
 * `s.run.*` synchronously, so the shim below calls the `/__test/*` hooks with curl. A hook
 * that is not built yet fails the test with its name, so a gap shows up as a failure, not a silent pass.
 */
async function startBinary(bin: string, opts: { pair?: boolean; config?: object; platform?: string }): Promise<TestServer> {
  ensureSession();
  const dataDir = tmpDir('briardr-data-');
  if (opts.config) fs.writeFileSync(path.join(dataDir, 'config.json'), JSON.stringify(opts.config));
  const port = await freePort();
  const env: NodeJS.ProcessEnv = {
    ...scrubbed(),
    BRIARDR_DATA_DIR: dataDir,
    BRIARDR_UPLOADS_DIR: path.join(dataDir, 'briardr-uploads'), // not the shared one in the temp directory
    BRIARDR_HERDR_SESSION: SESSION,
    BRIARDR_PORT: String(port),
    BRIARDR_PREVIEW_PORT: String(port + 1),
    BRIARDR_LISTEN: '',
    BRIARDR_NO_DICTATION: '1',
    BRIARDR_TEST_HOOKS: '1',
  };
  if (opts.platform) env.BRIARDR_PLATFORM = opts.platform;
  const child = spawn(bin, ['serve'], { env, stdio: ['ignore', 'pipe', 'pipe'] });
  let stderr = '';
  child.stderr?.on('data', (c) => (stderr += c));
  const base = `https://localhost:${port}`;
  const s = { port, base, cookie: '', dataDir };
  const until = Date.now() + 15_000;
  for (;;) {
    if (child.exitCode !== null) {
      await sleep(50); // let the pipe drain so the error text is complete
      throw new Error(`briardr exited during startup: ${stderr}`);
    }
    try {
      // A TCP connect, not an HTTPS request: in proxy mode the server speaks plain HTTP.
      await new Promise<void>((resolve, reject) => {
        const c = net.connect({ host: '127.0.0.1', port }, () => (c.destroy(), resolve()));
        c.on('error', reject);
      });
      break;
    } catch {
      if (Date.now() > until) {
        child.kill('SIGTERM');
        throw new Error(`briardr did not listen on ${port}: ${stderr}`);
      }
      await sleep(100);
    }
  }
  const stop = () =>
    new Promise<void>((resolve) => {
      if (child.exitCode !== null) return resolve();
      child.once('exit', () => resolve());
      child.kill('SIGTERM');
    });
  const proxy = (opts.config as { tls?: { mode?: string } } | undefined)?.tls?.mode === 'proxy';
  const run = hooks(s, bin, env, proxy ? 'http' : 'https');
  const ts: TestServer = { run, port, base, dataDir, cookie: '', deviceId: '', close: stop };
  if (opts.pair !== false) {
    const { code } = run.auth.createPairingCode();
    const r = await request(ts, 'POST', '/api/pair', { code, name: 'test' }, { cookie: false });
    ts.cookie = String(r.headers['set-cookie']?.[0] || '').split(';')[0];
    ts.deviceId = r.json.device.id;
  }
  return ts;
}

/** Delivers captured pushes to the test's `sendImpl` (see the push shim). */
let pushSync: { port: number; sync: () => Promise<void> } | null = null;

function hooks(s: { port: number; dataDir: string }, bin: string, env: NodeJS.ProcessEnv, scheme: 'http' | 'https'): Running {
  const hook = (method: 'GET' | 'POST', p: string, body?: unknown): any => {
    const args = ['-sk', '-X', method, `${scheme}://127.0.0.1:${s.port}/__test${p}`, '-H', `host: localhost:${s.port}`];
    if (body !== undefined) args.push('-H', 'content-type: application/json', '--data-binary', JSON.stringify(body));
    const out = execFileSync('curl', args, { encoding: 'utf8' });
    return out ? JSON.parse(out) : undefined;
  };
  const missing = (name: string): never => {
    throw new Error(`test hook for ${name} is not available on the Rust server yet`);
  };
  const shim = {
    auth: {
      createPairingCode: () => {
        const out = execFileSync(bin, ['pair'], { env: { ...env, BRIARDR_DATA_DIR: s.dataDir }, encoding: 'utf8' });
        const m = out.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/);
        if (!m) throw new Error('no pairing code');
        return { code: m[0] };
      },
      pair: (code: string, name: string, ip?: string) => hook('POST', '/auth/pair', { code, name, ip }),
      revoke: (id: string) => hook('POST', '/auth/revoke', { id }),
    },
    herdr: { refresh: async () => hook('POST', '/herdr/refresh', {}) },
    app: {
      listRoutes: () => hook('GET', '/routes'),
      d: { previews: { open: async (device: string, port: number) => hook('POST', '/previews/open', { device, port }) } },
    },
    push: {
      get sendImpl(): never {
        return missing('push.sendImpl (read)');
      },
      // The server records pushes instead of sending them (answering 201); after each request() the
      // harness passes every new payload to the test's function, as if the server had called it.
      set sendImpl(fn: (sub: unknown, body: string, opts: object) => unknown) {
        hook('POST', '/push/capture', { status: 201 });
        let seen = 0;
        pushSync = {
          port: s.port,
          sync: async () => {
            const sent: string[] = hook('GET', '/push/sent');
            for (; seen < sent.length; seen++) await fn(null, sent[seen], {});
          },
        };
      },
      get subject(): string {
        return hook('GET', '/push/subject');
      },
    },
    hub: {
      get sent() {
        return hook('GET', '/hub/sent');
      },
    },
    get urls(): string[] {
      return hook('GET', '/urls');
    },
  };
  return shim as unknown as Running;
}

export interface Res {
  status: number;
  headers: Record<string, any>;
  body: string;
  json: any;
}

export async function request(s: { base: string; cookie: string; port: number }, method: string, p: string, body?: unknown, o: { cookie?: boolean | string; origin?: string | null; host?: string; raw?: Buffer; headers?: Record<string, string> } = {}): Promise<Res> {
  const headers: Record<string, string> = { host: o.host || `localhost:${s.port}`, ...(o.headers || {}) };
  const cookie = o.cookie === false ? null : typeof o.cookie === 'string' ? o.cookie : s.cookie;
  if (cookie) headers.cookie = cookie;
  if (o.origin !== null) headers.origin = o.origin ?? s.base;
  let data: Buffer | undefined;
  if (o.raw) data = o.raw;
  else if (body !== undefined) {
    data = Buffer.from(JSON.stringify(body));
    headers['content-type'] = 'application/json';
  }
  const res = await fetch(`https://127.0.0.1:${s.port}${p}`, { method, headers, body: data && new Uint8Array(data), tls: { rejectUnauthorized: false } });
  const text = await res.text();
  let json: any = null;
  try {
    json = JSON.parse(text);
  } catch {}
  const h: Record<string, any> = res.headers.toJSON();
  h['set-cookie'] = res.headers.getSetCookie();
  if (pushSync?.port === s.port) await pushSync.sync();
  return { status: res.status, headers: h, body: text, json };
}

function socketHeaders(s: TestServer, o: { cookie?: string | false; origin?: string }): Record<string, string> {
  const headers: Record<string, string> = { origin: o.origin ?? s.base, host: `localhost:${s.port}` };
  const cookie = o.cookie === false ? null : o.cookie ?? s.cookie;
  if (cookie) headers.cookie = cookie;
  return headers;
}

/** Bun's WebSocket client; `terminate()` drops the connection without a closing handshake. */
export type TestSocket = WebSocket & { terminate(): void };

export function ws(s: TestServer, p: string, o: { cookie?: string | false; origin?: string } = {}): TestSocket {
  const options: Bun.WebSocketOptions = { headers: socketHeaders(s, o), tls: { rejectUnauthorized: false } };
  // (@types/node's WebSocket declaration hides Bun's options overload.)
  const sock = new WebSocket(`wss://127.0.0.1:${s.port}${p}`, options as string[]) as TestSocket;
  sock.binaryType = 'arraybuffer';
  return sock;
}

/** The HTTP status a WebSocket upgrade request gets (101 when accepted). */
export function upgradeStatus(s: TestServer, p: string, o: { cookie?: string | false; origin?: string } = {}): Promise<number> {
  const headers = { ...socketHeaders(s, o), connection: 'Upgrade', upgrade: 'websocket', 'sec-websocket-version': '13', 'sec-websocket-key': crypto.randomBytes(16).toString('base64') };
  return new Promise((resolve, reject) => {
    const sock = tls.connect({ host: '127.0.0.1', port: s.port, servername: 'localhost', rejectUnauthorized: false }, () => {
      sock.write(`GET ${p} HTTP/1.1\r\n${Object.entries(headers).map(([k, v]) => `${k}: ${v}\r\n`).join('')}\r\n`);
    });
    let head = '';
    sock.on('data', (c: Buffer) => {
      head += c.toString('latin1');
      const m = head.match(/^HTTP\/1\.1 (\d{3})/);
      if (m) {
        sock.destroy();
        resolve(Number(m[1]));
      }
    });
    sock.on('error', reject);
  });
}

export function opened(sock: WebSocket): Promise<void> {
  return new Promise((resolve, reject) => {
    if (sock.readyState === WebSocket.OPEN) return resolve();
    sock.addEventListener('open', () => resolve(), { once: true });
    sock.addEventListener('close', () => reject(new Error('socket closed before opening')), { once: true });
  });
}

/** Collect JSON messages from a socket until predicate matches (or timeout). */
export function until(sock: WebSocket, pred: (m: any) => boolean, ms = 8000): Promise<any[]> {
  const seen: any[] = [];
  return new Promise((resolve, reject) => {
    const t = setTimeout(() => {
      sock.removeEventListener('message', on);
      reject(new Error(`timeout; saw ${JSON.stringify(seen.slice(-5))}`));
    }, ms);
    const on = (e: MessageEvent) => {
      if (typeof e.data !== 'string') {
        seen.push({ t: 'frame' });
        if (pred({ t: 'frame' })) done();
        return;
      }
      let m: any;
      try {
        m = JSON.parse(e.data);
      } catch {
        return;
      }
      seen.push(m);
      if (pred(m)) done();
    };
    const done = () => {
      clearTimeout(t);
      sock.removeEventListener('message', on);
      resolve(seen);
    };
    sock.addEventListener('message', on);
  });
}

/** Resolves with the close code once the socket has closed. */
export function closed(sock: WebSocket, ms = 8000): Promise<number> {
  return new Promise((resolve, reject) => {
    if (sock.readyState === WebSocket.CLOSED) return resolve(-1);
    const t = setTimeout(() => reject(new Error('socket did not close')), ms);
    sock.addEventListener(
      'close',
      (e) => {
        clearTimeout(t);
        resolve(e.code);
      },
      { once: true },
    );
  });
}

export const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

/** Herdr CLI against the isolated session only. */
export function herdr(...args: string[]): any {
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('HERDR_')) env[k] = v;
  const out = execFileSync('herdr', ['--session', SESSION, ...args], { env, encoding: 'utf8' });
  try {
    return JSON.parse(out);
  } catch {
    return out;
  }
}

/** A dedicated test workspace with a shell pane in a disposable folder. */
export function testPane(label: string): { pane: string; workspace: string; dir: string } {
  ensureSession();
  const dir = tmpDir('briardr-ws-');
  const r = herdr('workspace', 'create', '--cwd', dir, '--label', label, '--no-focus');
  return { pane: r.result.root_pane.pane_id, workspace: r.result.workspace.workspace_id, dir };
}

export function closeWorkspace(id: string) {
  try {
    herdr('workspace', 'close', id);
  } catch {}
}

export function readPane(pane: string, lines = 30): string {
  const out = herdr('pane', 'read', pane, '--source', 'recent', '--lines', String(lines));
  return typeof out === 'string' ? out : JSON.stringify(out);
}

/** A copy of a test plugin (test/fixtures/plugins/<name>), built with scripts/plugin-build.ts and installed in `dataDir`. */
export function installPlugin(dataDir: string, name: string): string {
  const dir = path.join(tmpDir('briardr-plugin-'), name);
  fs.cpSync(path.resolve(import.meta.dir, 'fixtures', 'plugins', name), dir, { recursive: true, filter: (f) => !f.includes(`${path.sep}dist`) });
  buildPlugin(dir);
  fs.mkdirSync(path.join(dataDir, 'plugins'), { recursive: true });
  fs.symlinkSync(dir, path.join(dataDir, 'plugins', name));
  return dir;
}

export function buildPlugin(dir: string) {
  const r = Bun.spawnSync([process.execPath, path.resolve(import.meta.dir, '..', 'scripts', 'plugin-build.ts'), dir], { stderr: 'pipe', stdout: 'pipe' });
  if (r.exitCode !== 0) throw new Error(`plugin build failed: ${r.stderr.toString()}`);
}
