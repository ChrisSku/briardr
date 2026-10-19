// Safety rule: every request that reads or controls work is authenticated.
import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { closed, request, startServer, until, upgradeStatus, ws, type TestServer } from './helpers.ts';

let s: TestServer;
beforeAll(async () => {
  s = await startServer();
});
afterAll(async () => {
  await s.close();
});

function concrete(pattern: string): string {
  // Turn the route regex back into a path with placeholder params.
  return pattern.replace(/^\^/, '').replace(/\$$/, '').replace(/\(\[\^\/\]\+\)/g, 'w1%3Ap1').replace(/\\\//g, '/');
}

describe('authentication', () => {
  test('every non-public API route rejects requests without a device cookie', async () => {
    const routes = s.run.app.listRoutes();
    assert.ok(routes.length > 30, 'route list looks complete');
    const publicOnes = routes.filter((r) => r.public).map((r) => concrete(r.pattern));
    assert.deepEqual(publicOnes.sort(), ['/api/ca.mobileconfig', '/api/pair', '/api/session'].sort(), 'only session probe, pairing and the public CA profile are public');
    for (const r of routes.filter((x) => !x.public)) {
      const p = concrete(r.pattern);
      const res = await request(s, r.method, p, r.method === 'GET' ? undefined : {}, { cookie: false });
      assert.equal(res.status, 401, `${r.method} ${p} must require auth (got ${res.status})`);
    }
  });

  test('a forged or wrong cookie is rejected', async () => {
    const forged = s.cookie.replace(/\.[^.]+$/, '.AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA');
    assert.equal((await request(s, 'GET', '/api/state', undefined, { cookie: forged })).status, 401);
    assert.equal((await request(s, 'GET', '/api/state', undefined, { cookie: '__Host-briardr=nope' })).status, 401);
    assert.equal((await request(s, 'GET', '/api/state')).status, 200);
  });

  test('the device cookie is HttpOnly, Secure, SameSite=Strict and never in a URL or body', async () => {
    const { code } = s.run.auth.createPairingCode();
    const r = await request(s, 'POST', '/api/pair', { code, name: 'cookie check' }, { cookie: false });
    const c = String(r.headers['set-cookie'][0]);
    assert.match(c, /^__Host-briardr=/);
    assert.match(c, /HttpOnly/);
    assert.match(c, /Secure/);
    assert.match(c, /SameSite=Strict/);
    const token = c.split(';')[0].split('=')[1];
    assert.ok(!r.body.includes(token.split('.')[1]), 'token not echoed in the response body');
  });

  test('WebSockets require auth and a same-origin request', async () => {
    assert.equal(await upgradeStatus(s, '/ws/live?v=1', { cookie: false }), 401);
    assert.equal(await upgradeStatus(s, '/ws/term?v=1&pane=w1:p1&mode=observe', { cookie: false }), 401);
    assert.equal(await upgradeStatus(s, '/ws/live?v=1', { origin: 'https://evil.example' }), 403);
    assert.equal(await upgradeStatus(s, '/ws/live?v=1'), 101);
    const ok = ws(s, '/ws/live?v=1');
    const msgs = await until(ok, (m) => m.t === 'model');
    assert.ok(msgs.length);
    const hello = msgs.find((m) => m.t === 'hello');
    assert.match(hello?.instanceId, /^[a-f0-9]{32}$/);
    assert.equal(hello.instanceId, (await request(s, 'GET', '/api/session')).json.instanceId);
    ok.close();
  });

  test('state-changing requests need Briardr’s own origin', async () => {
    const r = await request(s, 'POST', '/api/projects', { workspaceId: 'w1' }, { origin: 'https://localhost:9999' });
    assert.equal(r.status, 403);
    const r2 = await request(s, 'POST', '/api/projects', { workspaceId: 'w1' }, { origin: null, headers: { 'sec-fetch-site': 'cross-site' } });
    assert.equal(r2.status, 403);
  });

  test('unknown Host headers are refused (DNS rebinding)', async () => {
    const r = await request(s, 'GET', '/api/state', undefined, { host: 'attacker.example' });
    assert.equal(r.status, 421);
  });

  test('pairing codes are single-use and lock after repeated wrong guesses', async () => {
    const { code } = s.run.auth.createPairingCode();
    for (let i = 0; i < 4; i++) assert.equal((await request(s, 'POST', '/api/pair', { code: 'ZZZZ-ZZZZ', name: 'x' }, { cookie: false })).json.error, 'invalid');
    assert.equal((await request(s, 'POST', '/api/pair', { code: 'ZZZZ-ZZZZ', name: 'x' }, { cookie: false })).json.error, 'locked');
    assert.equal((await request(s, 'POST', '/api/pair', { code, name: 'x' }, { cookie: false })).status, 400, 'the real code no longer works after lockout');
    const fresh = s.run.auth.createPairingCode();
    assert.equal((await request(s, 'POST', '/api/pair', { code: fresh.code, name: 'once' }, { cookie: false })).status, 200);
    assert.equal((await request(s, 'POST', '/api/pair', { code: fresh.code, name: 'twice' }, { cookie: false })).json.error, 'no_code');
  });

  test('revoking a device cuts it off immediately, including live sockets', async () => {
    // (Paired in-process: the HTTP pairing route is rate-limited by the tests above, as it should be.)
    const { code } = s.run.auth.createPairingCode();
    const p = s.run.auth.pair(code, 'to revoke');
    assert.ok(p.ok);
    const cookie = `__Host-briardr=${p.cookie}`;
    const id = p.device.id;
    const live = ws(s, '/ws/live?v=1', { cookie });
    await until(live, (m) => m.t === 'model');
    const closing = closed(live);
    assert.equal((await request(s, 'DELETE', `/api/devices/${id}`)).status, 200);
    assert.equal(await closing, 4401);
    assert.equal((await request(s, 'GET', '/api/state', undefined, { cookie })).status, 401);
  });

  test('security headers are present on app and API responses', async () => {
    const r = await request(s, 'GET', '/api/state');
    assert.match(String(r.headers['content-security-policy']), /frame-ancestors 'none'/);
    assert.equal(r.headers['x-content-type-options'], 'nosniff');
    assert.equal(r.headers['cache-control'], 'no-store');
  });
});
