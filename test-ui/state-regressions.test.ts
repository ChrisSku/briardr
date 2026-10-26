// Delayed note reads and pairing in place: regressions that fast local responses can hide.
import assert from 'node:assert/strict';
import crypto from 'node:crypto';
import { afterAll, beforeAll, test } from 'bun:test';
import { webkit, type Browser } from 'playwright';
import { closeWorkspace, installPlugin, request, sleep, startServer, testPane, waitUntil, type TestServer } from '../test/helpers.ts';
import { pageErrors } from './harness.ts';

let s: TestServer;
let browser: Browser;
let workspace: ReturnType<typeof testPane>;
beforeAll(async () => {
  s = await startServer();
  workspace = testPane('briardr-state-ui');
  await s.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await s?.close();
});

for (const scenario of ['open', 'event', 'saved', 'deleted'] as const) {
  test(`a delayed note ${scenario} response never replaces newer local edits`, async () => {
    const nid = crypto.randomBytes(8).toString('hex');
    const sid = workspace.pane;
    const endpoint = `/api/streams/${encodeURIComponent(sid)}/notes/${nid}`;
    const original = (await request(s, 'PUT', endpoint, { text: 'original note', baseRevision: 0 })).json.note;
    const ctx = await browser.newContext({ ignoreHTTPSErrors: true, serviceWorkers: 'block' });
    const page = await ctx.newPage();
    const errors = pageErrors(page);
    let release!: () => void;
    const held = new Promise<void>((r) => { release = r; });
    try {
      await ctx.addCookies([{ name: '__Host-briardr', value: s.cookie.slice('__Host-briardr='.length), domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
      await page.goto(`${s.base}/agents`);
      await page.waitForSelector('.count-strip');
      await page.evaluate(({ sid, note }) => {
        localStorage.setItem(`briardr.notes.list.${sid}`, JSON.stringify([note]));
        sessionStorage.setItem(`briardr.notes.open.${sid}`, note.id);
      }, { sid, note: original });
      let holdNext = scenario === 'open' || scenario === 'saved';
      let intercepted = false;
      let sawRead!: () => void;
      const reading = new Promise<void>((r) => { sawRead = r; });
      await page.route((url) => url.pathname === endpoint, async (route) => {
        if (route.request().method() !== 'GET' || !holdNext || intercepted) return route.continue();
        intercepted = true;
        const response = await route.fetch();
        sawRead();
        await held;
        await route.fulfill({ response });
      });
      const initialRead = !holdNext ? page.waitForResponse((r) => new URL(r.url()).pathname === endpoint && r.request().method() === 'GET') : null;
      await page.goto(`${s.base}/stream/${encodeURIComponent(sid)}/notes`);
      const area = page.locator('.notes-area');
      await area.waitFor();
      if (initialRead) {
        await initialRead;
        await sleep(100); // let the first read finish before triggering the live refresh
        holdNext = true;
        if (scenario === 'deleted') assert.equal((await request(s, 'DELETE', endpoint)).status, 200);
        else assert.equal((await request(s, 'PUT', endpoint, { text: 'another client changed it', baseRevision: 1 })).status, 200);
      }
      await reading;
      const typed = `local edit during ${scenario}`;
      await area.fill(typed);
      const saved = async () => (await request(s, 'GET', endpoint)).json.note?.text === typed;
      if (scenario === 'saved') await waitUntil('local edit saved before stale read arrives', saved);
      const delivered = page.waitForResponse((r) => new URL(r.url()).pathname === endpoint && r.request().method() === 'GET');
      release();
      await delivered;
      if (scenario === 'event' || scenario === 'deleted') {
        await page.getByRole('alert').filter({ hasText: scenario === 'deleted' ? 'deleted on another device' : 'changed on another device' }).waitFor();
        assert.equal(await area.inputValue(), typed);
        const draft = await page.evaluate(({sid,nid}) => JSON.parse(localStorage.getItem(`briardr.notes.draft.${sid}/${nid}`) ?? 'null'), {sid,nid});
        assert.equal(draft?.text, typed);
        await page.getByRole('button', { name: 'Keep mine' }).click();
      }
      await waitUntil('local edit saved', saved);
      await page.getByText('Saved on', { exact: false }).first().waitFor();
      assert.equal(await area.inputValue(), typed);
      await page.reload();
      await area.waitFor();
      assert.equal(await area.inputValue(), typed, 'saved text survives relaunch');
      assert.deepEqual(errors, []);
    } finally {
      release();
      await ctx.close();
    }
  });
}

test('pairing in place initializes installed and disabled plugins and follows remote toggles', async () => {
  installPlugin(s.dataDir, 'echo');
  const plugins = (await request(s, 'GET', '/api/plugins')).json.plugins;
  const echo = plugins.find((p: any) => p.id === 'echo');
  assert.equal((await request(s, 'PUT', '/api/plugins/echo', { enabled: true, approve: echo.backend.hash })).status, 200);
  assert.equal((await request(s, 'PUT', '/api/plugins/files', { enabled: false })).status, 200);
  assert.equal((await request(s, 'PUT', '/api/plugins/usage', { enabled: false })).status, 200);
  const ctx = await browser.newContext({ ignoreHTTPSErrors: true, serviceWorkers: 'block' });
  const page = await ctx.newPage();
  const errors = pageErrors(page);
  try {
    await page.goto(`${s.base}/`);
    await page.getByLabel('Pairing code').fill(s.run.auth.createPairingCode().code);
    await page.getByRole('button', { name: 'Sign in' }).click();
    await page.waitForSelector('.count-strip');
    assert.equal(await page.getByRole('button', { name: 'Usage', exact: true }).count(), 0);
    let navigations = 0;
    page.on('request', (r) => { if (r.isNavigationRequest() && r.frame() === page.mainFrame()) navigations++; });
    // SPA navigation retains the startup path that just paired; a page reload would hide the bug.
    await page.getByRole('button', { name: 'Workspaces', exact: true }).click();
    await page.locator('.tree-streams .row-main').first().click();
    await page.getByRole('tab', { name: 'Echo', exact: true }).waitFor();
    assert.equal(await page.getByRole('tab', { name: 'Files', exact: true }).count(), 0);
    await request(s, 'PUT', '/api/plugins/files', { enabled: true });
    await page.getByRole('tab', { name: 'Files', exact: true }).waitFor();
    await request(s, 'PUT', '/api/plugins/files', { enabled: false });
    await page.getByRole('tab', { name: 'Files', exact: true }).waitFor({ state: 'detached' });
    assert.equal(navigations, 0);
    assert.deepEqual(errors, []);
  } finally {
    await ctx.close();
    for (const id of ['files', 'usage']) await request(s, 'PUT', `/api/plugins/${id}`, { enabled: true });
    await request(s, 'PUT', '/api/plugins/echo', { enabled: false });
  }
});
