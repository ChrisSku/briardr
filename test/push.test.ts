// Push delivery is reported honestly: "accepted" only on a 2xx from the push service, "delivered"
// only after the device's service worker confirms. Payloads carry no terminal text.
import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import crypto from 'node:crypto';
import { request, startServer, type TestServer } from './helpers.ts';

const keys = () => {
  // A real P-256 subscription key pair, as a browser would create.
  const ecdh = crypto.createECDH('prime256v1');
  ecdh.generateKeys();
  return { p256dh: ecdh.getPublicKey('base64url'), auth: Buffer.alloc(16, 7).toString('base64url') };
};

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
    const sub = await request(s, 'POST', '/api/notifications/subscribe', { subscription: { endpoint: 'https://web.push.apple.com/zz', keys: keys() } });
    assert.equal(sub.status, 200);
    const t = await request(s, 'POST', '/api/notifications/test');
    assert.equal(t.json.delivery.accepted, true);
    assert.equal(t.json.delivery.deliveredAt, null);
    assert.equal(sent, 1);
    await request(s, 'POST', '/api/notifications/ack', { id: t.json.delivery.id });
    const q = await request(s, 'GET', `/api/notifications/deliveries/${t.json.delivery.id}`);
    assert.ok(q.json.delivery.deliveredAt);
  });

  test('a device chooses its own alerts and quiet hours; the host’s settings stay as they are', async () => {
    const device = s.run.auth.pair(s.run.auth.createPairingCode().code, 'other device');
    assert.ok(device.ok);
    const other = { cookie: `__Host-briardr=${device.cookie}` };
    const choose = (body: unknown, o = {}) => request(s, 'PUT', '/api/notifications/device', body, o);
    const prefs = async (o = {}) => (await request(s, 'GET', '/api/notifications', undefined, o)).json.prefs;
    assert.equal((await choose({ finished: false }, other)).status, 409, 'a device without alerts has nothing to choose');
    assert.equal(await prefs(other), null);
    // The first test subscribed this suite's own device.
    assert.deepEqual(await prefs(), {});
    const before = (await request(s, 'GET', '/api/settings')).json.notifications;

    assert.deepEqual((await choose({ finished: false, ready: true })).json.prefs, { finished: false, ready: true });
    assert.deepEqual((await choose({ quiet: { from: 1320, to: 420, utcOffset: 120 } })).json.prefs, { finished: false, ready: true, quiet: { from: 1320, to: 420, utcOffset: 120 } });
    // null goes back to the host's setting; fields left out stay.
    assert.deepEqual((await choose({ finished: null })).json.prefs, { ready: true, quiet: { from: 1320, to: 420, utcOffset: 120 } });
    for (const bad of [{ finished: 'yes' }, { quiet: { from: 1440, to: 0, utcOffset: 0 } }, { quiet: { from: 0, to: 60 } }, { quiet: { from: 0, to: 60, utcOffset: 900 } }]) assert.equal((await choose(bad)).status, 400, JSON.stringify(bad));
    assert.deepEqual(await prefs(), { ready: true, quiet: { from: 1320, to: 420, utcOffset: 120 } }, 'a refused change changes nothing');
    assert.deepEqual((await request(s, 'GET', '/api/settings')).json.notifications, before, 'the host’s own settings are untouched');
    assert.deepEqual((await choose({ ready: null, quiet: null })).json.prefs, {});
  });
});
