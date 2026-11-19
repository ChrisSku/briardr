// Failed persistence must be visible to callers; concurrent settings changes must both survive.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, onTestFinished, test } from 'bun:test';
import { closed, request, sleep, startServer, until, waitUntil, ws, type TestServer } from './helpers.ts';

let s: TestServer;
beforeAll(async () => { s = await startServer(); });
afterAll(async () => { await s.close(); });

test('failed revocation reports an error without clearing the cookie or closing live sockets', async () => {
  const p = s.run.auth.pair(s.run.auth.createPairingCode().code, 'revocation failure');
  assert.ok(p.ok);
  const cookie = `__Host-briardr=${p.cookie}`;
  const live = ws(s, '/ws/live?v=1', { cookie });
  onTestFinished(() => live.terminate());
  await until(live, (m) => m.t === 'model');
  const file = path.join(s.dataDir, 'secrets', 'devices.json');
  const backup = `${file}.backup`;
  // A directory at the destination reliably refuses the atomic rename, including tests run as root.
  fs.renameSync(file, backup);
  fs.mkdirSync(file);
  try {
    const out = await request(s, 'POST', '/api/signout', {}, { cookie });
    assert.equal(out.status, 500);
    assert.equal(out.headers['set-cookie']?.length ?? 0, 0, 'a failed sign-out keeps the browser credential');
    assert.equal((await request(s, 'DELETE', `/api/devices/${p.device.id}`)).status, 500);
    assert.equal((await request(s, 'GET', '/api/session', undefined, { cookie })).status, 200);
    const pong = until(live, (m) => m.t === 'pong');
    live.send(JSON.stringify({ t: 'ping' }));
    await pong;
  } finally {
    fs.rmdirSync(file);
    fs.renameSync(backup, file);
  }
  try {
    const closing = closed(live);
    const out = await request(s, 'POST', '/api/signout', {}, { cookie });
    assert.equal(out.status, 200);
    assert.match(String(out.headers['set-cookie']), /Max-Age=0/);
    assert.equal(await closing, 4401);
    assert.equal((await request(s, 'GET', '/api/session', undefined, { cookie })).status, 401);
    assert.equal((await request(s, 'DELETE', `/api/devices/${p.device.id}`)).status, 404);
  } finally {
    live.terminate();
  }
});

const quote = (s: string) => `'${s.replaceAll("'", "'\\''")}'`;

test('a settings save waiting for Herdr validation preserves another accepted patch on disk', async () => {
  const original = (await request(s, 'GET', '/api/settings')).json;
  await request(s, 'PUT', '/api/settings', { notifications: { needsYou: true } });
  const wrapper = path.join(s.dataDir, 'bin', 'waiting-herdr');
  const entered = path.join(s.dataDir, 'validation-entered');
  const release = path.join(s.dataDir, 'validation-release');
  fs.writeFileSync(wrapper, `#!/bin/sh\nprintf ready > ${quote(entered)}\nwhile [ ! -f ${quote(release)} ]; do sleep 0.02; done\nexec ${quote(original.herdrPath)} "$@"\n`, { mode: 0o700 });
  const first = request(s, 'PUT', '/api/settings', { herdrPath: wrapper });
  let second: ReturnType<typeof request> | undefined;
  try {
    await waitUntil('Herdr validation started', () => fs.existsSync(entered));
    let secondDone = false;
    second = request(s, 'PUT', '/api/settings', { notifications: { needsYou: false } }).then((r) => {
      secondDone = true;
      return r;
    });
    await sleep(100);
    assert.equal(secondDone, false, 'the next transaction waits for validation and persistence');
    fs.writeFileSync(release, '');
    assert.equal((await first).status, 200);
    assert.equal((await second).status, 200);
    const cfg = (await request(s, 'GET', '/api/settings')).json;
    const disk = JSON.parse(fs.readFileSync(path.join(s.dataDir, 'config.json'), 'utf8'));
    for (const value of [cfg, disk]) {
      assert.equal(value.herdrPath, wrapper);
      assert.equal(value.notifications.needsYou, false);
    }
  } finally {
    fs.writeFileSync(release, '');
    await first;
    await second;
    await request(s, 'PUT', '/api/settings', { herdrPath: original.herdrPath, notifications: original.notifications });
  }
});

test('a failed settings write leaves the effective configuration unchanged', async () => {
  const original = (await request(s, 'GET', '/api/settings')).json;
  const file = path.join(s.dataDir, 'config.json');
  const backup = `${file}.backup`;
  fs.renameSync(file, backup);
  fs.mkdirSync(file);
  try {
    const out = await request(s, 'PUT', '/api/settings', { notifications: { ready: !original.notifications.ready } });
    assert.equal(out.status, 500);
    assert.deepEqual((await request(s, 'GET', '/api/settings')).json.notifications, original.notifications);
  } finally {
    fs.rmdirSync(file);
    fs.renameSync(backup, file);
  }
});

test('a settings change in the app keeps what `briardr configure` saved for the restart', async () => {
  const disk = () => JSON.parse(fs.readFileSync(path.join(s.dataDir, 'config.json'), 'utf8'));
  const { ready } = (await request(s, 'GET', '/api/settings')).json.notifications;
  assert.match(s.run.cli('configure', '--allowed-networks', '10.8.0.0/24'), /Restart Briardr to apply/);
  try {
    assert.equal((await request(s, 'PUT', '/api/settings', { notifications: { ready: !ready } })).status, 200);
    assert.deepEqual(disk().allowedNetworks, ['10.8.0.0/24'], 'the command line’s change waits for the restart');
    assert.equal(disk().notifications.ready, !ready);
  } finally {
    s.run.cli('configure', '--allowed-networks', '');
    await request(s, 'PUT', '/api/settings', { notifications: { ready } });
  }
  assert.deepEqual(disk().allowedNetworks, []);
  assert.equal(disk().notifications.ready, ready);
});

test('`briardr configure` changes only what it is given, and refuses a config.json it can’t parse', () => {
  const file = path.join(s.dataDir, 'config.json');
  const kept = fs.existsSync(file) ? fs.readFileSync(file, 'utf8') : undefined;
  const disk = () => JSON.parse(fs.readFileSync(file, 'utf8'));
  try {
    fs.writeFileSync(file, JSON.stringify({ port: 9001, hostnames: ['briardr.test'] }));
    // The CLI runs with the test server's environment: BRIARDR_PORT, BRIARDR_LISTEN and BRIARDR_HERDR_SESSION.
    assert.match(s.run.cli('configure', '--allowed-networks', '10.8.0.0/24'), /Restart Briardr to apply/);
    const saved = disk();
    assert.deepEqual(saved.allowedNetworks, ['10.8.0.0/24']);
    assert.deepEqual(saved.hostnames, ['briardr.test']);
    assert.equal(saved.port, 9001, 'the file’s port, not the BRIARDR_PORT override');
    assert.deepEqual(saved.listen, ['tailscale'], 'the default, not the BRIARDR_LISTEN override');
    assert.equal(saved.herdrSession, '', 'the default, not the BRIARDR_HERDR_SESSION override');
    const backup = fs.readFileSync(`${file}.bak`, 'utf8');
    for (const broken of ['{"port":', '[]', '{"port": "x"}']) {
      fs.writeFileSync(file, broken);
      assert.throws(
        () => s.run.cli('configure', '--allowed-networks', ''),
        (e: { status: number; stderr: string }) => e.status === 78 && String(e.stderr).includes(`${file} can't be read`),
        broken,
      );
      assert.equal(fs.readFileSync(file, 'utf8'), broken, 'not saved over');
      assert.equal(fs.readFileSync(`${file}.bak`, 'utf8'), backup, 'nor its backup');
    }
  } finally {
    if (kept === undefined) fs.rmSync(file);
    else fs.writeFileSync(file, kept);
  }
});
