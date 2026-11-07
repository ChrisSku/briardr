// Files previews an HTML page in a frame that can't run anything, and switches to its source.
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
  fs.writeFileSync(path.join(workspace.dir, 'page.html'), '<h1>Hello page</h1><script>window.parent.document.title = "ran"</script>');
  await server.run.herdr.refresh();
  browser = await webkit.launch();
});

afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await server?.close();
});

test('Files previews an HTML page without running it and shows its source', async () => {
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
    await page.getByRole('button', { name: 'page.html', exact: true }).click();
    const frame = page.frameLocator('iframe.fv-html');
    await frame.getByRole('heading', { name: 'Hello page' }).waitFor();
    assert.notEqual(await page.title(), 'ran');
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
