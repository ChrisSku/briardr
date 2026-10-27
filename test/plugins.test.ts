// Plugins: built-ins on by default, turning one off refuses its routes, installed plugins start off,
// and a plugin with a backend needs its command approved before it can be turned on.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { buildPlugin, closeWorkspace, installPlugin, opened, request, startServer, testPane, until, ws, type TestServer } from './helpers.ts';

let s: TestServer;
let pane: string;
let workspace: string;
beforeAll(async () => {
  s = await startServer();
  ({ pane, workspace } = testPane('briardr-test-plugins'));
  await s.run.herdr.refresh();
});
afterAll(async () => {
  closeWorkspace(workspace);
  await s.close();
});

const api = (m: string, p: string, b?: unknown) => request(s, m, p, b);
const enc = encodeURIComponent;
const find = (list: any[], id: string) => list.find((p) => p.id === id);

function install(id: string, manifest: object) {
  const dir = path.join(s.dataDir, 'plugins', id);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, 'briardr-plugin.json'), JSON.stringify({ id, name: id, briardr: 1, ...manifest }));
}

describe('plugins', () => {
  test('lists the built-ins, all on, in tab order, also in the session', async () => {
    const r = await api('GET', '/api/plugins');
    assert.equal(r.status, 200);
    const builtins = r.json.plugins.filter((p: any) => p.builtin);
    assert.deepEqual(
      builtins.map((p: any) => p.id),
      ['files', 'changes', 'preview', 'docs', 'notes', 'usage'],
    );
    assert.ok(builtins.every((p: any) => p.enabled));
    // Usage is a page in Settings, not an app tab.
    assert.deepEqual(find(r.json.plugins, 'usage').contributes.settings, [{ id: 'usage', label: 'Usage' }]);
    assert.deepEqual(find(r.json.plugins, 'usage').contributes.appTabs, []);
    const session = await api('GET', '/api/session');
    assert.deepEqual(session.json.plugins, r.json.plugins);
  });

  test('needs a signed-in device', async () => {
    assert.equal((await request(s, 'GET', '/api/plugins', undefined, { cookie: false })).status, 401);
    assert.equal((await request(s, 'PUT', '/api/plugins/files', { enabled: false }, { cookie: false })).status, 401);
  });

  test('a built-in turned off refuses its routes until turned on again', async () => {
    const files = `/api/streams/${enc(pane)}/files`;
    assert.equal((await api('GET', files)).status, 200);
    const off = await api('PUT', '/api/plugins/files', { enabled: false });
    assert.equal(off.status, 200);
    assert.equal(find(off.json.plugins, 'files').enabled, false);
    const refused = await api('GET', `${files}/search?q=x`);
    assert.equal(refused.status, 404);
    assert.equal(refused.json.error, 'plugin_disabled');
    // Other features keep working.
    assert.equal((await api('GET', `/api/streams/${enc(pane)}/changes`)).status, 200);
    const state = () => JSON.parse(fs.readFileSync(path.join(s.dataDir, 'state', 'plugins.json'), 'utf8'));
    assert.deepEqual(state().enabled, { files: false });
    assert.equal((await api('PUT', '/api/plugins/files', { enabled: true })).status, 200);
    assert.equal((await api('GET', files)).status, 200);
    // Back to the default: nothing left for it.
    assert.deepEqual(state().enabled, {});
  });

  test('a change made on the host while Briardr runs (briardr plugin disable) applies at once', async () => {
    const file = path.join(s.dataDir, 'state', 'plugins.json');
    fs.writeFileSync(file, JSON.stringify({ enabled: { docs: false } }));
    const r = await api('GET', `/api/streams/${enc(pane)}/docs`);
    assert.equal(r.json.error, 'plugin_disabled');
    assert.equal(find((await api('GET', '/api/plugins')).json.plugins, 'docs').enabled, false);
    fs.writeFileSync(file, JSON.stringify({ enabled: {} }));
    assert.equal((await api('GET', `/api/streams/${enc(pane)}/docs`)).status, 200);
  });

  test('installed plugins start off; broken ones say why and stay off', async () => {
    install('ci', { client: 'src/index.ts', contributes: { streamTabs: [{ id: 'ci', label: 'CI' }] } });
    install('clash', { client: 'src/index.ts', contributes: { streamTabs: [{ id: 'files', label: 'Mine' }] } });
    const list = (await api('GET', '/api/plugins')).json.plugins;
    assert.equal(find(list, 'ci').enabled, false);
    assert.equal(find(list, 'ci').builtin, false);
    assert.match(find(list, 'clash').problem, /already uses/);
    const on = await api('PUT', '/api/plugins/ci', { enabled: true });
    assert.equal(on.status, 200);
    assert.equal(find(on.json.plugins, 'ci').enabled, true);
    const broken = await api('PUT', '/api/plugins/clash', { enabled: true });
    assert.equal(broken.status, 409);
    assert.equal(broken.json.error, 'plugin_broken');
    assert.equal((await api('PUT', '/api/plugins/nope', { enabled: true })).status, 404);
  });

  test('a backend needs the command the user approved, again when it changes', async () => {
    install('runner', { backend: { command: ['bun', 'server.ts'] } });
    const listed = find((await api('GET', '/api/plugins')).json.plugins, 'runner');
    assert.deepEqual(listed.backend.command, ['bun', 'server.ts']);
    assert.equal(listed.backend.approved, false);
    const unapproved = await api('PUT', '/api/plugins/runner', { enabled: true });
    assert.equal(unapproved.status, 409);
    assert.equal(unapproved.json.error, 'approval_needed');
    const wrong = await api('PUT', '/api/plugins/runner', { enabled: true, approve: 'f'.repeat(64) });
    assert.equal(wrong.status, 409);
    const ok = await api('PUT', '/api/plugins/runner', { enabled: true, approve: listed.backend.hash });
    assert.equal(ok.status, 200);
    assert.equal(find(ok.json.plugins, 'runner').backend.approved, true);
    // The command changed on disk: off until the new command is approved.
    install('runner', { backend: { command: ['bun', 'other.ts'] } });
    const changed = find((await api('GET', '/api/plugins')).json.plugins, 'runner');
    assert.equal(changed.backend.approved, false);
    assert.equal(changed.enabled, false);
    assert.equal((await api('PUT', '/api/plugins/runner', { enabled: true })).json.error, 'approval_needed');
  });

  test('an installed plugin’s client is served to signed-in devices while it’s on, built for this Briardr', async () => {
    const dir = installPlugin(s.dataDir, 'echo');
    let echo = find((await api('GET', '/api/plugins')).json.plugins, 'echo');
    assert.equal(echo.build, 'ok');
    assert.equal(echo.client, null, 'no client URL while off');
    assert.equal((await api('PUT', '/api/plugins/echo', { enabled: true, approve: echo.backend.hash })).status, 200);
    echo = find((await api('GET', '/api/plugins')).json.plugins, 'echo');
    assert.match(echo.client, /^\/plugins\/echo\/index\.[a-z0-9]+\.js$/);
    const js = await api('GET', echo.client);
    assert.equal(js.status, 200);
    assert.match(js.headers['content-type'], /javascript/);
    assert.match(js.body, /from"\/sdk\/v1\/svelte-internal-client\.js"/);
    assert.equal((await request(s, 'GET', echo.client, undefined, { cookie: false })).status, 401);
    assert.equal((await api('GET', '/plugins/echo/build.json')).status, 404);
    assert.equal((await api('GET', '/plugins/echo/..%2Fbriardr-plugin.json')).status, 404);
    assert.equal((await api('GET', '/plugins/echo/../briardr-plugin.json')).status, 404);
    // Built for another runtime: listed as stale, not served.
    const stamp = path.join(dir, 'dist', 'build.json');
    fs.writeFileSync(stamp, JSON.stringify({ ...JSON.parse(fs.readFileSync(stamp, 'utf8')), runtime: 'svelte@4.0.0+sdk1' }));
    echo = find((await api('GET', '/api/plugins')).json.plugins, 'echo');
    assert.equal(echo.build, 'stale');
    assert.equal(echo.client, null);
    buildPlugin(dir);
    assert.equal(find((await api('GET', '/api/plugins')).json.plugins, 'echo').build, 'ok');
    assert.equal((await api('PUT', '/api/plugins/echo', { enabled: false })).status, 200);
    assert.equal((await api('GET', echo.client ?? '/plugins/echo/index.js')).status, 404);
  });

  test('a backend answers through Briardr, with the stream and device, and pushes events to devices', async () => {
    const hash = find((await api('GET', '/api/plugins')).json.plugins, 'echo').backend.hash;
    assert.equal((await api('GET', '/api/plugins/echo/hello')).json.error, 'plugin_disabled');
    assert.equal((await api('PUT', '/api/plugins/echo', { enabled: true, approve: hash })).status, 200);
    const hello = await api('GET', `/api/plugins/echo/hello?stream=${enc(pane)}&a=1`);
    assert.equal(hello.status, 200);
    assert.equal(hello.json.stream, pane);
    assert.equal(typeof hello.json.cwd, 'string');
    assert.deepEqual(hello.json.query, { stream: pane, a: '1' });
    assert.equal((await request(s, 'GET', '/api/plugins/echo/hello', undefined, { cookie: false })).status, 401);
    assert.equal((await api('GET', `/api/plugins/echo/hello?stream=nope`)).json.error, 'stream_gone');
    // Bodies in, JSON or text out; only the allowed headers come back.
    const echoed = await api('POST', '/api/plugins/echo/echo', { x: [1, 2] });
    assert.deepEqual(echoed.json, { method: 'POST', body: { x: [1, 2] }, text: null, base64: null });
    const text = await api('GET', '/api/plugins/echo/text');
    assert.equal(text.body, 'plain words');
    assert.equal(text.headers['x-not-passed'], undefined);
    assert.match(text.headers['content-security-policy'], /script-src 'self'/);
    // Events reach the live connection.
    const sock = ws(s, '/ws/live?v=1');
    await opened(sock);
    const got = until(sock, (m) => m.t === 'plugin_event');
    assert.equal((await api('POST', '/api/plugins/echo/emit')).status, 204);
    const ev = (await got).find((m) => m.t === 'plugin_event');
    assert.deepEqual([ev.plugin, ev.topic, typeof ev.data.hits], ['echo', 'note', 'number']);
    sock.terminate();
    // Its own data folder.
    const data = await api('GET', '/api/plugins/echo/data');
    assert.equal(fs.readFileSync(path.join(data.json.dataDir, 'saved.txt'), 'utf8'), 'kept');
    assert.ok(data.json.dataDir.startsWith(path.join(s.dataDir, 'plugin-data')));
    // A crash fails that request; the next one starts it again.
    const pid = (await api('GET', '/api/plugins/echo/hello')).json.pid;
    assert.equal((await api('GET', '/api/plugins/echo/crash')).json.error, 'backend_gone');
    const again = await api('GET', '/api/plugins/echo/hello');
    assert.equal(again.status, 200);
    assert.notEqual(again.json.pid, pid);
    // Turned off: refused, and the process is gone.
    assert.equal((await api('PUT', '/api/plugins/echo', { enabled: false })).status, 200);
    assert.equal((await api('GET', '/api/plugins/echo/hello')).json.error, 'plugin_disabled');
    assert.throws(() => process.kill(again.json.pid, 0));
  });
});
