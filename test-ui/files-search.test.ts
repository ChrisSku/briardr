import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, test } from 'bun:test';
import { devices, webkit, type Browser } from 'playwright';
import { closeWorkspace, herdr, startServer, testPane, type TestServer } from '../test/helpers.ts';
import { pageErrors } from './harness.ts';

let server: TestServer;
let browser: Browser;
let workspace: ReturnType<typeof testPane>;

beforeAll(async () => {
  server = await startServer({ pair: false });
  workspace = testPane('briardr-file-search');
  herdr('pane', 'rename', workspace.pane, 'filename search target');
  for (const [name, text] of Object.entries({
    'src/needle.ts': 'export const source = 1;',
    'lib/deep/needle.ts': 'export const nested = 2;',
    'needle.md': '# Root match',
    'other.txt': 'needle appears only in the contents',
  })) {
    const file = path.join(workspace.dir, name);
    fs.mkdirSync(path.dirname(file), { recursive: true });
    fs.writeFileSync(file, text);
  }
  await server.run.herdr.refresh();
  browser = await webkit.launch();
});

afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await server?.close();
});

for (const profile of ['iphone', 'mac'] as const) {
  test(`filename search finds unopened nested files and restores the tree (${profile})`, async () => {
    const ctx = await browser.newContext({
      ...(profile === 'iphone' ? devices['iPhone 15 Pro'] : { viewport: { width: 1280, height: 800 } }),
      ignoreHTTPSErrors: true,
      serviceWorkers: 'block',
    });
    try {
      const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, `file-search-${profile}`);
      assert.ok(paired.ok);
      await ctx.addCookies([{ name: '__Host-briardr', value: paired.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
      const page = await ctx.newPage();
      const errors = pageErrors(page);
      await page.goto(server.base + '/workspaces');
      await page.locator('.tree-streams .row-main', { hasText: 'filename search target' }).click();
      await page.getByRole('tab', { name: 'Files', exact: true }).click();
      const src = page.getByRole('button', { name: 'src, folder', exact: true });
      await src.click();
      await page.getByRole('button', { name: 'needle.ts', exact: true }).waitFor();
      const input = page.getByRole('searchbox', { name: 'Search files by filename' });
      await input.fill('NEEDLE');
      await page.getByRole('status').filter({ hasText: '3 files found' }).waitFor();
      assert.equal(await page.locator('.ft-result').count(), 3);
      assert.equal(await page.locator('.ft-result .ft-path').filter({ hasText: 'lib/deep' }).count(), 1);
      await page.getByRole('button', { name: 'lib/deep/needle.ts', exact: true }).click();
      await page.locator('.code').filter({ hasText: 'nested = 2' }).waitFor();
      if (profile === 'iphone') await page.getByRole('button', { name: 'Back to folder' }).click();
      await page.getByRole('button', { name: 'Clear filename search' }).click();
      assert.equal(await input.inputValue(), '');
      assert.equal(await src.getAttribute('aria-expanded'), 'true');
      await page.getByRole('button', { name: 'needle.ts', exact: true }).waitFor();
      await input.fill('no-such-filename');
      await page.getByText('No matching files', { exact: true }).waitFor();
      await input.press('Escape');
      assert.equal(await input.inputValue(), '');
      await input.fill('needle.md');
      await page.getByRole('status').filter({ hasText: '1 file found' }).waitFor();
      await input.press('ArrowDown');
      await page.keyboard.press('Enter');
      await page.locator('.code').filter({ hasText: 'Root match' }).waitFor();
      assert.deepEqual(errors, []);
      fs.mkdirSync('test-results/file-search', { recursive: true });
      if (profile === 'iphone') await page.getByRole('button', { name: 'Back to folder' }).click();
      await page.screenshot({ path: `test-results/file-search/${profile}.png` });
    } finally {
      await ctx.close();
    }
  });
}
