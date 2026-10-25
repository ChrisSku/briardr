// The installed app on poor connections and across updates, in WebKit with the service worker on:
// - launches: a stalled network, proxy errors (502/503/504) and an unreachable host open the cached app,
//   labeled stale, and it reconnects by itself; other answers (a sign-in page) pass through; without a
//   cached shell the launch says why, without reloading in a loop.
// - updates: pages on the previous build keep working (its code stays cached) until they reload; a page
//   with work in progress waits and says so; clients converge on the new build, reloading once each; a
//   build whose shell doesn't install leaves the current one in use.
// A proxy (test-ui/proxy.ts) stands in for the network; the server serves a copy of the build under test
// (BRIARDR_WEB_DIST, default web/dist), which the update checks replace in place.
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, onTestFinished, test } from 'bun:test';
import { devices, webkit, type Browser, type BrowserContext, type Page } from 'playwright';
import { closeWorkspace, herdr, sleep, startServer, testPane, tmpDir, type TestServer } from '../test/helpers.ts';
import { startProxy, type Proxy } from './proxy.ts';

const build = path.resolve(process.env.BRIARDR_WEB_DIST || 'web/dist');
const served = tmpDir('briardr-served-');
let s: TestServer;
let proxy: Proxy;
let browser: Browser;
let ws: { pane: string; workspace: string; dir: string };

const versionOf = (dir: string) => fs.readFileSync(path.join(dir, 'index.html'), 'utf8').match(/briardr-version" content="([^"]+)"/)![1];
function serve(from: string) {
  fs.rmSync(served, { recursive: true, force: true });
  fs.cpSync(from, served, { recursive: true });
}

/**
 * Publishes the next build in place of the served one, as a deployment does (the old files are gone):
 * every bundle gets a new name, and the version changes. `broken` lists a file that doesn't exist in
 * the new shell, so installing it fails.
 */
function deploy(suffix: string, opts: { broken?: boolean } = {}): { version: string; assets: string[] } {
  const from = served;
  const v1 = versionOf(from);
  const v2 = `${v1.slice(0, 12)}${suffix}`;
  const next = tmpDir('briardr-next-');
  fs.cpSync(from, next, { recursive: true });
  const renames: [string, string][] = [];
  for (const f of fs.readdirSync(path.join(next, 'assets'))) {
    if (/\.(br|gz)$/.test(f)) continue;
    const to = f.replace(/\.([a-z0-9]+)\.(js|css|woff2)$/, `.$1${suffix}.$2`);
    if (to !== f) renames.push([f, to]);
  }
  for (const [f, to] of renames) fs.renameSync(path.join(next, 'assets', f), path.join(next, 'assets', to));
  for (const f of fs.readdirSync(next, { recursive: true }) as string[]) {
    const file = path.join(next, f);
    if (!fs.statSync(file).isFile()) continue;
    if (/\.(br|gz)$/.test(f)) {
      fs.rmSync(file); // stale compressed copies
      continue;
    }
    if (!/\.(html|js|css|json|webmanifest)$/.test(f)) continue;
    let text = fs.readFileSync(file, 'utf8');
    for (const [a, b] of renames) text = text.replaceAll(a, b);
    if (f === 'index.html' || f === 'sw.js') text = text.replaceAll(v1, v2);
    if (opts.broken && f === 'sw.js') text = text.replace('"/manifest.webmanifest"', '"/manifest.webmanifest","/assets/never-built.js"');
    fs.writeFileSync(file, text);
    // The import map changed with the names, and the server allows it by its hash.
    const map = f === 'index.html' && text.match(/<script type="importmap">([^<]*)<\/script>/)?.[1];
    if (map) fs.writeFileSync(path.join(next, 'index.html.csp'), `'sha256-${crypto.createHash('sha256').update(map).digest('base64')}'\n`);
  }
  serve(next);
  fs.rmSync(next, { recursive: true, force: true });
  return { version: v2, assets: renames.map(([, b]) => `/assets/${b}`) };
}

beforeAll(async () => {
  serve(build);
  const before = process.env.BRIARDR_WEB_DIST;
  process.env.BRIARDR_WEB_DIST = served; // read once, when the server starts
  s = await startServer({ pair: false });
  if (before === undefined) delete process.env.BRIARDR_WEB_DIST;
  else process.env.BRIARDR_WEB_DIST = before;
  proxy = await startProxy(s.base, s.dataDir);
  ws = testPane('briardr-pwa');
  herdr('pane', 'rename', ws.pane, 'pwa target');
  await s.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  proxy?.close();
  closeWorkspace(ws.workspace);
  await s?.close();
  fs.rmSync(served, { recursive: true, force: true });
});

const until = async (what: string, ok: () => Promise<boolean>, ms = 20_000) => {
  for (const t0 = Date.now(); !(await ok()); await sleep(200)) if (Date.now() - t0 > ms) assert.fail(`timed out waiting for ${what}`);
};
const shells = (page: Page) => page.evaluate(async () => (await caches.keys()).filter((k) => k.startsWith('briardr-shell-')));
const pageVersion = (page: Page) => page.evaluate(() => document.querySelector('meta[name="briardr-version"]')?.getAttribute('content'));

/** A signed-in context through the proxy, with the offline shell installed. */
async function installed(name: string): Promise<{ ctx: BrowserContext; page: Page }> {
  const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true, locale: 'en-US', serviceWorkers: 'allow' });
  onTestFinished(() => ctx.close());
  const p = s.run.auth.pair(s.run.auth.createPairingCode().code, name);
  if (!p.ok) throw new Error('pairing failed');
  await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.log('pageerror', e.message));
  await page.goto(proxy.base + '/agents');
  await page.waitForSelector('.count-strip');
  await page.waitForFunction(() => navigator.serviceWorker.controller !== null, null, { timeout: 15_000 });
  const v = versionOf(served);
  await until('the shell cache', async () => (await shells(page)).includes(`briardr-shell-${v}`));
  return { ctx, page };
}

/** The test pane's stream ID, from the model this page has cached (saved at most once a second). */
async function targetId(page: Page): Promise<string> {
  const id = await page.waitForFunction(() => JSON.parse(localStorage.getItem('briardr.cache.model') || 'null')?.streams.find((x: { name: string }) => x.name === 'pwa target')?.id, null, { timeout: 10_000 });
  return (await id.jsonValue()) as string;
}

/** Counts this page's own page loads (reloads). */
function navigations(page: Page) {
  let n = 0;
  page.on('request', (r) => r.isNavigationRequest() && r.frame() === page.mainFrame() && n++);
  return () => n;
}

describe('launch on a poor connection', () => {
  test('a stalled network opens the cached app within the timeout; it reconnects by itself and sends nothing typed meanwhile', async () => {
    const { page } = await installed('stalled launch');
    await page.goto(proxy.base + '/workspaces');
    await page.locator('.tree-streams .row-main', { hasText: 'pwa target' }).click();
    await page.waitForSelector('[data-ownership=controlling]');
    const sent: string[] = [];
    page.on('websocket', (w) => w.on('framesent', (f) => typeof f.payload === 'string' && f.payload.includes('"input"') && sent.push(f.payload)));

    await proxy.set('stall');
    const t0 = Date.now();
    await page.reload({ waitUntil: 'domcontentloaded' });
    await page.waitForSelector('.term-host', { timeout: 10_000 });
    const ms = Date.now() - t0;
    assert.ok(ms < 6000, `cached app opened after ${ms} ms`);
    assert.ok(await page.locator('.connbar').count(), 'the connection state is shown');
    // Typing while nothing is connected is dropped, never queued.
    await page.locator('.term-host').click();
    await page.keyboard.type('echo replayed');
    await sleep(500);

    await proxy.set('pass');
    await page.waitForSelector('[data-ownership=controlling]', { timeout: 40_000 });
    await page.locator('.connbar').waitFor({ state: 'detached', timeout: 40_000 });
    await sleep(1000);
    assert.deepEqual(sent, [], 'nothing typed while disconnected was sent after reconnecting');
    const cached = await page.evaluate(async () => (await Promise.all((await caches.keys()).map(async (k) => (await (await caches.open(k)).keys()).map((r) => new URL(r.url).pathname)))).flat());
    assert.ok(cached.length > 0 && !cached.some((p) => /^\/(api|ws)\//.test(p)), 'API responses and terminal traffic are never cached');
  });

  for (const status of [502, 503, 504] as const) {
    test(`a proxy answering ${status} opens the cached app, labeled stale`, async () => {
      const { page } = await installed(`proxy ${status}`);
      await page.goto(proxy.base + '/workspaces');
      await page.waitForSelector('.tree');
      await proxy.set(status);
      onTestFinished(() => proxy.set('pass'));
      await page.reload();
      await page.waitForSelector('.tree', { timeout: 10_000 });
      assert.equal(await pageVersion(page), versionOf(served));
      await page.locator('.connbar').waitFor({ timeout: 20_000 });
      assert.match((await page.locator('.connbar').textContent()) || '', /Can’t reach|Connecting|offline/);
      assert.ok(await page.locator('.tree.is-dim').count(), 'cached rows are dimmed');
      await proxy.set('pass');
      await page.locator('.connbar').waitFor({ state: 'detached', timeout: 40_000 });
    });
  }

  test('an unreachable host opens the cached app; other answers, like a sign-in page, pass through', async () => {
    const { page } = await installed('host down');
    onTestFinished(() => proxy.set('pass'));
    await proxy.set('down');
    await page.reload();
    await page.waitForSelector('.count-strip', { timeout: 10_000 });
    await page.locator('.connbar').waitFor({ timeout: 20_000 });
    // A proxy's sign-in page isn't hidden behind the cached app.
    await proxy.set(401);
    await page.reload();
    await page.getByText('proxy error 401').waitFor({ timeout: 10_000 });
  });

  test('without a cached shell, the launch says why, retries only when asked or back online, and saves the shell again', async () => {
    const { page } = await installed('no shell');
    onTestFinished(() => proxy.set('pass'));
    await page.evaluate(async () => {
      for (const k of await caches.keys()) await caches.delete(k);
    });
    await proxy.set('down');
    const count = navigations(page);
    await page.reload();
    await page.getByText('Briardr can’t open right now').waitFor({ timeout: 10_000 });
    await sleep(4000);
    assert.equal(count(), 1, 'no reload loop');
    await proxy.set('pass');
    await page.getByRole('button', { name: 'Try again' }).click();
    await page.waitForSelector('.count-strip', { timeout: 15_000 });
    // The online launch saved the shell again, so the next offline launch works.
    await until('the shell to be saved again', async () => (await shells(page)).includes(`briardr-shell-${versionOf(served)}`));
    await proxy.set('down');
    await page.reload();
    await page.waitForSelector('.count-strip', { timeout: 10_000 });
  });
});

describe('updates', () => {
  test('pages on the previous build keep working; a busy page waits and says so; both converge, reloading once each', async () => {
    onTestFinished(() => serve(build));
    const a = await installed('update a');
    const b = { page: await a.ctx.newPage() };
    await b.page.goto(proxy.base + '/agents');
    await b.page.waitForSelector('.count-strip');
    const v1 = versionOf(served);
    const oldAssets = fs.readdirSync(path.join(served, 'assets')).filter((f) => f.endsWith('.js')).map((f) => `/assets/${f}`);
    // B has typed text that isn't saved anywhere: the search in Jump.
    await b.page.getByRole('button', { name: 'Jump to a stream' }).click();
    await b.page.getByLabel('Search streams').fill('pwa');
    const [navA, navB] = [navigations(a.page), navigations(b.page)];

    const v2 = deploy('n').version;
    // Reconnecting (a network change) is when pages check the build.
    await proxy.set('down');
    await sleep(500);
    await proxy.set('pass');
    await until('A to reload into the new build', async () => (await pageVersion(a.page).catch(() => null)) === v2, 40_000);
    await a.page.waitForSelector('.count-strip');
    await b.page.locator('.update-bar').waitFor({ timeout: 20_000 });
    assert.match((await b.page.locator('.update-bar').textContent()) || '', /update is ready.*reloads once you’re done with the text you typed/s);
    assert.equal(await pageVersion(b.page), v1);
    // B still runs the previous build, whose files the host no longer has: they come from its retained shell.
    assert.deepEqual((await shells(b.page)).sort(), [`briardr-shell-${v1}`, `briardr-shell-${v2}`].sort());
    const statuses = await b.page.evaluate(async (urls) => Promise.all(urls.map(async (u) => (await fetch(u)).status)), oldAssets);
    assert.ok(statuses.every((x) => x === 200), `previous build's code still loads: ${statuses.join(',')}`);
    // Plugins in B load against B's own build's runtime.
    const loaded = await b.page.evaluate(async () => {
      await import('/sdk/v1/svelte.js' as string);
      return performance.getEntriesByType('resource').map((e) => new URL(e.name).pathname).filter((p) => /^\/(sdk\/v1|assets)\/svelte\./.test(p));
    });
    assert.ok(loaded.length && loaded.every((p) => oldAssets.includes(p)), `B's plugin runtime is its own build's (${loaded.join(', ')})`);

    // Done with the text: B reloads by itself.
    await b.page.keyboard.press('Escape');
    await until('B to reload into the new build', async () => (await pageVersion(b.page).catch(() => null)) === v2, 20_000);
    await b.page.waitForSelector('.count-strip');
    await sleep(3000);
    assert.equal(navA(), 1, 'A reloaded once');
    assert.equal(navB(), 1, 'B reloaded once');

    // One more deployment: only this build's shell and the one before it are kept.
    const v3 = deploy('m').version;
    await proxy.set('down');
    await sleep(500);
    await proxy.set('pass');
    await until('both to reload into the third build', async () => (await pageVersion(a.page).catch(() => null)) === v3 && (await pageVersion(b.page).catch(() => null)) === v3, 40_000);
    await until('the oldest shell to be deleted', async () => JSON.stringify((await shells(a.page)).sort()) === JSON.stringify([`briardr-shell-${v2}`, `briardr-shell-${v3}`].sort()));
  });

  test('a window opened during a deployment installs the new build; an older, busy page keeps its own build’s code until it reloads', async () => {
    onTestFinished(() => serve(build));
    const a = await installed('new window');
    const v1 = versionOf(served);
    const oldAssets = fs.readdirSync(path.join(served, 'assets')).filter((f) => f.endsWith('.js')).map((f) => `/assets/${f}`);
    await a.page.getByRole('button', { name: 'Jump to a stream' }).click();
    await a.page.getByLabel('Search streams').fill('pwa');
    const v2 = deploy('w').version;
    // A new window loads the new build, and the browser updates the service worker for it.
    const c = await a.ctx.newPage();
    await c.goto(proxy.base + '/agents');
    await c.waitForSelector('.count-strip');
    assert.equal(await pageVersion(c), v2);
    await a.page.locator('.update-bar').waitFor({ timeout: 20_000 });
    assert.equal(await pageVersion(a.page), v1);
    const statuses = await a.page.evaluate(async (urls) => Promise.all(urls.map(async (u) => (await fetch(u)).status)), oldAssets);
    assert.ok(statuses.every((x) => x === 200), `previous build's code still loads: ${statuses.join(',')}`);
    const loaded = await a.page.evaluate(async () => {
      await import('/sdk/v1/svelte.js' as string);
      return performance.getEntriesByType('resource').map((e) => new URL(e.name).pathname).filter((p) => /^\/(sdk\/v1|assets)\/svelte\./.test(p));
    });
    assert.ok(loaded.length && loaded.every((p) => oldAssets.includes(p)), `the plugin runtime is the page's own build's (${loaded.join(', ')})`);
    await a.page.keyboard.press('Escape');
    await until('A to reload into the new build', async () => (await pageVersion(a.page).catch(() => null)) === v2, 20_000);
  });

  test('a build whose shell fails to install leaves the current one in use, online and offline', async () => {
    onTestFinished(() => serve(build));
    const { ctx, page } = await installed('broken update');
    const v1 = versionOf(served);
    const count = navigations(page);
    deploy('b', { broken: true });
    await proxy.set('down');
    await sleep(500);
    await proxy.set('pass');
    await page.locator('.connbar').waitFor({ state: 'detached', timeout: 40_000 });
    await sleep(5000);
    assert.equal(count(), 0, 'no reload into a build that didn’t install');
    assert.equal(await pageVersion(page), v1);
    assert.deepEqual(await shells(page), [`briardr-shell-${v1}`]);
    await ctx.setOffline(true);
    await proxy.set('down');
    await page.reload();
    await page.waitForSelector('.count-strip', { timeout: 10_000 });
    assert.equal(await pageVersion(page), v1);
    await ctx.setOffline(false);
    await proxy.set('pass');
  });
});

describe('installation', () => {
  /** Pairs through the pairing screen, as a person does. */
  async function paired(name: string, opts: { standalone?: boolean; profile?: keyof typeof devices } = {}) {
    const ctx = await browser.newContext({ ...devices[opts.profile ?? 'iPhone 15 Pro'], ignoreHTTPSErrors: true, locale: 'en-US', serviceWorkers: 'block' });
    onTestFinished(() => ctx.close());
    // iOS reports a Home Screen app with navigator.standalone.
    if (opts.standalone) await ctx.addInitScript(() => Object.defineProperty(navigator, 'standalone', { value: true }));
    const page = await ctx.newPage();
    await page.goto(proxy.base + '/');
    await page.getByLabel('Pairing code').fill(s.run.auth.createPairingCode().code);
    await page.getByLabel('Name for this device').fill(name);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.waitForSelector('.count-strip');
    return { ctx, page };
  }

  test('after pairing in Safari, a suggestion to install appears on the lists, leads to the steps, and stays dismissed', async () => {
    const { page } = await paired('install suggestion');
    const card = page.getByRole('complementary', { name: 'Install Briardr' });
    await card.waitFor();
    await card.getByRole('button', { name: 'How' }).click();
    await page.waitForURL(/\/settings\/install$/);
    const section = page.locator('#set-install');
    assert.match((await section.textContent()) || '', /tap Share.*Add to Home Screen/s);
    assert.match((await section.textContent()) || '', /may not share this browser’s sign-in.*Add a device/s);
    // Notification setup points to the same steps.
    assert.ok(await page.locator('#set-notifications').getByRole('link', { name: 'Install app' }).count());
    await page.goto(proxy.base + '/workspaces');
    await page.getByRole('complementary', { name: 'Install Briardr' }).getByRole('button', { name: 'Not now' }).click();
    assert.equal(await card.count(), 0);
    await page.reload();
    await page.waitForSelector('.tree');
    await sleep(500);
    assert.equal(await card.count(), 0, 'dismissed for good');
    // Never over a terminal.
    await page.locator('.tree-streams .row-main', { hasText: 'pwa target' }).click();
    await page.waitForSelector('[data-ownership]');
    assert.equal(await card.count(), 0);
  });

  test('the installed app never suggests installing, and says it is installed', async () => {
    const { page } = await paired('installed app', { standalone: true });
    await sleep(500);
    assert.equal(await page.getByRole('complementary', { name: 'Install Briardr' }).count(), 0);
    await page.goto(proxy.base + '/settings/install');
    await page.locator('#set-install').getByText('Briardr is installed on this device').waitFor();
  });

  test('where the browser offers its install prompt, Install shows it', async () => {
    const { page } = await paired('install prompt', { profile: 'Desktop Chrome' });
    // The event as Chromium fires it when Briardr can be installed.
    await page.evaluate(() => {
      const e = new Event('beforeinstallprompt', { cancelable: true }) as any;
      e.prompt = async () => ((window as any).prompted = true);
      e.userChoice = Promise.resolve({ outcome: 'accepted' });
      dispatchEvent(e);
    });
    await page.getByRole('complementary', { name: 'Install Briardr' }).getByRole('button', { name: 'Install' }).click();
    assert.equal(await page.evaluate(() => (window as any).prompted), true);
    await page.evaluate(() => dispatchEvent(new Event('appinstalled')));
    assert.equal(await page.getByRole('complementary', { name: 'Install Briardr' }).count(), 0);
    await page.goto(proxy.base + '/settings/install');
    await page.locator('#set-install').getByText('Use the install button in the address bar').waitFor();
  });

  test('the manifest’s shortcuts open their screens, not the last one used', async () => {
    const manifest = await (await fetch(`${s.base}/manifest.webmanifest`, { tls: { rejectUnauthorized: false } } as RequestInit)).json();
    const urls = manifest.shortcuts.map((x: { url: string }) => x.url);
    assert.deepEqual(urls, ['/agents', '/workspaces']);
    assert.ok(manifest.shortcuts.every((x: { icons: { src: string }[] }) => /\/icons\/icon-192\.[0-9a-f]{10}\.png$/.test(x.icons[0].src)), 'shortcut icons use the hashed names');
    const { page } = await paired('shortcuts');
    await page.goto(proxy.base + '/settings');
    await page.waitForSelector('.settings');
    for (const [url, ready] of [['/workspaces', '.tree'], ['/agents', '.count-strip']]) {
      await page.goto(proxy.base + url);
      await page.waitForSelector(ready);
      assert.equal(new URL(page.url()).pathname, url);
    }
    // The start URL still restores the last screen.
    await page.goto(proxy.base + '/');
    await page.waitForSelector('.count-strip');
    assert.equal(new URL(page.url()).pathname, '/agents');
  });
});

describe('notifications', () => {
  /**
   * A Home Screen app whose push layer is fake (automated browsers have no push service): permission and
   * this browser's subscription live in localStorage['fake.push'], calls are logged. The host is real.
   */
  async function homeScreenApp(name: string, fake: { permission: string; sub: { endpoint: string; key?: number[] } | null; wanted?: boolean }) {
    const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true, locale: 'en-US', serviceWorkers: 'allow' });
    onTestFinished(() => ctx.close());
    await ctx.addInitScript(() => {
      Object.defineProperty(navigator, 'standalone', { value: true });
      const load = () => JSON.parse(localStorage.getItem('fake.push') || 'null');
      const save = (v: unknown) => localStorage.setItem('fake.push', JSON.stringify(v));
      const log = (c: string) => save({ ...load(), calls: [...(load().calls || []), c] });
      const subscription = (st: any) =>
        st.sub && {
          endpoint: st.sub.endpoint,
          options: { applicationServerKey: st.sub.key ? new Uint8Array(st.sub.key).buffer : null },
          toJSON: () => ({ endpoint: st.sub.endpoint, keys: { p256dh: 'BFAKE', auth: 'AFAKE' } }),
          unsubscribe: async () => (log('unsubscribe'), save({ ...load(), sub: null }), true),
        };
      // iOS has these only in Home Screen apps; emulated Safari doesn't, so they're provided whole.
      const w = window as any;
      w.Notification = function Notification() {};
      Object.defineProperty(w.Notification, 'permission', { get: () => load().permission });
      w.Notification.requestPermission = async () => (log('request'), save({ ...load(), permission: 'granted' }), 'granted');
      w.PushManager = function PushManager() {};
      const pushManager = {
        getSubscription: async () => subscription(load()),
        subscribe: async (o: PushSubscriptionOptionsInit) => {
          log('subscribe');
          save({ ...load(), sub: { endpoint: `https://push.example/${Math.random().toString(36).slice(2)}`, key: Array.from(new Uint8Array(o.applicationServerKey as ArrayBuffer)) } });
          return subscription(load());
        },
      };
      Object.defineProperty(ServiceWorkerRegistration.prototype, 'pushManager', { get: () => pushManager, configurable: true });
    });
    const p = s.run.auth.pair(s.run.auth.createPairingCode().code, name);
    await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    const page = await ctx.newPage();
    // Seed this origin's storage before the app runs.
    await page.goto(proxy.base + '/manifest.webmanifest');
    await page.evaluate(([f, w]) => {
      localStorage.setItem('fake.push', JSON.stringify({ ...f, calls: [] }));
      if (w !== undefined) localStorage.setItem('briardr.push.wanted', JSON.stringify(w));
    }, [fake, fake.wanted] as const);
    return { ctx, page };
  }
  const fake = (page: Page) => page.evaluate(() => JSON.parse(localStorage.getItem('fake.push')!));
  const setFake = (page: Page, patch: object) =>
    page.evaluate((p) => {
      localStorage.setItem('fake.push', JSON.stringify({ ...JSON.parse(localStorage.getItem('fake.push')!), ...p }));
      document.dispatchEvent(new Event('visibilitychange')); // back in the app
    }, patch);
  const hostHas = (page: Page) => page.evaluate(async () => (await fetch('/api/notifications')).json());
  const step = (page: Page) => page.locator('#set-notifications .steps li').nth(1);

  test('a host that lost this device’s subscription gets it back on launch, without asking for permission', async () => {
    const { page } = await homeScreenApp('push repair', { permission: 'granted', sub: { endpoint: 'https://push.example/kept' }, wanted: true });
    await page.goto(proxy.base + '/agents');
    await page.waitForSelector('.count-strip');
    await until('the host to have the subscription', async () => (await hostHas(page)).endpoint === 'https://push.example/kept');
    assert.deepEqual((await fake(page)).calls, [], 'no permission prompt, no new subscription');
    await page.goto(proxy.base + '/settings/notifications');
    await step(page).getByText('On.').waitFor();
  });

  test('settings never say On when the device disagrees; permission changes show on return; Repair and Turn off work', async () => {
    const { page } = await homeScreenApp('push health', { permission: 'granted', sub: { endpoint: 'https://push.example/a' }, wanted: true });
    await page.goto(proxy.base + '/settings/notifications');
    await step(page).getByText('On.').waitFor();
    // Blocked in Settings while away: shown on return, without a reload.
    await setFake(page, { permission: 'denied' });
    await step(page).getByText('Not working.').waitFor();
    assert.match((await step(page).textContent()) || '', /turned off for Briardr in Settings → Notifications/);
    assert.equal(await step(page).getByText('On.').count(), 0);
    assert.equal(await page.getByRole('button', { name: 'Send test' }).isDisabled(), true);
    // Allowed again, but the subscription expired meanwhile: Repair makes a new one.
    await setFake(page, { permission: 'granted', sub: null });
    await step(page).getByText('subscription expired or was removed').waitFor();
    await step(page).getByRole('button', { name: 'Repair' }).click();
    await step(page).getByText('On.').waitFor();
    const repaired = await fake(page);
    assert.deepEqual(repaired.calls, ['subscribe'], 'permission already granted: not asked again');
    assert.equal((await hostHas(page)).endpoint, repaired.sub.endpoint);
    // The host's key changed: the old subscription is replaced.
    await setFake(page, { sub: { ...repaired.sub, key: [1, 2, 3] } });
    await step(page).getByText('has a new notification key').waitFor();
    await step(page).getByRole('button', { name: 'Repair' }).click();
    await step(page).getByText('On.').waitFor();
    assert.deepEqual((await fake(page)).calls, ['subscribe', 'unsubscribe', 'subscribe']);
    // Turning off waits for the browser and the host.
    await step(page).getByRole('button', { name: 'Turn off' }).click();
    await step(page).getByRole('button', { name: 'Allow notifications' }).waitFor();
    assert.equal((await fake(page)).sub, null);
    assert.equal((await hostHas(page)).subscribed, false);
    // Off stays off: nothing is repaired behind the user's back.
    await page.reload();
    await step(page).getByRole('button', { name: 'Allow notifications' }).waitFor();
    assert.equal((await hostHas(page)).subscribed, false);
  });

  test('permission is only requested from the button', async () => {
    const { page } = await homeScreenApp('push ask', { permission: 'default', sub: null });
    await page.goto(proxy.base + '/settings/notifications');
    await step(page).getByRole('button', { name: 'Allow notifications' }).waitFor();
    await sleep(500);
    assert.deepEqual((await fake(page)).calls, []);
    await step(page).getByRole('button', { name: 'Allow notifications' }).click();
    await step(page).getByText('On.').waitFor();
    assert.deepEqual((await fake(page)).calls, ['request', 'subscribe']);
  });

  test('the icon badge follows live state only; an alert opens its stream in an open app or at launch', async () => {
    const { ctx, page } = await installed('badge');
    await ctx.addInitScript(() => {
      const calls: string[] = ((window as any).badgeCalls = []);
      (navigator as any).setAppBadge = async (n: number) => void calls.push(`set ${n}`);
      (navigator as any).clearAppBadge = async () => void calls.push('clear');
    });
    await page.reload();
    await page.waitForSelector('.count-strip');
    await page.waitForFunction(() => (window as any).badgeCalls.includes('clear'), null, { timeout: 10_000 });
    // While the host is unreachable, the cached count isn't presented as current.
    await proxy.set('down');
    onTestFinished(() => proxy.set('pass'));
    await page.locator('.connbar').waitFor({ timeout: 20_000 });
    const before = await page.evaluate(() => (window as any).badgeCalls.length);
    await sleep(1000);
    assert.equal(await page.evaluate(() => (window as any).badgeCalls.length), before);
    await proxy.set('pass');
    await page.locator('.connbar').waitFor({ state: 'detached', timeout: 40_000 });
    // A tap while the app is open (the service worker posts where to go).
    const id = await targetId(page);
    await page.evaluate((path) => navigator.serviceWorker.dispatchEvent(new MessageEvent('message', { data: { t: 'open', path } })), `/stream/${encodeURIComponent(id)}?from=alert`);
    await page.waitForSelector('.term-host');
    assert.equal(new URL(page.url()).pathname, `/stream/${encodeURIComponent(id)}`);
    // A tap that launches the app opens that stream, not the last screen.
    await page.goto(proxy.base + '/workspaces');
    await page.waitForSelector('.tree');
    await page.goto(proxy.base + `/stream/${encodeURIComponent(id)}?from=alert`);
    await page.waitForSelector('.term-host');
    assert.equal(new URL(page.url()).pathname, `/stream/${encodeURIComponent(id)}`);
  });
});

describe('offline readiness and local work', () => {
  const readiness = (page: Page) => page.locator('#set-diagnostics .offline-info dd').first();

  test('readiness reflects the shell: ready once installed; missing files show, and Repair restores them keeping local data', async () => {
    const { ctx, page } = await installed('readiness');
    await page.goto(proxy.base + '/settings/install');
    await page.locator('#set-install').getByText('Opens offline: ready').waitFor();
    await page.goto(proxy.base + '/settings/diagnostics');
    await readiness(page).getByText('Ready.').waitFor();
    // Local data that must survive a repair: a note draft, a preference, the pairing (a cookie).
    await page.evaluate(() => {
      localStorage.setItem('briardr.notes.draft.s1/n1', JSON.stringify({ text: 'unsaved idea', base: 0, createdAt: 1 }));
      localStorage.setItem('briardr.prefs', JSON.stringify({ fontSize: 17, bracketedPaste: true, screenReaderTerminal: false, lowBatteryInApp: true }));
    });
    // The browser cleared some of the shell's files.
    await page.evaluate(async () => {
      const name = (await caches.keys()).find((k) => k.startsWith('briardr-shell-'))!;
      const cache = await caches.open(name);
      for (const r of (await cache.keys()).filter((r) => r.url.includes('/assets/')).slice(0, 3)) await cache.delete(r);
    });
    await page.reload();
    await readiness(page).getByText('Some of the files Briardr needs offline were cleared').waitFor({ timeout: 10_000 });
    await page.getByRole('button', { name: 'Repair offline app' }).click();
    await readiness(page).getByText('Ready.').waitFor({ timeout: 30_000 });
    const kept = await page.evaluate(() => [localStorage.getItem('briardr.notes.draft.s1/n1'), JSON.parse(localStorage.getItem('briardr.prefs')!).fontSize]);
    assert.deepEqual(kept, [JSON.stringify({ text: 'unsaved idea', base: 0, createdAt: 1 }), 17]);
    // Still paired, and it opens offline again.
    await proxy.set('down');
    onTestFinished(() => proxy.set('pass'));
    await page.goto(proxy.base + '/agents');
    await page.waitForSelector('.count-strip', { timeout: 10_000 });
    await proxy.set('pass');
    await page.reload();
    await page.waitForSelector('.count-strip');
    assert.equal(await page.getByLabel('Pairing code').count(), 0);
    void ctx;
  });

  test('a shell that fails to install says so, and Repair succeeds once the host serves a good build', async () => {
    onTestFinished(() => serve(build));
    deploy('f', { broken: true });
    const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true, locale: 'en-US', serviceWorkers: 'allow' });
    onTestFinished(() => ctx.close());
    const p = s.run.auth.pair(s.run.auth.createPairingCode().code, 'install fails');
    await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    const page = await ctx.newPage();
    await page.goto(proxy.base + '/settings/diagnostics');
    await readiness(page).getByText('Saving the app for offline use didn’t finish').waitFor({ timeout: 20_000 });
    await page.goto(proxy.base + '/settings/install');
    await page.locator('#set-install').getByText('Briardr can’t open offline yet').waitFor();
    serve(build);
    await page.goto(proxy.base + '/settings/diagnostics');
    await page.getByRole('button', { name: 'Repair offline app' }).click();
    await readiness(page).getByText('Ready.').waitFor({ timeout: 30_000 });
  });

  test('cached terminal screens are kept for recent streams only (30 days, at most 100)', async () => {
    const { page } = await installed('screen retention');
    const screens = (op: 'fill' | 'list') =>
      page.evaluate(
        (op) =>
          new Promise<string[]>((resolve) => {
            const r = indexedDB.open('briardr', 1);
            r.onupgradeneeded = () => r.result.createObjectStore('screens');
            r.onsuccess = () => {
              const tx = r.result.transaction('screens', 'readwrite');
              const store = tx.objectStore('screens');
              if (op === 'fill') {
                const now = Date.now();
                for (let i = 0; i < 110; i++) store.put({ data: '', cols: 80, rows: 24, savedAt: now - i * 60_000 }, `screen:recent-${i}`);
                store.put({ data: '', cols: 80, rows: 24, savedAt: now - 31 * 24 * 3600_000 }, 'screen:old');
              }
              const keys = store.getAllKeys();
              tx.oncomplete = () => resolve((keys.result as string[]).filter((k) => /^screen:(recent|old)/.test(k)));
            };
          }),
        op,
      );
    assert.equal((await screens('fill')).length, 111);
    await page.reload();
    await page.waitForSelector('.count-strip');
    await until('old screens to be pruned', async () => (await screens('list')).length <= 100);
    const left = await screens('list');
    assert.ok(!left.includes('screen:old'), 'a month-old screen is gone');
    assert.ok(left.includes('screen:recent-0'), 'the most recent stays');
  });

  test('the installed app asks to keep its storage; a refusal changes nothing but the wording', async () => {
    const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true, locale: 'en-US', serviceWorkers: 'allow' });
    onTestFinished(() => ctx.close());
    await ctx.addInitScript(() => {
      Object.defineProperty(navigator, 'standalone', { value: true });
      const asked: string[] = ((window as any).asked = []);
      Object.defineProperty(navigator, 'storage', { value: { persisted: async () => false, persist: async () => (asked.push('persist'), false), estimate: async () => ({ usage: 5e6, quota: 1e9 }) } });
    });
    const p = s.run.auth.pair(s.run.auth.createPairingCode().code, 'persist');
    await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    const page = await ctx.newPage();
    await page.goto(proxy.base + '/settings/diagnostics');
    await page.locator('#set-diagnostics').getByText('may clear it when this device runs low on space').waitFor();
    assert.deepEqual(await page.evaluate(() => (window as any).asked), ['persist'], 'asked once, by itself, in the installed app');
    await page.getByText('5 MB in use').waitFor();
    await page.getByRole('button', { name: 'Ask to keep data' }).click();
    await page.getByText('Briardr works the same either way').waitFor();
  });

  test('a note draft this device can’t store says so, offers its text, and holds updates', async () => {
    const { ctx, page } = await installed('draft quota');
    await ctx.addInitScript(() => {
      // Storage is full for new note drafts; copying is captured.
      const set = Storage.prototype.setItem;
      Storage.prototype.setItem = function (k: string, v: string) {
        if (k.startsWith('briardr.notes.draft.')) throw new DOMException('full', 'QuotaExceededError');
        return set.call(this, k, v);
      };
      Object.defineProperty(navigator, 'clipboard', { value: { writeText: async (t: string) => void ((window as any).copied = t) } });
    });
    const id = await targetId(page);
    await page.goto(proxy.base + `/stream/${encodeURIComponent(id)}/notes`);
    await page.getByRole('button', { name: /New note/ }).click();
    await proxy.set('down'); // the host can't save it either
    onTestFinished(() => proxy.set('pass'));
    await page.locator('textarea.notes-area').fill('only here');
    await page.getByRole('alert').filter({ hasText: 'couldn’t keep a copy' }).waitFor({ timeout: 10_000 });
    assert.match((await page.locator('.notes-status').textContent()) || '', /couldn’t keep a copy/);
    await page.getByRole('button', { name: 'Copy text' }).click();
    assert.equal(await page.evaluate(() => (window as any).copied), 'only here');
    // A build the service worker activates now waits for this text, typed in one go (one input event).
    await page.evaluate(() => navigator.serviceWorker.dispatchEvent(new MessageEvent('message', { data: { t: 'shell', version: 'next' } })));
    await page.locator('.update-bar').waitFor({ timeout: 5000 });
    assert.match((await page.locator('.update-bar').textContent()) || '', /reloads once you’re done with the text you typed/);
    assert.equal(await page.locator('textarea.notes-area').inputValue(), 'only here');
    const events = await page.evaluate(() => localStorage.getItem('briardr.diag.1') || '');
    assert.match(events, /"ev":"storage_write_failed"|"ev":"note_draft_stored"/);
    assert.ok(!events.includes('only here'), 'diagnostics never hold the text');
  });
});
