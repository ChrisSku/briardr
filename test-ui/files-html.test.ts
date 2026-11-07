// Files shows an HTML page as a page: its stylesheet and script load, its links work, and it can't act as
// Briardr. The source is a toggle away.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, test } from 'bun:test';
import { webkit, type Browser } from 'playwright';
import { closeWorkspace, herdr, startServer, testPane, type TestServer } from '../test/helpers.ts';
import { pageErrors } from './harness.ts';

let server: TestServer;
let browser: Browser;
let workspace: ReturnType<typeof testPane>;

beforeAll(async () => {
  server = await startServer({ pair: false });
  workspace = testPane('briardr-files-html');
  herdr('pane', 'rename', workspace.pane, 'html preview target');
  // Like a lesson: a stylesheet and a script by relative path, another page, a link out.
  fs.mkdirSync(path.join(workspace.dir, 'lessons'));
  fs.mkdirSync(path.join(workspace.dir, 'assets'));
  fs.writeFileSync(path.join(workspace.dir, 'assets/style.css'), 'h1 { color: rgb(1, 2, 3); }');
  fs.writeFileSync(path.join(workspace.dir, 'assets/app.js'), 'document.getElementById("out").textContent = "script ran";');
  fs.writeFileSync(path.join(workspace.dir, 'lessons/two.html'), '<h1>Second page</h1>');
  fs.writeFileSync(
    path.join(workspace.dir, 'lessons/page.html'),
    '<!doctype html><head><link rel="stylesheet" href="../assets/style.css"></head><h1>Hello page</h1><p id="out"></p><a href="two.html">Next</a> <a href="https://example.invalid/x">Out</a><script src="../assets/app.js"></script><script>try { window.parent.document.title = "ran" } catch {}</script>',
  );
  await server.run.herdr.refresh();
  browser = await webkit.launch();
});

afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await server?.close();
});

test('Files shows an HTML page with its stylesheet, script and links, and its source', async () => {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, ignoreHTTPSErrors: true, serviceWorkers: 'block' });
  try {
    const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, 'files-html');
    assert.ok(paired.ok);
    await ctx.addCookies([{ name: '__Host-briardr', value: paired.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    const page = await ctx.newPage();
    const errors = pageErrors(page);
    await page.goto(server.base + '/workspaces');
    await page.locator('.tree-streams .row-main', { hasText: 'html preview target' }).click();
    await page.getByRole('tab', { name: 'Files', exact: true }).click();
    await page.getByRole('button', { name: 'lessons, folder', exact: true }).click();
    await page.getByRole('button', { name: 'page.html', exact: true }).click();
    const frame = page.frameLocator('iframe.fv-html');
    await frame.getByRole('heading', { name: 'Hello page' }).waitFor();
    // The page's own script ran, its stylesheet applied, and it couldn't reach Briardr.
    await frame.getByText('script ran').waitFor();
    assert.equal(await frame.getByRole('heading').evaluate((h) => getComputedStyle(h).color), 'rgb(1, 2, 3)');
    assert.notEqual(await page.title(), 'ran');
    await frame.getByRole('link', { name: 'Next' }).click();
    await frame.getByRole('heading', { name: 'Second page' }).waitFor();
    await page.getByRole('button', { name: 'Show source', exact: true }).click();
    await page.getByRole('button', { name: 'Preview page', exact: true }).click();
    await frame.getByRole('heading', { name: 'Hello page' }).waitFor();
    await page.getByRole('button', { name: 'Show source', exact: true }).click();
    await page.locator('.code', { hasText: '<h1>Hello page</h1>' }).waitFor();
    assert.equal(await page.locator('iframe.fv-html').count(), 0);
    await page.getByRole('button', { name: 'Preview page', exact: true }).click();
    await frame.getByRole('heading', { name: 'Hello page' }).waitFor();
    assert.deepEqual(errors, []);
  } finally {
    await ctx.close();
  }
});
