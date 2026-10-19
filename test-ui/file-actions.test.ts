import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, test } from 'bun:test';
import { devices, webkit, type Browser, type Locator, type Page } from 'playwright';
import { closeWorkspace, herdr, request, sleep, startServer, testPane, type TestServer } from '../test/helpers.ts';

let server: TestServer;
let browser: Browser;
let workspace: ReturnType<typeof testPane>;
const contents = 'const one = 1;\n// ``` a fence inside the file\nconst two = 2;\n';

beforeAll(async () => {
  server = await startServer({ pair: false });
  workspace = testPane('briardr-file-actions');
  herdr('pane', 'rename', workspace.pane, 'file actions target');
  for (const profile of ['iphone', 'mac']) {
    fs.mkdirSync(path.join(workspace.dir, `src-${profile}`), { recursive: true });
    fs.writeFileSync(path.join(workspace.dir, `src-${profile}/full.ts`), contents);
    fs.writeFileSync(path.join(workspace.dir, `src-${profile}/binary.dat`), Buffer.from([0, 1, 2]));
    fs.mkdirSync(path.join(workspace.dir, `trash-${profile}/nested`), { recursive: true });
    fs.writeFileSync(path.join(workspace.dir, `trash-${profile}/nested/readme.txt`), 'delete this folder');
  }
  await server.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await server?.close();
});

async function actionsMenu(page: Page, row: Locator, touch: boolean) {
  const more = row.locator('..').getByRole('button', { name: /^More actions for / });
  if (touch) await more.tap();
  else await more.click();
  await page.getByRole('menu').waitFor();
}

for (const profile of ['iphone', 'mac'] as const) {
  test(`file menus send @file references and confirm deletion (${profile})`, async () => {
    const ctx = await browser.newContext({
      ...(profile === 'iphone' ? devices['iPhone 15 Pro'] : { viewport: { width: 1280, height: 800 } }),
      ignoreHTTPSErrors: true, serviceWorkers: 'block',
    });
    try {
      const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, `actions-${profile}`);
      assert.ok(paired.ok);
      await ctx.addCookies([{ name: '__Host-briardr', value: paired.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
      if (profile === 'iphone') await ctx.addInitScript(({ sid, contents }) => {
        localStorage.setItem('briardr.files.prompt', JSON.stringify({ [sid]: {
          question: '', snippets: [{ path: 'src-iphone/full.ts', from: 1, to: 4, text: contents, whole: true }],
        } }));
      }, { sid: workspace.pane, contents });
      const page = await ctx.newPage();
      const inputs: any[] = [];
      const errors: string[] = [];
      const fileReads: string[] = [];
      page.on('request', (req) => { if (req.url().includes('/files/file?')) fileReads.push(req.url()); });
      page.on('pageerror', (e) => errors.push(e.message));
      page.on('websocket', (ws) => ws.on('framesent', ({ payload }) => {
        if (typeof payload !== 'string') return;
        try { const message = JSON.parse(payload); if (message.t === 'input') inputs.push(message); } catch {}
      }));
      await page.goto(server.base + '/workspaces');
      await page.locator('.tree-streams .row-main', { hasText: 'file actions target' }).click();
      await page.waitForSelector('[data-ownership=controlling]');
      await page.getByRole('tab', { name: 'Files', exact: true }).click();
      if (profile === 'iphone') {
        const saved = await page.evaluate((sid) => JSON.parse(localStorage.getItem('briardr.files.prompt')!)[sid], workspace.pane);
        assert.deepEqual(saved.snippets, [], 'legacy full-file contents are removed from the saved draft');
        assert.deepEqual(saved.references, [{ path: 'src-iphone/full.ts', dir: false }]);
      }
      const folder = page.getByRole('button', { name: `src-${profile}, folder`, exact: true });
      await actionsMenu(page, folder, profile === 'iphone');
      assert.equal(await folder.getAttribute('aria-expanded'), 'false');
      await page.getByRole('menuitem', { name: 'Send to terminal…' }).click();
      await page.getByRole('dialog', { name: 'Ask about code' }).waitFor();
      await page.getByLabel('Question or instructions').fill('Review these');
      await page.getByRole('button', { name: 'Close', exact: true }).click();
      await folder.click();
      const file = page.getByRole('button', { name: 'full.ts', exact: true });
      await actionsMenu(page, file, profile === 'iphone');
      await page.getByRole('menuitem', { name: 'Send to terminal…' }).click();
      await page.getByText(`@src-${profile}/full.ts`, { exact: true }).waitFor();
      await page.getByText('What will be pasted', { exact: true }).click();
      const expected = `Review these\n\n@src-${profile}/\n\n@src-${profile}/full.ts`;
      assert.equal(await page.locator('.prompt-raw pre').textContent(), expected);
      const before = inputs.length;
      await page.getByRole('button', { name: 'Send to terminal', exact: true }).click();
      await page.waitForSelector('[role=tab][aria-selected=true]:has-text("Terminal")');
      await sleep(150);
      assert.equal(inputs.length, before + 1, 'one paste and no Enter');
      assert.equal(inputs.at(-1).paste.text, expected);
      assert.deepEqual(fileReads, [], 'attaching a file must not fetch its contents');
      assert.equal(await page.evaluate(() => localStorage.getItem('briardr.files.prompt')), '{}');

      await page.getByRole('tab', { name: 'Files', exact: true }).click();
      // All file types use @ references.
      const binary = page.getByRole('button', { name: 'binary.dat', exact: true });
      const binaryActions = binary.locator('..').getByRole('button', { name: /^More actions for / });
      await binaryActions.focus();
      await binaryActions.press('Enter');
      await page.getByRole('menuitem', { name: 'Send to terminal…' }).click();
      await page.getByText('What will be pasted', { exact: true }).click();
      assert.equal(await page.locator('.prompt-raw pre').textContent(), `@src-${profile}/binary.dat`);
      await page.getByRole('button', { name: 'Clear prompt', exact: true }).click();

      // The same menu works on search results, including cancel and refresh after deletion.
      const search = page.getByRole('searchbox', { name: 'Search files by filename' });
      await search.fill('full.ts');
      const result = page.getByRole('button', { name: `src-${profile}/full.ts`, exact: true });
      await actionsMenu(page, result, profile === 'iphone');
      await page.getByRole('menuitem', { name: 'Delete…' }).click();
      await page.getByRole('button', { name: 'Cancel', exact: true }).click();
      assert.ok(fs.existsSync(path.join(workspace.dir, `src-${profile}/full.ts`)));
      await actionsMenu(page, result, profile === 'iphone');
      await page.getByRole('menuitem', { name: 'Delete…' }).click();
      await page.getByRole('button', { name: 'Delete', exact: true }).click();
      await result.waitFor({ state: 'detached' });
      assert.ok(!fs.existsSync(path.join(workspace.dir, `src-${profile}/full.ts`)));
      await page.getByRole('button', { name: 'Clear filename search' }).click();
      const trash = page.getByRole('button', { name: `trash-${profile}, folder`, exact: true });
      await actionsMenu(page, trash, profile === 'iphone');
      fs.mkdirSync('test-results/file-actions', { recursive: true });
      await page.screenshot({ path: `test-results/file-actions/${profile}-menu.png` });
      await page.getByRole('menuitem', { name: 'Delete…' }).click();
      await page.getByText('This permanently deletes the folder and everything inside it.', { exact: false }).waitFor();
      await page.getByRole('button', { name: 'Delete', exact: true }).click();
      await trash.waitFor({ state: 'detached' });
      assert.ok(!fs.existsSync(path.join(workspace.dir, `trash-${profile}`)));
      assert.deepEqual(errors, []);
    } finally { await ctx.close(); }
  });
}

test('deletion requires authentication and refuses the stream root and paths outside it', async () => {
  const base = `/api/streams/${encodeURIComponent(workspace.pane)}/files`;
  assert.equal((await request(server, 'DELETE', `${base}?path=src-iphone`, undefined, { cookie: false })).status, 401);
  // Pair a test device for the route guards.
  const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, 'delete-guards');
  assert.ok(paired.ok);
  for (const target of ['', '.', '..', '../secret.txt', '.git']) {
    assert.equal((await request(server, 'DELETE', `${base}?path=${encodeURIComponent(target)}`, undefined, { cookie: `__Host-briardr=${paired.cookie}` })).status, 403);
  }
  assert.ok(fs.existsSync(workspace.dir));
});
