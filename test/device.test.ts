// What kind of device the client runs on, from what its browser says: the steps to install Briardr, whether
// alerts need the Home Screen app, and the name a new device is paired under all follow from it. An iPad's
// Safari says it's a Mac; its touch screen tells it apart.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { iosDevice, isIOS, platform, type Platform } from '../web/src/device.ts';

const SAFARI_MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Safari/605.1.15';
const CHROME_MAC = 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36';

/** What browsers on each kind of device report: user agent, navigator.platform and touch points. */
const DEVICES: [string, string, string, number, Platform][] = [
  ['Safari on an iPhone', 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1', 'iPhone', 5, 'iphone'],
  ['Chrome on an iPhone', 'Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/131.0.6778.73 Mobile/15E148 Safari/604.1', 'iPhone', 5, 'iphone'],
  ['Safari on an iPad, as a Mac (the default)', SAFARI_MAC, 'MacIntel', 5, 'ipad'],
  ['Safari on an iPad, asking for mobile sites', 'Mozilla/5.0 (iPad; CPU OS 18_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 Safari/604.1', 'iPad', 5, 'ipad'],
  ['Safari on a Mac', SAFARI_MAC, 'MacIntel', 0, 'mac-safari'],
  ['Chrome on a Mac', CHROME_MAC, 'MacIntel', 0, 'desktop'],
  ['Firefox on a Mac', 'Mozilla/5.0 (Macintosh; Intel Mac OS X 10.15; rv:133.0) Gecko/20100101 Firefox/133.0', 'MacIntel', 0, 'firefox'],
  ['Chrome on Android', 'Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Mobile Safari/537.36', 'Linux armv81', 5, 'android'],
  ['Edge on a Windows touch screen', 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36 Edg/131.0.0.0', 'Win32', 10, 'desktop'],
];

test('each browser is told apart, an iPad by its touch screen when it says it is a Mac', () => {
  for (const [name, userAgent, nav, maxTouchPoints, expected] of DEVICES) assert.equal(platform({ userAgent, platform: nav, maxTouchPoints }), expected, name);
});

test('iPhones and iPads, whatever their browser, are iOS and named for what they are', () => {
  for (const [name, userAgent, nav, maxTouchPoints] of DEVICES) {
    const p = platform({ userAgent, platform: nav, maxTouchPoints });
    const expected = p === 'iphone' ? 'iPhone' : p === 'ipad' ? 'iPad' : null;
    assert.equal(iosDevice(p), expected, name);
    assert.equal(isIOS(p), expected !== null, name);
  }
});
