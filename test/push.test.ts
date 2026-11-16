// Push delivery is reported honestly: "accepted" only on a 2xx from the push service, "delivered"
// only after the device's service worker confirms. Payloads carry no terminal text.
import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import fs from 'node:fs';
import path from 'node:path';
import { pushKeys, request, startServer, waitUntil, type TestServer } from './helpers.ts';

describe('notification endpoints', () => {
  let s: TestServer;
  beforeAll(async () => {
    s = await startServer();
  });
  afterAll(() => s.close());

  test('subscribe, test send and service-worker confirmation', async () => {
    let sent = 0;
    s.run.push.sendImpl = async () => (sent++, { statusCode: 201 });
    assert.equal((await request(s, 'POST', '/api/notifications/test')).status, 409, 'must subscribe first');
    const sub = await request(s, 'POST', '/api/notifications/subscribe', { subscription: { endpoint: 'https://web.push.apple.com/zz', keys: pushKeys() } });
    assert.equal(sub.status, 200);
    const t = await request(s, 'POST', '/api/notifications/test');
    assert.equal(t.json.delivery.accepted, true);
    assert.equal(t.json.delivery.deliveredAt, null);
    assert.equal(sent, 1);
    await request(s, 'POST', '/api/notifications/ack', { id: t.json.delivery.id });
    const q = await request(s, 'GET', `/api/notifications/deliveries/${t.json.delivery.id}`);
    assert.ok(q.json.delivery.deliveredAt);
  });

  test('a device chooses its own alerts and quiet hours, which the host keeps until it is revoked', async () => {
    const device = s.run.auth.pair(s.run.auth.createPairingCode().code, 'other device');
    assert.ok(device.ok);
    const other = { cookie: `__Host-briardr=${device.cookie}` };
    const choose = (body: unknown, o = {}) => request(s, 'PUT', '/api/notifications/device', body, o);
    const prefs = async (o = {}) => (await request(s, 'GET', '/api/notifications', undefined, o)).json.prefs;
    const kept = () => JSON.parse(fs.readFileSync(path.join(s.dataDir, 'state', 'device-alerts.json'), 'utf8'));
    assert.deepEqual(await prefs(), {}, 'nothing chosen: the host’s settings');
    const before = (await request(s, 'GET', '/api/settings')).json.notifications;

    const quiet = { from: 1320, to: 420, utcOffset: 60, next: { at: Date.UTC(2026, 2, 29, 1), utcOffset: 120 } };
    assert.deepEqual((await choose({ finished: false, ready: true })).json.prefs, { finished: false, ready: true });
    assert.deepEqual((await choose({ quiet, includeNames: false })).json.prefs, { finished: false, ready: true, includeNames: false, quiet });
    // null goes back to the host's setting; fields left out stay.
    assert.deepEqual((await choose({ finished: null })).json.prefs, { ready: true, includeNames: false, quiet });
    for (const bad of [{ finished: 'yes' }, { quiet: { from: 1440, to: 0, utcOffset: 0 } }, { quiet: { from: 0, to: 60 } }, { quiet: { from: 0, to: 60, utcOffset: 900 } }, { quiet: { ...quiet, next: { at: 0 } } }]) assert.equal((await choose(bad)).status, 400, JSON.stringify(bad));
    assert.deepEqual(await prefs(), { ready: true, includeNames: false, quiet }, 'a refused change changes nothing');
    assert.deepEqual((await request(s, 'GET', '/api/settings')).json.notifications, before, 'the host’s own settings are untouched');

    // Each device's own, and kept whether or not its alerts are on.
    assert.equal((await choose({ needsYou: false }, other)).status, 200);
    assert.deepEqual(await prefs(other), { needsYou: false });
    assert.deepEqual(Object.keys(kept()).sort(), [device.device.id, s.deviceId].sort());
    assert.equal((await request(s, 'DELETE', '/api/notifications/subscribe')).status, 200);
    assert.deepEqual(await prefs(), { ready: true, includeNames: false, quiet });
    // Revoked: its choices go.
    assert.ok(s.run.auth.revoke(device.device.id));
    await waitUntil('the host to forget the revoked device', () => !(device.device.id in kept()));
    assert.deepEqual(Object.keys(kept()), [s.deviceId]);
    assert.deepEqual((await choose({ ready: null, includeNames: null, quiet: null })).json.prefs, {});
    assert.deepEqual(kept(), {}, 'nothing chosen, nothing kept');
  });
});
