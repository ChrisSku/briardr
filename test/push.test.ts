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
});
