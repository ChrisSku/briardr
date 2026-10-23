// Connection recovery in WebKit: a silent dead connection is detected by the health check, cached
// state stays visible and labeled stale, and the app reconnects by itself when the Mac returns.
import assert from 'node:assert/strict';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import { afterAll, beforeAll, describe, onTestFinished, test } from 'bun:test';
import { devices, webkit, type Browser } from 'playwright';
import { closeWorkspace, freePort, herdr, SESSION, sleep, testPane, tmpDir } from '../test/helpers.ts';
import { serverArgv } from './harness.ts';

const dataDir = tmpDir('briardr-recovery-');
let base = ''; // the same port every time the server starts, set in beforeAll
let env: NodeJS.ProcessEnv = {};
let server: ChildProcess | null = null;
let browser: Browser;
let ws: { pane: string; workspace: string };

async function startServer() {
  server = spawn(...serverArgv('serve'), { env, stdio: 'ignore' });
  for (let i = 0; i < 50; i++) {
    try {
      const r = await fetch(`${base}/api/session`, { headers: { origin: base } } as any).catch(() => null);
      if (r) return;
    } catch {}
    await sleep(200);
  }
}

beforeAll(async () => {
  process.env.NODE_TLS_REJECT_UNAUTHORIZED = '0';
  const port = await freePort();
  base = `https://localhost:${port}`;
  env = { ...process.env, BRIARDR_DATA_DIR: dataDir, BRIARDR_HERDR_SESSION: SESSION, BRIARDR_PORT: String(port), BRIARDR_PREVIEW_PORT: String(port + 1), BRIARDR_LISTEN: '', BRIARDR_NO_DICTATION: '1' };
  ws = testPane('briardr-recovery');
  herdr('pane', 'rename', ws.pane, 'recovery target');
  await startServer();
  browser = await webkit.launch();
});
afterAll(async () => {
  server?.kill('SIGCONT');
  server?.kill();
  await browser?.close();
  closeWorkspace(ws.workspace);
});

describe('recovery', () => {
  test('a silent dead connection never looks live; cached state stays visible and stale; reconnects by itself', async () => {
    const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true });
    const page = await ctx.newPage();
    await page.goto(base + '/');
    const out = execFileSync(...serverArgv('pair'), { env, encoding: 'utf8' });
    await page.getByLabel('Pairing code').fill(out.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/)![0]);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.waitForSelector('.count-strip');
    await page.goto(base + '/workspaces');
    await page.locator('.tree-streams .row-main', { hasText: 'recovery target' }).click();
    await page.waitForSelector('[data-ownership=controlling]');
    const headTop = () => page.locator('.stream-head').evaluate((e) => e.getBoundingClientRect().top);
    const hostTop = () => page.locator('.term-host').evaluate((e) => e.getBoundingClientRect().top);
    const [head0, host0] = [await headTop(), await hostTop()];
    let navigations = 0;
    page.on('request', (r) => { if (r.isNavigationRequest() && r.frame() === page.mainFrame()) navigations++; });

    // 1. Freeze the server: sockets stay open but nothing answers (like a dead network path).
    server!.kill('SIGSTOP');
    const t0 = Date.now();
    await page.waitForSelector('[data-ownership=reconnecting]', { timeout: 25_000 });
    const detectedMs = Date.now() - t0;
    assert.ok(detectedMs < 20_000, `dead terminal connection detected in ${detectedMs} ms`);
    await page.getByText('Last seen screen · not live').waitFor();
    assert.ok(await page.locator('.term-host.is-stale').count(), 'terminal dimmed as stale');
    // The reconnecting banner floats over the header; nothing below it moves.
    await page.locator('.top-overlay .own-banner', { hasText: 'Showing the last screen' }).waitFor();
    assert.deepEqual([await headTop(), await hostTop()], [head0, host0], 'banner overlays instead of pushing content down');
    assert.equal(navigations, 0, 'a dead network alone does not reload the app');

    // 2. Kill it outright: lists stay visible from cache, dimmed and labeled stale.
    server!.kill('SIGCONT');
    server!.kill('SIGKILL');
    await page.goto(base + '/workspaces').catch(() => {}); // offline: the service worker serves the shell
    await page.waitForSelector('.tree', { timeout: 10_000 });
    await page.locator('.connbar').waitFor({ timeout: 20_000 });
    assert.match(await page.locator('.connbar').textContent() || '', /Can’t reach the Mac|Connecting|Retrying|offline/i);
    assert.equal(await page.locator('.screen-head').first().evaluate((e) => e.getBoundingClientRect().top), 0, 'connection banner overlays the header');
    assert.ok(await page.locator('.tree.is-dim').count(), 'cached tree rows are dimmed');
    assert.ok(await page.locator('.tree-streams .row-main', { hasText: 'recovery target' }).count(), 'cached rows still visible');

    // 3. Bring the Mac back: reconnect triggers one reload, preserving this tab's route.
    const reloaded = page.waitForEvent('framenavigated', { predicate: (f) => f === page.mainFrame(), timeout: 40_000 });
    await startServer();
    await reloaded;
    await page.locator('.connbar').waitFor({ state: 'detached', timeout: 40_000 });
    await page.waitForSelector('.tree');
    assert.equal(new URL(page.url()).pathname, '/workspaces');
    assert.equal(await page.locator('.tree.is-dim').count(), 0);
    await ctx.close();
  });

  for (const serviceWorkers of ['allow', 'block'] as const) {
    test(`an open app reloads once after a server restart (service workers ${serviceWorkers})`, async () => {
      const ctx = await browser.newContext({ ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true, serviceWorkers });
      onTestFinished(() => ctx.close());
      const page = await ctx.newPage();
      await page.goto(base + '/');
      const out = execFileSync(...serverArgv('pair'), { env, encoding: 'utf8' });
      await page.getByLabel('Pairing code').fill(out.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/)![0]);
      await page.getByRole('button', { name: 'Sign in' }).click();
      await page.waitForSelector('.count-strip');
      await page.waitForFunction(() => sessionStorage.getItem('briardr.serverInstance') !== null);
      const previous = await page.evaluate(() => sessionStorage.getItem('briardr.serverInstance'));
      const route = page.url();
      let navigations = 0;
      page.on('request', (r) => { if (r.isNavigationRequest() && r.frame() === page.mainFrame()) navigations++; });
      const exited = new Promise<void>((resolve) => server!.once('exit', () => resolve()));
      server!.kill('SIGKILL');
      await exited;
      const reloaded = page.waitForEvent('framenavigated', { predicate: (f) => f === page.mainFrame(), timeout: 40_000 });
      await startServer();
      await reloaded;
      await page.waitForSelector('.count-strip');
      await page.waitForFunction((old) => {
        const current = sessionStorage.getItem('briardr.serverInstance');
        return current && current !== old && !document.querySelector('.connbar');
      }, previous);
      await sleep(1500);
      assert.equal(page.url(), route, 'the current route survives the reload');
      assert.equal(navigations, 1, 'no reload loop');
    });
  }
});
