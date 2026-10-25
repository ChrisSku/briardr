// Behaviour the other UI checks don't cover: a refused pairing code, a device revoked from elsewhere,
// a cancelled upload, and a new build replacing the installed app's offline shell.
// Serves a copy of the build under test (BRIARDR_WEB_DIST, default web/dist). The update check
// switches to BRIARDR_UPDATE_DIST if set (e.g. the previous frontend), else to a re-versioned copy.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, onTestFinished, test } from 'bun:test';
import { devices, webkit, type Browser, type Page } from 'playwright';
import { closeWorkspace, herdr, sleep, startServer, testPane, tmpDir, waitUntil, type TestServer } from '../test/helpers.ts';

const build = path.resolve(process.env.BRIARDR_WEB_DIST || 'web/dist');
const update = process.env.BRIARDR_UPDATE_DIST ? path.resolve(process.env.BRIARDR_UPDATE_DIST) : null;
const served = tmpDir('briardr-served-');
let s: TestServer;
let browser: Browser;
let ws: { pane: string; workspace: string; dir: string };

const versionOf = (dir: string) => fs.readFileSync(path.join(dir, 'index.html'), 'utf8').match(/briardr-version" content="([^"]+)"/)![1];
function serve(from: string) {
  fs.rmSync(served, { recursive: true, force: true });
  fs.cpSync(from, served, { recursive: true });
}

beforeAll(async () => {
  serve(build);
  const before = process.env.BRIARDR_WEB_DIST;
  process.env.BRIARDR_WEB_DIST = served; // read once, when the server starts
  s = await startServer({ pair: false });
  // Assigning undefined to process.env stores the string "undefined", which later servers would read as a path.
  if (before === undefined) delete process.env.BRIARDR_WEB_DIST;
  else process.env.BRIARDR_WEB_DIST = before;
  ws = testPane('briardr-validation');
  herdr('pane', 'rename', ws.pane, 'validation target');
  await s.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  closeWorkspace(ws.workspace);
  await s.close();
  fs.rmSync(served, { recursive: true, force: true });
});

async function signedIn(name: string, opts: { serviceWorkers?: 'allow' | 'block' } = {}) {
  const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true, locale: 'en-US', serviceWorkers: opts.serviceWorkers ?? 'block' });
  const p = s.run.auth.pair(s.run.auth.createPairingCode().code, name);
  if (!p.ok) throw new Error('pairing failed');
  await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.log('pageerror', e.message));
  await page.goto(s.base + '/agents');
  await page.waitForSelector('.count-strip');
  return { ctx, page, deviceId: p.device.id };
}

describe('pairing and sign-out', () => {
  test('a wrong pairing code is refused with the reason; the right one signs in', async () => {
    const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true, serviceWorkers: 'block' });
    onTestFinished(() => ctx.close());
    const page = await ctx.newPage();
    await page.goto(s.base + '/');
    const { code } = s.run.auth.createPairingCode();
    const wrong = code.replace(/[A-Z0-9]$/, (c) => (c === 'A' ? 'B' : 'A'));
    await page.getByLabel('Pairing code').fill(wrong.toLowerCase()); // typed lower case: shown upper case
    assert.equal(await page.getByLabel('Pairing code').inputValue(), wrong);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.getByRole('alert').filter({ hasText: 'That code isn’t right.' }).waitFor();
    assert.equal(await page.locator('.count-strip').count(), 0);
    await page.getByLabel('Pairing code').fill(code);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.waitForSelector('.count-strip');
  });

  test('a device revoked elsewhere goes back to pairing and says why', async () => {
    const a = await signedIn('to revoke');
    onTestFinished(() => a.ctx.close());
    await a.page.waitForFunction(() => !document.querySelector('.connbar'), null, { timeout: 10_000 });
    s.run.auth.revoke(a.deviceId);
    await a.page.getByLabel('Pairing code').waitFor({ timeout: 10_000 });
    await a.page.getByText('This device was signed out of Briardr.').waitFor();
  });
});

describe('uploads', () => {
  test('cancelling an upload inserts nothing and returns to review', async () => {
    const a = await signedIn('upload cancel');
    onTestFinished(() => a.ctx.close());
    await a.page.goto(s.base + '/workspaces');
    await a.page.locator('.tree-streams .row-main', { hasText: 'validation target' }).click();
    await a.page.waitForSelector('[data-ownership=controlling]');
    // Hold the upload request so it can be cancelled while it is in flight.
    let held = 0;
    await a.page.route('**/uploads?**', async (route) => {
      held++;
      await sleep(5000);
      await route.continue().catch(() => {});
    });
    const sent: string[] = [];
    a.page.on('websocket', (w) => w.on('framesent', (f) => typeof f.payload === 'string' && f.payload.includes('"input"') && sent.push(f.payload)));
    await a.page.getByRole('button', { name: 'Insert text, image or file' }).click();
    const chooser = a.page.waitForEvent('filechooser');
    await a.page.getByRole('menuitem', { name: /Select file/ }).click();
    await (await chooser).setFiles({ name: 'notes.txt', mimeType: 'text/plain', buffer: Buffer.alloc(200_000, 'x') });
    await a.page.getByRole('button', { name: 'Upload and insert path' }).click();
    await a.page.getByRole('button', { name: 'Cancel upload' }).click();
    await a.page.getByRole('button', { name: 'Upload and insert path' }).waitFor();
    assert.equal(held, 1, 'the upload was in flight when cancelled');
    assert.equal(await a.page.getByRole('alert').count(), 0, 'cancelling is not an error');
    await sleep(6000); // past the held request
    assert.deepEqual(sent, [], 'nothing was typed into the terminal');
  });
});

describe('service worker', () => {
  test('an old offline shell automatically reloads into the new build when connectivity returns', async () => {
    const a = await signedIn('sw update', { serviceWorkers: 'allow' });
    onTestFinished(async () => {
      await a.ctx.close();
      serve(build);
    });
    const cacheNames = () => a.page.evaluate(async () => (await caches.keys()).filter((k) => k.startsWith('briardr-shell-')).sort());
    const v1 = versionOf(build);
    await a.page.waitForFunction(() => navigator.serviceWorker.controller !== null, null, { timeout: 15_000 });
    await waitUntil('the first shell cache', async () => JSON.stringify(await cacheNames()) === JSON.stringify([`briardr-shell-${v1}`]));
    // Publish the next build in place of the current one.
    let v2: string;
    if (update) {
      serve(update);
      v2 = versionOf(update);
    } else {
      v2 = v1 + 'n';
      for (const f of ['index.html', 'sw.js']) fs.writeFileSync(path.join(served, f), fs.readFileSync(path.join(served, f), 'utf8').replaceAll(v1, v2));
    }
    assert.notEqual(v1, v2);
    // Launch the old cached app while offline, then reconnect without navigating manually.
    await a.ctx.setOffline(true);
    await a.page.reload();
    await a.page.waitForSelector('.count-strip');
    assert.equal(await a.page.evaluate(() => document.querySelector('meta[name="briardr-version"]')?.getAttribute('content')), v1);
    const reloaded = a.page.waitForEvent('framenavigated', { predicate: (f) => f === a.page.mainFrame(), timeout: 40_000 });
    await a.ctx.setOffline(false);
    await reloaded;
    await a.page.waitForSelector('.count-strip');
    // The previous shell stays (pages still on it load their code from it) until the next build.
    await waitUntil('the new shell', async () => JSON.stringify(await cacheNames()) === JSON.stringify([`briardr-shell-${v1}`, `briardr-shell-${v2}`].sort()));
    assert.equal(await a.page.evaluate(() => document.querySelector('meta[name="briardr-version"]')?.getAttribute('content')), v2);
    // Offline launch: the new shell comes from the cache and shows the last known state.
    await a.ctx.setOffline(true);
    await a.page.reload();
    await a.page.waitForSelector('.count-strip', { timeout: 10_000 });
    assert.equal(await a.page.evaluate(() => document.querySelector('meta[name="briardr-version"]')?.getAttribute('content')), v2);
    await a.page.locator('nav[aria-label="Main"] button', { hasText: 'Workspaces' }).click();
    await a.page.locator('.tree-streams .row-main', { hasText: 'validation target' }).waitFor();
    // (Playwright's offline mode doesn't cut WebKit's loopback WebSocket; test-ui/recovery covers the
    // offline banner by stopping the server.)
    await a.ctx.setOffline(false);
  });
});
