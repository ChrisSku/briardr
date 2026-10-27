// Updates: Briardr asks a releases source (here a fake GitHub) for the latest release, and only a release
// installation installs one. The server under test runs from the checkout, so it says what to run instead.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { request, SERVER_BIN, startServer, tmpDir, type TestServer } from './helpers.ts';

let s: TestServer;
let releases: ReturnType<typeof Bun.serve>;
let latest: { status: number; tag: string } = { status: 200, tag: 'v9.9.9' };
const current = JSON.parse(fs.readFileSync(path.resolve(import.meta.dir, '..', 'package.json'), 'utf8')).version;

beforeAll(async () => {
  releases = Bun.serve({
    port: 0,
    fetch: (req) =>
      new URL(req.url).pathname === '/latest' ? Response.json({ tag_name: latest.tag }, { status: latest.status }) : new Response('not found', { status: 404 }),
  });
  s = await startServer({ env: { BRIARDR_RELEASES_URL: `http://127.0.0.1:${releases.port}` } });
});
afterAll(async () => {
  await s.close();
  releases.stop(true);
});

describe('updates', () => {
  test('nothing is known until a check, and checking is on by default', async () => {
    const r = await request(s, 'GET', '/api/update');
    assert.equal(r.status, 200);
    assert.equal(r.json.current, current);
    assert.equal(r.json.latest, null);
    assert.equal(r.json.available, false);
    assert.equal(r.json.check, true);
  });

  test('a check finds the newer release; a checkout says how to update instead of installing', async () => {
    const r = await request(s, 'POST', '/api/update/check', {});
    assert.equal(r.status, 200);
    assert.equal(r.json.latest, '9.9.9');
    assert.equal(r.json.available, true);
    assert.equal(r.json.error, null);
    assert.equal(r.json.canInstall, false);
    assert.match(r.json.how, /runs from a checkout/);
    assert.match(r.json.command, /git checkout v9\.9\.9/);
    assert.equal(r.json.notes, 'https://github.com/ChrisSku/briardr/releases/tag/v9.9.9');

    const refused = await request(s, 'POST', '/api/update/install', { version: '9.9.9' });
    assert.equal(refused.status, 409);
    assert.equal(refused.json.error, 'update_failed');
    assert.match(refused.json.message, /checkout/);
    // Only the release the check found, never a version a request names.
    const other = await request(s, 'POST', '/api/update/install', { version: '10.0.0' });
    assert.equal(other.status, 409);
    assert.match(other.json.message, /isn’t the newer release/);
  });

  test('a failed check keeps what the last one found and says what went wrong', async () => {
    latest = { status: 500, tag: '' };
    const r = await request(s, 'POST', '/api/update/check', {});
    latest = { status: 200, tag: 'v9.9.9' };
    assert.equal(r.json.latest, '9.9.9');
    assert.match(r.json.error, /latest release/);
  });

  test('the daily check can be turned off in Settings', async () => {
    assert.equal((await request(s, 'PUT', '/api/settings', { updates: { check: false } })).status, 200);
    assert.equal((await request(s, 'GET', '/api/update')).json.check, false);
    const saved = JSON.parse(fs.readFileSync(path.join(s.dataDir, 'config.json'), 'utf8'));
    assert.deepEqual(saved.updates, { check: false });
  });

  test('`briardr update --check` says whether there is a newer release', async () => {
    const env = { ...process.env, BRIARDR_DATA_DIR: tmpDir('briardr-data-'), BRIARDR_RELEASES_URL: `http://127.0.0.1:${releases.port}` };
    // Not execFileSync: this process serves the fake releases.
    const check = () => new Response(Bun.spawn([SERVER_BIN, 'update', '--check'], { env, stdout: 'pipe' }).stdout).text();
    assert.match(await check(), new RegExp(`Briardr 9\\.9\\.9 is available; this is ${current.replace(/\./g, '\\.')}`));
    latest = { status: 200, tag: `v${current}` };
    try {
      assert.match(await check(), /is the latest release/);
    } finally {
      latest = { status: 200, tag: 'v9.9.9' };
    }
  });
});
