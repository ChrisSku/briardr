// Settings → Plugins: turning a built-in off removes its tab on this and other devices, a link to it
// shows the terminal, and turning it on brings it back. Usage, an app tab, leaves the tab bar.
import assert from 'node:assert/strict';
import { afterAll, beforeAll, test } from 'bun:test';
import { devices, webkit, type Browser, type BrowserContext, type Page } from 'playwright';
import fs from 'node:fs';
import path from 'node:path';
import { buildPlugin, closeWorkspace, herdr, installPlugin, readPane, sleep, startServer, testPane, type TestServer } from '../test/helpers.ts';
import { pageErrors } from './harness.ts';

let server: TestServer;
let browser: Browser;
let workspace: ReturnType<typeof testPane>;

beforeAll(async () => {
  server = await startServer({ pair: false });
  workspace = testPane('briardr-plugins-ui');
  herdr('pane', 'rename', workspace.pane, 'plugins target');
  await server.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await server?.close();
});

async function open(profile: 'iphone' | 'mac'): Promise<{ ctx: BrowserContext; page: Page; errors: string[] }> {
  const ctx = await browser.newContext({
    ...(profile === 'iphone' ? devices['iPhone 15 Pro'] : { viewport: { width: 1280, height: 800 } }),
    ignoreHTTPSErrors: true,
    serviceWorkers: 'block',
  });
  const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, `plugins-${profile}`);
  assert.ok(paired.ok);
  await ctx.addCookies([{ name: '__Host-briardr', value: paired.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
  const page = await ctx.newPage();
  const errors = pageErrors(page);
  return { ctx, page, errors };
}

for (const profile of ['iphone', 'mac'] as const) {
  test(`turning plugins off and on in Settings (${profile})`, async () => {
    const { ctx, page, errors } = await open(profile);
    const other = await open(profile);
    try {
      const stream = `${server.base}/stream/${encodeURIComponent(workspace.pane)}`;
      await other.page.goto(stream);
      await other.page.getByRole('tab', { name: 'Docs' }).waitFor();

      await page.goto(`${server.base}/settings/plugins`);
      const docs = page.getByRole('switch', { name: /^Docs/ });
      assert.equal(await docs.isChecked(), true);
      await docs.click();
      await page.waitForFunction(() => [...document.querySelectorAll<HTMLInputElement>('[role=switch]')].every((e) => !e.disabled));
      assert.equal(await docs.isChecked(), false);
      // The other device hears about it over its live connection.
      await other.page.getByRole('tab', { name: 'Docs' }).waitFor({ state: 'detached' });
      assert.equal(await other.page.getByRole('tab', { name: 'Notes' }).count(), 1);

      // A link to a tab that's off shows the terminal.
      await page.goto(`${stream}/docs`);
      await page.getByRole('tab', { name: 'Terminal', selected: true }).waitFor();
      assert.equal(await page.getByRole('tab', { name: 'Docs' }).count(), 0);

      // Usage is an app tab: it leaves the tab bar (or sidebar).
      await page.goto(`${server.base}/settings/plugins`);
      await page.getByRole('switch', { name: /^Usage/ }).click();
      await page.getByRole('button', { name: 'Usage', exact: true }).waitFor({ state: 'detached' });

      await page.getByRole('switch', { name: /^Docs/ }).click();
      await page.getByRole('switch', { name: /^Usage/ }).click();
      await page.getByRole('button', { name: 'Usage', exact: true }).waitFor();
      await other.page.getByRole('tab', { name: 'Docs' }).waitFor();
      const list = await page.evaluate(() => fetch('/api/plugins').then((r) => r.json()));
      assert.ok(list.plugins.every((p: any) => !p.builtin || p.enabled));
      assert.deepEqual(errors, []);
      assert.deepEqual(other.errors, []);
    } finally {
      await ctx.close();
      await other.ctx.close();
    }
  });
}

test('an installed plugin loads at runtime, shares Briardr’s Svelte and SDK, and reloads when rebuilt', async () => {
  const dir = installPlugin(server.dataDir, 'echo');
  const { ctx, page, errors } = await open('mac');
  try {
    await page.goto(`${server.base}/settings/plugins`);
    await page.getByRole('switch', { name: /^Echo/ }).click();
    // It has a backend: Briardr shows the command and asks first.
    const dialog = page.getByRole('dialog');
    await dialog.getByText('bun server/main.ts').waitFor();
    await dialog.getByRole('button', { name: 'Turn on' }).click();
    await page.waitForFunction(() => [...document.querySelectorAll<HTMLInputElement>('[role=switch]')].every((e) => !e.disabled));
    await page.goto(`${server.base}/stream/${encodeURIComponent(workspace.pane)}/echo`);
    await page.getByRole('tab', { name: 'Echo', selected: true }).waitFor();
    // The SDK's Empty component and the plugin's own (injected) styles.
    await page.getByText('The echo plugin is running').waitFor();
    assert.equal(await page.locator('.echo-tab strong').evaluate((e) => getComputedStyle(e).color), 'rgb(102, 51, 153)');
    // $state and $derived in the plugin run on the app's runtime.
    await page.getByRole('button', { name: /Clicked 0, doubled 0/ }).click();
    await page.getByRole('button', { name: /Clicked 1, doubled 2/ }).waitFor();
    // Inserting goes through the stream's terminal controller, once this device controls it.
    const send = page.getByRole('button', { name: 'Send to terminal' });
    await page.waitForFunction(() => !(document.querySelector('.echo-tab button:last-of-type') as HTMLButtonElement)?.disabled);
    await send.click();
    let text = '';
    for (let i = 0; i < 40 && !text.includes('echo from plugin'); i++) await sleep(100), (text = readPane(workspace.pane));
    assert.match(text, /echo from plugin/);

    // The backend answers through Briardr, and its events arrive over the live connection.
    await page.getByRole('button', { name: 'Ask backend' }).click();
    await page.getByTestId('answer').getByText(`plugins-mac in ${workspace.pane}`).waitFor();
    await page.getByRole('button', { name: 'Emit' }).click();
    await page.getByText('Events: 1').waitFor();

    // A rebuild reaches the open page without a reload.
    const tab = path.join(dir, 'src', 'Tab.svelte');
    fs.writeFileSync(tab, fs.readFileSync(tab, 'utf8').replace('Clicked {count}', 'Pressed {count}'));
    buildPlugin(dir);
    await page.getByRole('button', { name: /Pressed 0, doubled 0/ }).waitFor({ timeout: 8000 });
    assert.deepEqual(errors, []);
  } finally {
    await ctx.close();
  }
});

