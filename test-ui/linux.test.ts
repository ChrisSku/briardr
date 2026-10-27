// A Linux host in WebKit at iPhone size: Mac-only controls are absent and the app says "server".
// Each test stands alone: the pairing-screen test uses an unpaired context, and the rest share a
// context that pairs once in beforeAll, so the tests run in any order and alone.
import assert from 'node:assert/strict';
import { execFileSync, spawn, type ChildProcess } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { devices, webkit, type Browser, type Page } from 'playwright';
import { closeWorkspace, freePort, herdr, SESSION, sleep, testPane, tmpDir } from '../test/helpers.ts';
import { serverArgv } from './harness.ts';

const dataDir = tmpDir('briardr-linux-ui-');
const logFile = path.join(dataDir, 'server.log');
let base = '';
let server: ChildProcess;
let browser: Browser;
let anon: Page;
let paired: Page;
let ws: { pane: string; workspace: string };

function serverEnv(port: number): NodeJS.ProcessEnv {
  return { ...process.env, BRIARDR_PLATFORM: 'linux', BRIARDR_DATA_DIR: dataDir, BRIARDR_HERDR_SESSION: SESSION, BRIARDR_PORT: String(port), BRIARDR_PREVIEW_PORT: String(port + 1), BRIARDR_LISTEN: '' };
}

beforeAll(async () => {
  process.env.NODE_TLS_REJECT_UNAUTHORIZED = '0';
  const port = await freePort();
  base = `https://localhost:${port}`;
  const env = serverEnv(port);
  ws = testPane('briardr-linux-ui');
  herdr('pane', 'rename', ws.pane, 'linux target');
  server = spawn(...serverArgv('serve'), { env, stdio: ['ignore', 'ignore', fs.openSync(logFile, 'w')] });
  let ready = false;
  for (let i = 0; i < 75 && !ready; i++) {
    if (server.exitCode !== null) break;
    ready = Boolean(await fetch(`${base}/api/session`).catch(() => null));
    if (!ready) await sleep(200);
  }
  if (!ready) throw new Error(`server did not start on ${base}: ${fs.existsSync(logFile) ? fs.readFileSync(logFile, 'utf8') : '(no log)'}`);

  browser = await webkit.launch();
  const opts = { ...devices['iPhone 15 Pro'], ignoreHTTPSErrors: true };
  anon = await (await browser.newContext(opts)).newPage();
  paired = await (await browser.newContext(opts)).newPage();

  await paired.goto(base + '/');
  await paired.getByText('Sign in this device to see and steer the work running on your server.').waitFor();
  const out = execFileSync(...serverArgv('pair'), { env, encoding: 'utf8' });
  await paired.getByLabel('Pairing code').fill(out.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/)![0]);
  await paired.getByRole('button', { name: 'Sign in' }).click();
  await paired.waitForSelector('.count-strip');
});
afterAll(async () => {
  server?.kill();
  await browser?.close();
  closeWorkspace(ws.workspace);
});

describe('Linux host', () => {
  test('the pairing screen speaks of the server', async () => {
    await anon.goto(base + '/');
    await anon.getByText('Sign in this device to see and steer the work running on your server.').waitFor();
    assert.ok(await anon.getByText('on the server with', { exact: false }).isVisible());
  });

  test('the host summary shows its name and no battery', async () => {
    await paired.goto(base + '/settings');
    await paired.waitForSelector('.mac-summary .mac-name');
    assert.equal(await paired.locator('.mac-summary').getAttribute('aria-label'), 'Server summary');
    assert.equal(await paired.locator('.mac-batt').count(), 0, 'no battery');
  });

  test('Settings has no Mac power, Keychain or low-battery controls', async () => {
    await paired.goto(base + '/settings');
    await paired.locator('.set-group').first().waitFor();
    const list = await paired.locator('.settings').innerText();
    assert.ok(!/Power/.test(list), list);
    await paired.goto(base + '/settings/power');
    await paired.getByText('There’s no such section.').waitFor();
    assert.equal(await paired.locator('#set-power').count(), 0);
    assert.equal(await paired.getByRole('button', { name: /Sleep Mac/ }).count(), 0);
    await paired.goto(base + '/settings/usage');
    await paired.getByRole('button', { name: /^Sources/ }).click();
    await paired.getByRole('dialog', { name: 'Usage sources' }).getByText('Codex: read limits through the Codex app server').waitFor();
    assert.equal(await paired.getByText('Claude: read limits with Claude Code’s sign-in').count(), 0);
    await paired.goto(base + '/settings/notifications');
    await paired.locator('#set-notifications').waitFor();
    assert.equal(await paired.getByText('Mac low battery').count(), 0);
    await paired.goto(base + '/settings/about');
    await paired.getByText('the server doesn’t transcribe speech', { exact: false }).waitFor();
    await paired.goto(base + '/settings/herdr');
    await paired.getByText('Path to the herdr executable on the server').waitFor();
  });

  test('Enter is a plain Enter key: holding it doesn’t start server dictation', async () => {
    await paired.goto(base + '/workspaces');
    await paired.locator('.tree-streams .row-main', { hasText: 'linux target' }).click();
    await paired.waitForSelector('[data-ownership=controlling]');
    const enter = paired.getByRole('button', { name: 'Enter', exact: true });
    await enter.waitFor();
    assert.equal(await paired.getByRole('button', { name: 'Enter. Hold to dictate.' }).count(), 0);
    // A long press is still just Enter: no recording starts.
    const box = (await enter.boundingBox())!;
    await paired.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
    await paired.mouse.down();
    await sleep(700);
    assert.equal(await paired.locator('.key-enter.is-dictating').count(), 0);
    await paired.mouse.up();
  });
});
