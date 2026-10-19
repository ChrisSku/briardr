// Linux server deployment: the Linux host API, the reverse-proxy mode behind a public HTTPS origin,
// trusted forwarding headers, pairing limits on the internet, preview origins, and generated files.
import assert from 'node:assert/strict';
import http from 'node:http';
import net from 'node:net';
import os from 'node:os';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { spawnSync } from 'node:child_process';
import { closed, request, SERVER_BIN, sleep, startServer, tmpDir, until, type TestServer } from './helpers.ts';

const ORIGIN = 'https://briardr.test';
const PREVIEW = 'https://p{n}.briardr.test';
const MAX_CLIENT_ATTEMPTS = 5;
const MAX_CODE_ATTEMPTS = 20;
const LAUNCH_PATH = '/__briardr/launch';
const PREVIEW_COOKIE = '__Host-briardr-preview';

describe('Linux host API', () => {
  let s: TestServer;
  beforeAll(async () => {
    s = await startServer({ platform: 'linux' });
  });
  afterAll(() => s.close());

  test('reports itself as a server, with no Mac-only capabilities', async () => {
    const r = await request(s, 'GET', '/api/session');
    assert.equal(r.json.host.noun, 'server');
    assert.equal(r.json.host.battery, false);
    assert.equal((await request(s, 'GET', '/api/session', undefined, { cookie: false })).json.host.noun, 'server', 'the pairing screen can say “server” too');
    const m = await request(s, 'GET', '/api/mac');
    assert.equal(m.json.host.platform, 'linux');
    assert.equal(m.json.mac.battery.present, false);
    const st = await request(s, 'GET', '/api/settings');
    assert.equal(st.json.dictation, false);
    assert.equal(st.json.host.claudeKeychain, false);
  });

  test('Mac-only actions are refused, never acknowledged', async () => {
    for (const [method, p, body] of [
      ['POST', '/api/power/sleep', { requestId: 'abcdefgh1234' }],
      ['POST', '/api/power/keep-awake', { on: true }],
      ['POST', '/api/dictation', undefined],
      ['PUT', '/api/settings', { usage: { claudeKeychainOptIn: true } }],
      ['PUT', '/api/settings', { notifications: { lowBattery: true } }],
    ] as const) {
      const r = await request(s, method, p, body);
      assert.equal(r.status, 501, `${method} ${p}`);
      assert.equal(r.json.error, 'unsupported');
      assert.match(r.json.message, /server/);
    }
    assert.equal((await request(s, 'PUT', '/api/settings', { usage: { claudeKeychainOptIn: false } })).status, 200, 'turning it off is fine');
    assert.equal((await request(s, 'GET', '/api/diagnostics')).headers['content-disposition'], 'attachment; filename="briardr-server-diagnostics.ndjson"');
  });
});

describe('WireGuard (direct mode on a tunnel address)', () => {
  const WG = 'https://10.8.0.1:7443';

  test('the push (VAPID) subject is an https URL Apple accepts, never a made-up mailto domain', async () => {
    const s = await startServer({ pair: false, config: { publicOrigin: WG } });
    try {
      assert.equal((s.run.push as any).subject, WG);
    } finally {
      await s.close();
    }
    const d = await startServer({ pair: false });
    try {
      assert.match((d.run.push as any).subject, /^https:\/\//, 'derived from the listen address, not mailto:…@localhost.localdomain');
    } finally {
      await d.close();
    }
  });
});

// ---------- proxy mode, end to end (requests as Caddy forwards them) ----------

interface Call {
  status: number;
  headers: Headers;
  json: any;
  body: string;
}

describe('listen addresses', () => {
  test('a listen address the host doesn’t have is reported clearly (and is retryable)', () => {
    // TEST-NET-1: never an address of this host. Exit code 75 tells systemd to retry (RestartPreventExitStatus=78 only).
    const r = spawnSync(SERVER_BIN, ['serve'], {
      env: { ...process.env, BRIARDR_DATA_DIR: tmpDir('briardr-data-'), BRIARDR_LISTEN: '192.0.2.77', BRIARDR_PORT: '0', BRIARDR_NO_DICTATION: '1' },
      encoding: 'utf8',
      timeout: 20_000,
    });
    assert.equal(r.status, 75, r.stderr);
    assert.match(r.stderr, /192\.0\.2\.77/);
    assert.match(r.stderr, /wg-quick@wg0/);
  });
});

describe('proxy mode', () => {
  let s: TestServer;
  let cookie = '';
  beforeAll(async () => {
    s = await startServer({ pair: false, config: { tls: { mode: 'proxy' }, publicOrigin: ORIGIN, publicPreviewOrigin: PREVIEW } });
    const { code } = s.run.auth.createPairingCode();
    const p = s.run.auth.pair(code, 'phone');
    assert.ok(p.ok);
    cookie = `__Host-briardr=${p.cookie}`;
  });
  afterAll(() => s.close());

  async function call(method: string, p: string, o: { body?: unknown; host?: string; origin?: string | null; cookie?: string | null; xff?: string; port?: number } = {}): Promise<Call> {
    const headers: Record<string, string> = { host: o.host ?? 'briardr.test', 'x-forwarded-for': o.xff ?? '203.0.113.10', 'x-forwarded-proto': 'https' };
    if (o.origin !== null) headers.origin = o.origin ?? ORIGIN;
    const c = o.cookie === undefined ? cookie : o.cookie;
    if (c) headers.cookie = c;
    if (o.body !== undefined) headers['content-type'] = 'application/json';
    const res = await fetch(`http://127.0.0.1:${o.port ?? s.port}${p}`, { method, headers, body: o.body === undefined ? undefined : JSON.stringify(o.body), redirect: 'manual' });
    const body = await res.text();
    let json: any = null;
    try {
      json = JSON.parse(body);
    } catch {}
    return { status: res.status, headers: res.headers, json, body };
  }

  function upgrade(p: string, headers: Record<string, string>): Promise<number> {
    return new Promise((resolve, reject) => {
      const sock = net.connect(s.port, '127.0.0.1', () => {
        const all = { connection: 'Upgrade', upgrade: 'websocket', 'sec-websocket-version': '13', 'sec-websocket-key': 'dGhlIHNhbXBsZSBub25jZQ==', ...headers };
        sock.write(`GET ${p} HTTP/1.1\r\n${Object.entries(all).map(([k, v]) => `${k}: ${v}\r\n`).join('')}\r\n`);
      });
      let head = '';
      sock.on('data', (c) => {
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

  test('serves plain HTTP on loopback only, with no certificate of its own', async () => {
    assert.deepEqual(s.run.urls, [`http://127.0.0.1:${s.port}`]);
    assert.equal((await call('GET', '/api/state')).status, 200);
    assert.equal((await call('GET', '/api/ca.mobileconfig', { cookie: null })).status, 404, 'no local CA profile behind a public proxy');
  });

  test('accepts only the public host and origin', async () => {
    assert.equal((await call('GET', '/api/state', { host: 'evil.example' })).status, 421);
    assert.equal((await call('POST', '/api/projects', { body: {}, origin: 'https://evil.example' })).status, 403);
    assert.equal((await call('POST', '/api/projects', { body: {}, origin: 'http://briardr.test' })).status, 403, 'the http origin is not the public one');
    assert.equal((await call('POST', '/api/projects', { body: {}, origin: `https://localhost:${s.port}` })).status, 403);
    assert.notEqual((await call('POST', '/api/projects', { body: { workspaceId: 'nope' } })).status, 403);
  });

  test('every API route needs a device; WebSockets need a device and the public origin', async () => {
    for (const r of s.run.app.listRoutes().filter((x) => !x.public)) {
      const p = r.pattern.replace(/^\^/, '').replace(/\$$/, '').replace(/\(\[\^\/\]\+\)/g, 'x').replace(/\\\//g, '/');
      assert.equal((await call(r.method, p, { cookie: null, body: r.method === 'GET' ? undefined : {}, xff: `198.51.100.${(p.length % 200) + 1}` })).status, 401, `${r.method} ${p}`);
    }
    const base = { host: 'briardr.test', origin: ORIGIN, 'x-forwarded-for': '203.0.113.20' };
    assert.equal(await upgrade('/ws/live?v=1', base), 401);
    assert.equal(await upgrade('/ws/live?v=1', { ...base, cookie, origin: 'https://evil.example' }), 403);
    assert.equal(await upgrade('/ws/live?v=1', { ...base, cookie, host: 'evil.example' }), 403);
    assert.equal(await upgrade('/ws/live?v=1', { ...base, cookie }), 101);
  });

  test('the live connection works through the proxy and revocation closes it', async () => {
    const { code } = s.run.auth.createPairingCode();
    const p = s.run.auth.pair(code, 'to revoke');
    assert.ok(p.ok);
    const c = `__Host-briardr=${p.cookie}`;
    const options: Bun.WebSocketOptions = { headers: { host: 'briardr.test', origin: ORIGIN, cookie: c, 'x-forwarded-for': '203.0.113.30' } };
    const live = new WebSocket(`ws://127.0.0.1:${s.port}/ws/live?v=1`, options as string[]);
    const msgs = await until(live, (m) => m.t === 'mac');
    assert.equal(msgs.find((m) => m.t === 'mac').host.noun, os.platform() === 'darwin' ? 'Mac' : 'server');
    const closing = closed(live);
    s.run.auth.revoke(p.device.id);
    assert.equal(await closing, 4401);
  });

  test('security headers frame only the preview origins', async () => {
    const r = await call('GET', '/');
    const csp = r.headers.get('content-security-policy') || '';
    assert.match(csp, /frame-src https:\/\/p1\.briardr\.test https:\/\/p2\.briardr\.test /);
    assert.doesNotMatch(csp, /frame-src https:\/\/briardr\.test:\*/);
    assert.match(csp, /connect-src 'self' wss:\/\/briardr\.test/);
  });

  test('pairing: limits per client, a code that strangers can’t burn, and no hint whether one is active', async () => {
    const pair = (code: string, xff: string) => call('POST', '/api/pair', { body: { code, name: 'x' }, cookie: null, xff });
    const none = await pair('ZZZZ-ZZZZ', '203.0.113.40');
    assert.equal(none.status, 400);
    assert.equal(none.json.error, 'invalid', 'no active code looks like a wrong code');
    const { code } = s.run.auth.createPairingCode();
    for (let i = 0; i < MAX_CLIENT_ATTEMPTS; i++) assert.equal((await pair('ZZZZ-ZZZZ', '203.0.113.41')).json.error, 'invalid');
    assert.equal((await pair(code, '203.0.113.41')).status, 400, 'that client is locked out of this code');
    const good = await pair(code, '203.0.113.42');
    assert.equal(good.status, 200, 'another client can still pair with it');
    const set = good.headers.get('set-cookie') || '';
    assert.match(set, /^__Host-briardr=[^;]+; Path=\/; Secure; HttpOnly; SameSite=Strict/);
    // A code is withdrawn after MAX_CODE_ATTEMPTS wrong guesses from everyone together.
    const next = s.run.auth.createPairingCode();
    for (let i = 0; i < MAX_CODE_ATTEMPTS; i++) s.run.auth.pair('ZZZZZZZZ', 'x', `198.18.0.${i}`);
    assert.equal(s.run.auth.pair(next.code, 'x', '198.18.1.1').ok, false);
  });

  test('pairing attempts are limited per client; a spoofed X-Forwarded-For entry doesn’t evade it', async () => {
    let limited = 0;
    for (let i = 0; i < 12; i++) if ((await call('POST', '/api/pair', { body: { code: 'ZZZZ-ZZZZ' }, cookie: null, xff: `10.${i}.0.1, 203.0.113.50` })).status === 429) limited++;
    assert.ok(limited >= 2, 'the 11th attempt from the same real client is refused');
    assert.equal((await call('POST', '/api/pair', { body: { code: 'ZZZZ-ZZZZ' }, cookie: null, xff: '203.0.113.51' })).status, 400, 'another client is not affected');
  });

  test('requests without a device are rate-limited per client; signed-in devices are not', async () => {
    let first429 = -1;
    for (let i = 0; i < 130 && first429 < 0; i++) if ((await call('GET', '/api/state', { cookie: null, xff: '203.0.113.60' })).status === 429) first429 = i;
    assert.ok(first429 >= 100, `limited after ~120 (at ${first429})`);
    assert.equal(await upgrade('/ws/live?v=1', { host: 'briardr.test', origin: ORIGIN, 'x-forwarded-for': '203.0.113.60' }), 429);
    assert.equal((await call('GET', '/api/state', { cookie: null, xff: '203.0.113.61' })).status, 401);
    assert.equal((await call('GET', '/api/state', { xff: '203.0.113.60' })).status, 200, 'the same address with a device cookie still works');
  });

  test('previews: their own origin, a one-time launch link, and no Briardr cookie at the dev server', async () => {
    const seen: { cookie?: string; url?: string }[] = [];
    const dev = http.createServer((req, res) => {
      seen.push({ cookie: req.headers.cookie, url: req.url });
      res.end(`dev ${req.url}`);
    });
    await new Promise<void>((r) => dev.listen(0, '127.0.0.1', () => r()));
    const port = (dev.address() as any).port;
    const r = await call('POST', '/api/previews', { body: { port } });
    assert.equal(r.status, 200);
    const pv = r.json.preview;
    assert.equal(pv.origin, 'https://p1.briardr.test');
    assert.ok(pv.launch.startsWith(`https://p1.briardr.test${LAUNCH_PATH}?t=`));
    const lp = pv.listenPort;
    const launchPath = new URL(pv.launch).pathname + new URL(pv.launch).search + '&path=' + encodeURIComponent('/app?x=1');
    const at = (p: string, o: { cookie?: string | null; host?: string } = {}) => call('GET', p, { port: lp, host: o.host ?? 'p1.briardr.test', cookie: o.cookie ?? null, origin: null });

    assert.equal((await at('/', { cookie })).status, 401, 'the device cookie alone opens nothing on the preview origin');
    assert.equal((await at(launchPath, { host: 'p2.briardr.test' })).status, 421, 'only this slot’s host');
    const launched = await at(launchPath);
    assert.equal(launched.status, 303);
    assert.equal(launched.headers.get('location'), '/app?x=1');
    const set = launched.headers.get('set-cookie') || '';
    assert.match(set, new RegExp(`^${PREVIEW_COOKIE}=[^;]+; Path=/; Secure; HttpOnly; SameSite=Strict$`));
    const session = set.split(';')[0];
    assert.equal((await at(launchPath)).status, 401, 'a launch link works once');
    assert.equal((await at(launchPath, { cookie: session })).status, 303, 'reloading the used link with a session still works');

    // A dev server's own WebSocket (HMR) through the preview origin.
    const hmr = Bun.serve({ hostname: '127.0.0.1', port: 0, fetch: (req, srv) => (srv.upgrade(req) ? undefined : new Response('no')), websocket: { message: (w, m) => void w.send(`echo:${m}`) } });
    const hp = (await call('POST', '/api/previews', { body: { port: hmr.port } })).json.preview;
    const hl = await call('GET', new URL(hp.launch).pathname + new URL(hp.launch).search, { port: hp.listenPort, host: new URL(hp.origin).host, cookie: null, origin: null });
    const hs = (hl.headers.get('set-cookie') || '').split(';')[0];
    const sock = new WebSocket(`ws://127.0.0.1:${hp.listenPort}/hmr`, { headers: { host: new URL(hp.origin).host, cookie: hs, origin: hp.origin } } as any);
    const echoed = await new Promise<string>((resolve, reject) => {
      sock.addEventListener('open', () => sock.send('ping'));
      sock.addEventListener('message', (e) => resolve(String(e.data)));
      sock.addEventListener('error', () => reject(new Error('preview WebSocket failed')));
      setTimeout(() => reject(new Error('preview WebSocket timed out')), 5000);
    });
    assert.equal(echoed, 'echo:ping');
    sock.close();
    await call('DELETE', `/api/previews/${hp.id}`);
    hmr.stop(true);

    const ok = await at('/app?x=1', { cookie: `${session}; ${cookie}; theme=dark` });
    assert.equal(ok.status, 200);
    assert.equal(ok.body, 'dev /app?x=1');
    assert.equal(seen.at(-1)!.cookie, 'theme=dark', 'neither Briardr cookie reaches the dev server');

    const fresh = (await call('POST', '/api/previews', { body: { port } })).json.preview;
    assert.equal(fresh.id, pv.id, 'the same preview, with a new link');
    const off = await at(new URL(fresh.launch).pathname + new URL(fresh.launch).search + '&path=' + encodeURIComponent('//evil.example/'));
    assert.equal(off.headers.get('location'), '/', 'never redirects off the preview origin');

    // Revoking the device ends the preview and its session.
    const { code } = s.run.auth.createPairingCode();
    const other = s.run.auth.pair(code, 'other');
    assert.ok(other.ok);
    const r2 = await call('POST', '/api/previews', { body: { port }, cookie: `__Host-briardr=${other.cookie}` });
    const p2 = r2.json.preview;
    assert.equal(p2.origin, 'https://p2.briardr.test', 'each device’s preview gets its own slot');
    const l2 = await call('GET', new URL(p2.launch).pathname + new URL(p2.launch).search, { port: p2.listenPort, host: 'p2.briardr.test', cookie: null, origin: null });
    const s2 = (l2.headers.get('set-cookie') || '').split(';')[0];
    assert.equal((await call('GET', '/', { port: p2.listenPort, host: 'p2.briardr.test', cookie: session, origin: null })).status, 401, 'one preview’s session doesn’t open another');
    assert.equal((await call('GET', '/', { port: p2.listenPort, host: 'p2.briardr.test', cookie: s2, origin: null })).status, 200);
    s.run.auth.revoke(other.device.id);
    await sleep(200);
    await assert.rejects(call('GET', '/', { port: p2.listenPort, host: 'p2.briardr.test', cookie: s2, origin: null }), 'revocation closed the preview');

    assert.equal((await call('DELETE', `/api/previews/${pv.id}`)).json.ok, true);
    await sleep(200);
    await assert.rejects(at('/', { cookie: session }));
    dev.close();
  });
});

describe('proxy mode with previews off and a non-loopback proxy', () => {
  let s: TestServer;
  beforeAll(async () => {
    s = await startServer({ pair: false, config: { tls: { mode: 'proxy' }, publicOrigin: ORIGIN, trustedProxies: ['192.0.2.1'] } });
  });
  afterAll(() => s.close());

  test('only the configured proxy may connect', async () => {
    const res = await fetch(`http://127.0.0.1:${s.port}/api/session`, { headers: { host: 'briardr.test' } });
    assert.equal(res.status, 403);
  });

  test('previews are refused when no preview origins are configured', async () => {
    const { code } = s.run.auth.createPairingCode();
    const p = s.run.auth.pair(code, 'x');
    assert.ok(p.ok);
    // The peer check runs first, so ask the preview manager directly.
    assert.deepEqual(await (s.run.app as any).d.previews.open(p.device.id, 1), { ok: false, error: 'unavailable' });
  });
});
