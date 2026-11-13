// Briardr on herdr-core, the Herdr app's runtime, against a throwaway core of this run's own (never yours):
// - the model: the app's layout document and its labels place terminals; Briardr creates, renames and closes
//   in that document as the app does, and hides a tab whose terminals all ended;
// - typing: keys, pastes and clicks arrive encoded for the modes the program turned on, and the browser
//   terminal's replies as its own;
// - seen: shared with every Herdr client through `herdr.seen`, and a finished agent alerts;
// - another machine (a second core joined as one): its terminals are shown, and its files never looked for here.
import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import crypto from 'node:crypto';
import { request, startCore, startServer, tmpDir, until, waitUntil, ws, type TestCore, type TestServer } from './helpers.ts';

/** Reports each read from its terminal as a hex line, after turning on the modes its arguments name. */
const ECHO = `
import os, sys, tty
modes = {"kitty": "\\x1b[>1u", "paste": "\\x1b[?2004h", "mouse": "\\x1b[?1002h\\x1b[?1006h"}
tty.setraw(0)
for m in sys.argv[1:]:
    os.write(1, modes[m].encode())
os.write(1, b"ready\\r\\n")
while True:
    os.write(1, (os.read(0, 1024).hex(" ") + "\\r\\n").encode())
`;
/** An agent as far as the core can tell (OSC 7501): it works for a moment, then is done. */
const AGENT = `printf '\\033]7501;state=working:app=claude-code\\033\\\\'; sleep 1; printf '\\033]7501;state=done:app=claude-code\\033\\\\'; exec sleep 600`;

const keys = () => {
  const ecdh = crypto.createECDH('prime256v1');
  ecdh.generateKeys();
  return { p256dh: ecdh.getPublicKey('base64url'), auth: Buffer.alloc(16, 7).toString('base64url') };
};

let core: TestCore;
let s: TestServer;
const model = async () => (await request(s, 'GET', '/api/state')).json.model;
const layout = async () => (await core.call('doc.get', { name: 'herdr-app' })).value;
const stream = async (id: string) => (await model())?.streams.find((x: { id: string }) => x.id === id);
const run = (...argv: string[]) => JSON.parse(core.hc('run', '--bg', ...argv)).address as string;
/** What a terminal running ECHO has read, one hex line per read. */
const reads = (t: string) => core.hc('read', t).split('\n').map((l) => l.trim()).filter((l) => /^[0-9a-f]{2}( [0-9a-f]{2})*$/.test(l));

beforeAll(async () => {
  core = startCore();
  s = await startServer({ config: { herdrBackend: 'core', herdrCoreSocket: core.socket, notifications: { debounceMs: 200 } } });
  await waitUntil('herdr-core', async () => (await request(s, 'GET', '/api/state')).json?.herdr?.ok === true);
});
afterAll(() => s?.close());

describe('the layout', () => {
  test('workspaces and projects are added and renamed in the app’s document; a new stream is labelled for its project', async () => {
    const dir = tmpDir('briardr-ws-');
    const w = await request(s, 'POST', '/api/workspaces', { name: 'api', folder: dir });
    assert.equal(w.status, 200, w.body);
    assert.equal((await request(s, 'GET', '/api/state')).json.herdr.backend, 'core');
    const p = await request(s, 'POST', '/api/projects', { workspaceId: w.json.workspaceId, name: 'review' });
    assert.equal(p.status, 200, p.body);
    const st = await request(s, 'POST', '/api/streams', { projectId: p.json.projectId, kind: 'shell' });
    assert.equal(st.status, 200, st.body);

    const doc = await layout();
    const space = doc.spaces.find((x: { id: string }) => x.id === w.json.workspaceId);
    assert.deepEqual(
      [space.name, space.folder, space.tabs.map((t: { name: string }) => t.name)],
      ['api', dir, ['', 'review']],
      'as the app writes them: an unnamed first tab is called by its number',
    );
    assert.deepEqual(space.tabs[1].root, { id: p.json.projectId, type: 'row', children: [{ id: space.tabs[1].root.children[0].id, type: 'block', content: { terminal: p.json.streamId } }] });
    const labels = JSON.parse(core.hc('get', st.json.streamId)).labels;
    assert.deepEqual([labels['herdr.space'], labels['herdr.tab'], labels['herdr.seen']], [w.json.workspaceId, p.json.projectId, '0'], 'the app places it from its labels');
    const m = await model();
    const names = (ws: string) => m.projects.filter((x: { workspaceId: string }) => x.workspaceId === ws).map((x: { name: string }) => x.name);
    assert.deepEqual(names(w.json.workspaceId), ['1', 'review']);
    assert.equal(m.streams.filter((x: { projectId: string }) => x.projectId === p.json.projectId).length, 2, 'the labelled stream is there before the app adopts it');

    assert.equal((await request(s, 'POST', `/api/workspaces/${w.json.workspaceId}/rename`, { name: 'api v2' })).status, 200);
    assert.equal((await request(s, 'POST', `/api/projects/${p.json.projectId}/rename`, { name: 'checks' })).status, 200);
    const renamed = (await layout()).spaces.find((x: { id: string }) => x.id === w.json.workspaceId);
    assert.deepEqual([renamed.name, renamed.tabs[1].name], ['api v2', 'checks']);
    const named = await request(s, 'POST', `/api/streams/${st.json.streamId}/rename`, { name: 'x' });
    assert.equal(named.status, 501, 'the app’s terminals have no names');

    // Closing ends terminals; the tab goes with its last one, as the app closes it.
    assert.equal((await request(s, 'POST', `/api/projects/${p.json.projectId}/close`)).status, 200);
    assert.equal(JSON.parse(core.hc('get', st.json.streamId)).state, 'finished');
    assert.ok(!(await model()).projects.some((x: { id: string }) => x.id === p.json.projectId));
    assert.equal((await request(s, 'POST', `/api/workspaces/${w.json.workspaceId}/close`)).status, 200);
    assert.ok(!(await model()).workspaces.some((x: { id: string }) => x.id === w.json.workspaceId));
  });
});

describe('typing', () => {
  test('keys, pastes and clicks arrive encoded for the program’s modes; the browser terminal’s replies go as its own', async () => {
    const t = run('python3', '-c', ECHO, 'kitty', 'paste', 'mouse');
    await waitUntil('the program', async () => core.hc('read', t).includes('ready') && !!(await stream(t)));
    const o = ws(s, `/ws/term?v=1&pane=${t}&mode=control&cols=70&rows=20&session=core`);
    const opened = await until(o, (m) => m.t === 'live');
    assert.ok(opened.some((m) => m.t === 'hello' && m.raw === true), 'the program’s own output: the browser keeps its history');
    assert.equal(JSON.parse(core.hc('get', t)).cols, 70, 'the controller’s size');
    const send = async (m: object) => {
      const before = reads(t).length;
      o.send(JSON.stringify(m));
      await waitUntil('the read', async () => reads(t).length > before);
      return reads(t).slice(before).join(' ');
    };
    assert.equal(await send({ t: 'input', id: 1, keys: ['ctrl+c'] }), '1b 5b 39 39 3b 35 75', 'kitty: CSI 99;5u');
    assert.equal(await send({ t: 'input', id: 2, text: 'hi' }), '68 69');
    assert.equal(await send({ t: 'input', id: 3, paste: { text: 'p\x1bq', bracketed: true } }), '1b 5b 32 30 30 7e 70 71 1b 5b 32 30 31 7e', 'bracketed, and sanitized');
    assert.equal(await send({ t: 'mouse', action: 'down', button: 'left', column: 3, row: 1, modifiers: 0 }), '1b 5b 3c 30 3b 34 3b 32 4d', 'SGR');
    assert.equal(await send({ t: 'reply', data: '\x1b[?1;2c' }), '1b 5b 3f 31 3b 32 63');
    o.close();
    core.hc('kill', t);
  });
});

describe('seen and alerts', () => {
  test('a finished agent alerts; seen and unread are shared through herdr.seen', async () => {
    const pushes: string[] = [];
    s.run.push.sendImpl = async (_sub, body) => (pushes.push(body), { statusCode: 201 });
    assert.equal((await request(s, 'POST', '/api/notifications/subscribe', { subscription: { endpoint: 'https://web.push.apple.com/core', keys: keys() } })).status, 200);
    const t = run('sh', '-c', AGENT);
    await waitUntil('done', async () => (await stream(t))?.state === 'done');
    await waitUntil('the alert', async () => {
      await request(s, 'GET', '/api/state'); // hands the pushes so far to sendImpl
      return pushes.some((p) => JSON.parse(p).kind === 'finished');
    });

    assert.deepEqual((await request(s, 'POST', `/api/streams/${t}/seen`, { seen: true })).json, { shared: true });
    await waitUntil('idle', async () => (await stream(t))?.state === 'idle');
    assert.equal(JSON.parse(core.hc('get', t)).labels['herdr.seen'], '1', 'the turn it saw');
    await request(s, 'POST', `/api/streams/${t}/seen`, { seen: false });
    await waitUntil('done again', async () => (await stream(t))?.state === 'done');
    core.hc('kill', t);
  });
});

describe('another machine', () => {
  test('its spaces and terminals are shown as the core addresses them, and its files aren’t looked for here', async () => {
    const box = startCore('box');
    core.hc('machine', 'add', '--name', 'box', '--via', box.via);
    await waitUntil('box connected', async () => core.hc('machine', 'ls').includes('"connected"'));
    const t = run('--on', 'box', 'sh');
    assert.match(t, /^box\/t\d+$/);
    await core.call('doc.set', { name: 'herdr-app', on: 'box', version: 0, value: { spaces: [{ id: 'sp1', name: 'far', folder: '/srv', tabs: [{ id: 'tb1', name: '', root: { id: 'tb1', type: 'row', children: [{ id: 'p1', type: 'block', content: { terminal: t.split('/')[1] } }] } }] }] } });
    await waitUntil('its space', async () => (await model()).workspaces.some((w: { id: string }) => w.id === 'box/sp1'));
    const m = await model();
    assert.equal(m.workspaces.find((w: { id: string }) => w.id === 'box/sp1').machine, 'box');
    const st = m.streams.find((x: { id: string }) => x.id === t);
    assert.deepEqual([st.machine, st.cwd, st.projectId], ['box', null, 'box/tb1']);
    const changes = await request(s, 'GET', `/api/streams/${encodeURIComponent(t)}/changes`);
    assert.equal(changes.status, 409);
    assert.match(changes.body, /runs on box/);
    assert.equal((await request(s, 'POST', `/api/workspaces/${encodeURIComponent('box/sp1')}/rename`, { name: 'near' })).status, 200);
    assert.equal((await core.call('doc.get', { name: 'herdr-app', on: 'box' })).value.spaces[0].name, 'near', 'in that machine’s document');
    core.hc('kill', t);
  });
});
