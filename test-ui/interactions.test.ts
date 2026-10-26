// Browser interaction checks in WebKit (Safari's engine) at iPhone size, against an in-process
// Briardr server and the test run's own Herdr session. Verifies what actually reaches the program.
import assert from 'node:assert/strict';
import { execFile } from 'node:child_process';
import http from 'node:http';
import path from 'node:path';
import { afterAll, afterEach, beforeAll, beforeEach, describe, onTestFinished, test } from 'bun:test';
import { promisify } from 'node:util';
import { devices, webkit, type Browser, type BrowserContext, type Locator, type Page } from 'playwright';
import { closeWorkspace, herdr, request, SESSION, sleep, startServer, testPane, WAIT, waitUntil, type TestServer } from '../test/helpers.ts';

let s: TestServer;
/** The host as the server under test describes it before pairing: the "Mac" on macOS, the "server" on Linux
 *  (CI). The client words its messages after it, and only a Mac dictates (the Enter key's hold). */
let host: { noun: string; platform: string };
let browser: Browser;
let ws: { pane: string; workspace: string; dir: string };
const rawecho = path.resolve('test/fixtures/rawecho.py');

beforeAll(async () => {
  s = await startServer({ pair: false });
  host = (await request(s, 'GET', '/api/session', undefined, { cookie: false })).json.host;
  ws = testPane('briardr-ui');
  herdr('pane', 'rename', ws.pane, 'ui target');
  herdr('pane', 'send-keys', ws.pane, 'ctrl+]', 'ctrl+c'); // a clean shell prompt
  await s.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  closeWorkspace(ws.workspace);
  await s.close();
});

let pairedViaUi = false;

async function device(profile: 'iphone' | 'mac' = 'iphone', name = 'ui'): Promise<{ ctx: BrowserContext; page: Page }> {
  const base = profile === 'iphone' ? devices['iPhone 15 Pro'] : { viewport: { width: 1280, height: 800 } };
  const ctx = await browser.newContext({ ...base, ignoreHTTPSErrors: true, locale: 'en-US' });
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.log('pageerror', e.message));
  if (!pairedViaUi) {
    // The first device pairs through the real pairing screen.
    pairedViaUi = true;
    await page.goto(s.base + '/');
    const { code } = s.run.auth.createPairingCode();
    await page.getByLabel('Pairing code').fill(code);
    await page.getByLabel('Name for this device').fill(name);
    await page.getByRole('button', { name: 'Sign in' }).click();
  } else {
    // Others get a device cookie directly: the HTTP pairing route is rate-limited (10 per 15 min),
    // which these many test devices would otherwise trip.
    const p = s.run.auth.pair(s.run.auth.createPairingCode().code, name);
    if (!p.ok) throw new Error('pairing failed');
    await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    await page.goto(s.base + '/agents');
  }
  await page.waitForSelector('.count-strip');
  await page.waitForLoadState('load');
  return { ctx, page };
}

async function openTarget(page: Page) {
  await page.goto(s.base + '/workspaces');
  await page.locator('.tree-streams .row-main', { hasText: 'ui target' }).click();
  await page.waitForSelector('[data-ownership=controlling]', { timeout: 10_000 });
}

/** Start the byte-echo program in the pane (from the API side, not the UI under test); `--kitty` turns
 *  on the kitty keyboard protocol in it. */
async function startEcho(mode: '' | '--kitty' = '') {
  herdr('pane', 'run', ws.pane, `clear; python3 ${rawecho} ${mode}`);
  for (let i = 0; i < 30; i++) {
    if ((await paneText('recent', 10)).includes('READY')) return;
    await sleep(100);
  }
  throw new Error('echo program did not start');
}
async function stopEcho() {
  herdr('pane', 'send-keys', ws.pane, 'ctrl+]');
  await sleep(300);
}
// Polling reads the pane asynchronously: the Briardr server under test runs in this process, and a
// synchronous `herdr` call would freeze it (and its WebSockets) on every poll.
const run = promisify(execFile);
async function paneText(source: 'recent' | 'recent-unwrapped', lines: number): Promise<string> {
  const env: NodeJS.ProcessEnv = {};
  for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('HERDR_')) env[k] = v;
  const { stdout } = await run('herdr', ['--session', SESSION, 'pane', 'read', ws.pane, '--source', source, '--lines', String(lines)], { env, encoding: 'utf8' });
  try {
    return JSON.stringify(JSON.parse(stdout));
  } catch {
    return stdout;
  }
}
async function bytesSeen(): Promise<string[]> {
  // Only output after this test's own READY: earlier echo programs leave BYTES lines in the pane.
  const text = await paneText('recent-unwrapped', 80);
  const at = text.lastIndexOf('READY');
  if (at < 0) return [];
  return text.slice(at)
    .split('\n')
    .filter((l) => l.startsWith('BYTES '))
    .map((l) => l.slice(6).trim());
}
async function expectBytes(hex: string, ms = WAIT) {
  const t0 = Date.now();
  while (Date.now() - t0 < ms) {
    if ((await bytesSeen()).join('').includes(hex)) return;
    await sleep(100);
  }
  assert.fail(`expected bytes ${hex}; saw ${(await bytesSeen()).slice(-6).join(' ')}`);
}
const hex = (s: string) => Buffer.from(s, 'latin1').toString('hex');

/** The screen point `offset` cells into `needle` in the page's terminal, once it shows there; lines with the
 *  typed command (`echo`, `printf`) don't count. Rendering is WebGL, so there's no text in the DOM. Run the
 *  command after the page takes control: a resize after it makes zsh redraw a wrapped prompt over the line
 *  above it. */
async function cellOf(page: Page, needle: string, offset = 2): Promise<{ x: number; y: number }> {
  let pt: { x: number; y: number } | null = null;
  const find = () =>
    page.evaluate(
      ([needle, offset]) => {
        const ctl = (document.querySelector('.term-host') as any).briardrTerminal;
        const buf = ctl.term.buffer.active;
        for (let r = 0; r < ctl.term.rows; r++) {
          const text = buf.getLine(buf.viewportY + r)?.translateToString(true) || '';
          const c = text.indexOf(needle);
          if (c >= 0 && !text.includes('echo') && !text.includes('printf')) {
            const screen = document.querySelector('.xterm-screen')!.getBoundingClientRect();
            const cell = ctl.cell();
            return { x: screen.left + (c + offset) * cell.w, y: screen.top + (r + 0.5) * cell.h };
          }
        }
        return null;
      },
      [needle, offset] as const,
    );
  await waitUntil(`${needle} in the terminal`, async () => !!(pt = await find()), 10_000);
  return pt!;
}

/** Simulates the on-screen keyboard: the visual viewport loses `height` px at the bottom (0 hides it). */
async function showKeyboard(page: Page, height: number) {
  await page.evaluate((h) => {
    if (h) Object.defineProperty(window.visualViewport!, 'height', { value: innerHeight - h, configurable: true });
    else delete (window.visualViewport as any).height;
    window.visualViewport!.dispatchEvent(new Event('resize'));
  }, height);
  await sleep(100);
}
/** Asserts that each element ends above a keyboard `height` px tall. */
async function aboveKeyboard(page: Page, height: number, els: Record<string, Locator>) {
  const top = page.viewportSize()!.height - height;
  for (const [name, el] of Object.entries(els)) {
    const b = (await el.boundingBox())!;
    assert.ok(b.y + b.height <= top + 0.5, `${name} ends at ${b.y + b.height}, under the keyboard at ${top}`);
  }
}

describe('terminal input (iPhone)', () => {
  let page: Page;
  let ctx: BrowserContext;
  beforeAll(async () => {
    ({ ctx, page } = await device('iphone', 'iphone-ui'));
    await openTarget(page);
  });
  afterAll(async () => {
    await ctx.close();
  });
  // Each test gets its own echo program, so no test depends on what an earlier one typed.
  beforeEach(() => startEcho());
  // These tests share one page: close anything a test (or its failure) left open, so it can't block
  // the next one.
  afterEach(async () => {
    await stopEcho();
    for (let i = 0; i < 4; i++) {
      const close = page.locator('.sheet-head [aria-label="Close"]');
      if (await close.count()) await close.first().click();
      else if (await page.locator('[role=menu]').count()) await page.keyboard.press('Escape');
      else break;
    }
  });

  test('typing goes to the program; the bar’s Enter is a carriage return', async () => {
    await page.locator('.term-host').click();
    await page.keyboard.type('hi');
    await expectBytes(hex('h'));
    await expectBytes(hex('i'));
    await page.getByRole('button', { name: host.platform === 'macos' ? 'Enter. Hold to dictate.' : 'Enter', exact: true }).click();
    await expectBytes('0d');
  });

  test('a tapped Control modifier applies to the next key only', async () => {
    await page.getByRole('button', { name: 'Control', exact: true }).click();
    await page.locator('.term-host').click();
    await page.keyboard.type('c');
    await expectBytes('03'); // ctrl+c
    await page.keyboard.type('c');
    await expectBytes(hex('c'));
  });

  test('Command reaches the program as a super chord', async () => {
    // Herdr 0.9.4 and newer send super chords only to a program that turned on a keyboard protocol.
    await stopEcho();
    await startEcho('--kitty');
    await page.getByRole('button', { name: 'Command', exact: true }).click();
    await page.locator('.term-host').click();
    await page.keyboard.type('a');
    await expectBytes(hex('\x1b[97;9u')); // kitty protocol: a, modifiers 1 + super(8)
    await page.getByRole('button', { name: 'Command', exact: true }).click();
    await page.getByRole('button', { name: 'esc (Escape)', exact: true }).click();
    await expectBytes(hex('\x1b[27;9u'));
  });

  test('Jump opens under the header, and its field stays put while the list shrinks', async () => {
    await page.getByRole('button', { name: 'Jump to a stream' }).click();
    const field = page.getByLabel('Search streams');
    await page.waitForFunction(() => document.activeElement?.getAttribute('aria-label') === 'Search streams');
    const head = (await page.locator('.stream-head').boundingBox())!;
    const sheet = (await page.locator('.sheet.jump').boundingBox())!;
    assert.ok(sheet.y >= head.y + head.height && sheet.y <= head.y + head.height + 16, `sheet top ${sheet.y} just under header ${head.y + head.height}`);
    const before = (await field.boundingBox())!.y;
    await field.fill('no stream is called this');
    await page.getByText('No matching streams.').waitFor();
    assert.equal((await field.boundingBox())!.y, before);
    // With the on-screen keyboard up, the sheet ends above it.
    await showKeyboard(page, 300);
    await field.fill('');
    await sleep(100);
    await aboveKeyboard(page, 300, { sheet: page.locator('.sheet.jump') });
    await showKeyboard(page, 0);
  });

  test('sheets over the terminal sit on the on-screen keyboard: their fields and buttons stay above it', async () => {
    await page.getByRole('button', { name: 'Stream actions' }).click();
    await page.getByRole('menuitem', { name: 'Rename' }).click();
    const rename = page.getByRole('dialog', { name: 'Rename stream' });
    await page.waitForFunction(() => document.activeElement?.closest('[role=dialog]'));
    await showKeyboard(page, 300);
    try {
      await aboveKeyboard(page, 300, { 'Rename field': rename.getByLabel('Name'), Save: rename.getByRole('button', { name: 'Save' }) });
      await rename.getByRole('button', { name: 'Cancel' }).click();
      await page.getByRole('button', { name: 'Insert text, image or file' }).click();
      await page.getByRole('menuitem', { name: /Paste text/ }).click();
      const paste = page.getByRole('dialog', { name: 'Paste text' });
      await page.waitForFunction(() => document.activeElement?.classList.contains('paste-area'));
      await sleep(100);
      // The sheet fits above the keyboard; its text field starts in view and the sheet body scrolls.
      await aboveKeyboard(page, 300, { 'Paste sheet': paste, Insert: paste.getByRole('button', { name: 'Insert', exact: true }) });
      const area = (await paste.getByLabel('Text to paste').boundingBox())!;
      assert.ok(area.y + 60 <= page.viewportSize()!.height - 300, `text field starts at ${area.y}`);
    } finally {
      await showKeyboard(page, 0);
    }
  });

  test('the on-screen keyboard resizes the pane to the rows above it, so a full-screen program fits', async () => {
    const size = () => page.evaluate(() => {
      const { term } = (document.querySelector('.term-host') as any).briardrTerminal;
      return { cols: term.cols as number, rows: term.rows as number };
    });
    const full = await size();
    await showKeyboard(page, 300);
    try {
      await aboveKeyboard(page, 300, { 'key bar': page.locator('.keybar-wrap') });
      await waitUntil('the pane to take the rows above the keyboard', async () => (await size()).rows < full.rows);
      // All of it is in view: its top under the tabs, its bottom on the key bar.
      const screen = (await page.locator('.term-shift').boundingBox())!;
      const host = (await page.locator('.term-host').boundingBox())!;
      const bar = (await page.locator('.keybar-wrap').boundingBox())!;
      assert.ok(screen.y >= host.y, `the screen starts at ${screen.y}, above the terminal at ${host.y}`);
      assert.ok(Math.abs(screen.y + screen.height - bar.y) < 4, `the screen ends at ${screen.y + screen.height}, the key bar starts at ${bar.y}`);
      // The program sees the new size.
      const { cols, rows } = await size();
      await stopEcho();
      herdr('pane', 'run', ws.pane, 'clear; stty size');
      await waitUntil(`stty size to print ${rows} ${cols}`, async () => (await paneText('recent', 10)).includes(`${rows} ${cols}`), 10_000);
    } finally {
      await showKeyboard(page, 0);
      await waitUntil('the pane to take its full size back', async () => (await size()).rows === full.rows);
      await startEcho(); // the shell has the pane; afterEach stops the echo program
    }
  });

  test('tapping the stream name, or the space after it, opens Jump', async () => {
    const isOpen = () => page.waitForFunction(() => document.activeElement?.getAttribute('aria-label') === 'Search streams');
    const closed = () => page.locator('.sheet.jump').waitFor({ state: 'detached' });
    if (await page.locator('.sheet.jump').count()) {
      await page.keyboard.press('Escape');
      await closed();
    }
    await page.locator('.stream-head h1').click();
    await isOpen();
    await page.keyboard.press('Escape');
    await closed();
    const main = (await page.locator('.stream-head .head-main').boundingBox())!;
    await page.mouse.click(main.x + main.width - 4, main.y + main.height / 2);
    await isOpen();
    await page.keyboard.press('Escape');
    await closed();
  });

  test('a double-tapped modifier locks; holding a modifier forms a chord', async () => {
    const ctrl = page.locator('.keys-main .mod').nth(0); // stable: its accessible name changes with state
    await ctrl.click();
    await ctrl.click(); // double tap → locked
    assert.equal(await ctrl.getAttribute('aria-label'), 'Control, locked');
    await page.locator('.term-host').click();
    await page.keyboard.type('a');
    await page.keyboard.type('b');
    await expectBytes('0102'); // ctrl+a, ctrl+b: still locked
    await ctrl.click(); // unlock
    assert.equal(await ctrl.getAttribute('aria-label'), 'Control');
    // Hold Option while pressing Escape on the bar → alt+esc chord; releasing ends it.
    const opt = page.locator('.keys-main .mod').nth(1);
    const box = (await opt.boundingBox())!;
    await opt.dispatchEvent('pointerdown', { pointerId: 21, clientX: box.x + 5, clientY: box.y + 5, pointerType: 'touch', isPrimary: false });
    await sleep(400);
    assert.equal(await opt.getAttribute('aria-label'), 'Option, held');
    await page.getByRole('button', { name: 'esc (Escape)', exact: true }).click();
    await expectBytes('1b1b');
    await opt.dispatchEvent('pointerup', { pointerId: 21, pointerType: 'touch', isPrimary: false });
    assert.equal(await opt.getAttribute('aria-label'), 'Option');
  });

  test('Escape and Tab keys; arrows use the program’s cursor-key mode', async () => {
    await page.getByRole('button', { name: 'esc (Escape)', exact: true }).click();
    await expectBytes('1b');
    await page.getByRole('button', { name: 'tab (Tab)', exact: true }).click();
    await expectBytes('09');
    await page.getByRole('button', { name: 'Shift-Tab', exact: true }).click();
    await expectBytes(hex('\x1b[Z'));
    await page.getByRole('button', { name: '|', exact: true }).click();
    await expectBytes(hex('|'));
    await page.getByRole('button', { name: 'Control-D', exact: true }).click();
    await expectBytes('04');
    await page.locator('.arrowpad button.sr-only', { hasText: 'Arrow up' }).dispatchEvent('click');
    await expectBytes(hex('\x1bOA')); // application cursor mode, as the program requested
  });

  test('Delete sits above Enter, as wide; a tap deletes once, holding keeps deleting', async () => {
    const del = page.locator('.key-delete');
    const [d, e] = [(await del.boundingBox())!, (await page.locator('.key-enter').boundingBox())!];
    assert.ok(Math.abs(d.x - e.x) < 1 && Math.abs(d.width - e.width) < 1, `Delete ${d.x}+${d.width} lines up with Enter ${e.x}+${e.width}`);
    assert.equal(await page.getByRole('button', { name: 'Control-Z', exact: true }).count(), 0);
    const count = async () => (await bytesSeen()).join('').split('7f').length - 1;
    const before = await count();
    await del.click();
    await expectBytes('7f');
    await sleep(500);
    assert.equal((await count()) - before, 1, 'one tap, one backspace');
    await del.dispatchEvent('pointerdown', { pointerId: 12, pointerType: 'touch', isPrimary: true });
    await sleep(900);
    await del.dispatchEvent('pointerup', { pointerId: 12, pointerType: 'touch', isPrimary: true });
    await sleep(300);
    assert.ok((await count()) - before >= 4, `held Delete repeated (${(await count()) - before - 1} more)`);
  });

  test('the drag-to-repeat arrow pad sends repeated arrows while held', async () => {
    const pad = page.locator('.arrowpad .pad');
    const box = (await pad.boundingBox())!;
    const before = (await bytesSeen()).join('').split(hex('\x1bOC')).length;
    await pad.dispatchEvent('pointerdown', { pointerId: 9, clientX: box.x + box.width / 2, clientY: box.y + box.height / 2, pointerType: 'touch', isPrimary: true });
    await pad.dispatchEvent('pointermove', { pointerId: 9, clientX: box.x + box.width / 2 + 30, clientY: box.y + box.height / 2, pointerType: 'touch', isPrimary: true });
    await sleep(900);
    await pad.dispatchEvent('pointerup', { pointerId: 9, pointerType: 'touch', isPrimary: true });
    await sleep(300);
    const count = (await bytesSeen()).join('').split(hex('\x1bOC')).length - before;
    assert.ok(count >= 3, `right arrow repeated (${count} times)`);
  });

  test('Paste text is reviewed, inserted as one bracketed paste, and never submitted', async () => {
    await page.getByRole('button', { name: 'Insert text, image or file' }).click();
    await page.getByRole('menuitem', { name: /Paste text/ }).click();
    const area = page.getByLabel('Text to paste');
    // The sheet focuses its text field (and doesn't move focus away while you type).
    await page.waitForFunction(() => document.activeElement?.classList.contains('paste-area'));
    await area.fill('line one\nline two');
    assert.equal(await area.inputValue(), 'line one\nline two');
    await page.getByRole('button', { name: 'Insert', exact: true }).click();
    await expectBytes(hex('\x1b[200~line one\nline two\x1b[201~'));
    await sleep(300);
    assert.ok(!(await bytesSeen()).slice(-2).join('').endsWith('0d'), 'no Enter after the paste');
  });

  test('an image is reviewed, made a PNG within 4096 px, uploaded privately, and its path inserted without Enter', async () => {
    await page.getByRole('button', { name: 'Insert text, image or file' }).click();
    const chooser = page.waitForEvent('filechooser');
    await page.getByRole('menuitem', { name: /Select image/ }).click();
    await (await chooser).setFiles(path.resolve('test/fixtures/big-photo.jpg'));
    await page.getByText(/× \d+ px PNG/).waitFor();
    const info = await page.locator('.upload-preview .hint').textContent();
    const [w, h] = info!.match(/(\d+) × (\d+)/)!.slice(1).map(Number);
    assert.ok(Math.max(w, h) <= 4096, `scaled to ${w}×${h}`);
    assert.match(info!, /camera and location details removed/);
    await page.getByRole('button', { name: 'Upload and insert path' }).click();
    await page.getByRole('dialog').waitFor({ state: 'detached' });
    let m: RegExpMatchArray | undefined;
    for (let i = 0; i < 40 && !m; i++) {
      await sleep(100);
      const all = Buffer.from((await bytesSeen()).join(''), 'hex').toString('latin1');
      m = [...all.matchAll(/\x1b\[200~(.*?)\x1b\[201~/gs)].filter((x) => x[1].includes('briardr-uploads')).pop();
    }
    assert.ok(m, 'path inserted as one bracketed paste');
    const p = m![1].replace(/^'|'$/g, '');
    assert.match(p, /briardr-uploads\/[0-9a-f]{24}\/big-photo\.png$/);
    const fs = await import('node:fs');
    assert.equal(fs.statSync(p).mode & 0o777, 0o600);
    const head = fs.readFileSync(p).subarray(0, 24);
    assert.equal(head.toString('latin1', 1, 4), 'PNG');
    assert.ok(Math.max(head.readUInt32BE(16), head.readUInt32BE(20)) <= 4096);
    assert.ok(!fs.readFileSync(p).includes(Buffer.from('Exif')), 'no EXIF metadata');
    await sleep(300);
    assert.ok(!(await bytesSeen()).join('').endsWith('0d'), 'Enter was not pressed');
  });

  test('something picked before a reconnect is never inserted after it', async () => {
    const r = await page.evaluate(() => {
      const ctl = (document.querySelector('.term-host') as any).briardrTerminal;
      return ctl.paste('stale-insert', true, ctl.generation.value - 1);
    });
    assert.equal(r, false);
    await page.getByText(`Not inserted: the connection to the ${host.noun} changed since you picked it.`).waitFor();
    await sleep(300);
    assert.ok(!Buffer.from((await bytesSeen()).join(''), 'hex').toString('latin1').includes('stale-insert'));
  });

  test('a vertical swipe sends one wheel event per row crossed, anchored at the start cell', async () => {
    const host = page.locator('.term-host');
    const box = (await host.boundingBox())!;
    const cellH = await page.evaluate(() => (document.querySelector('.term-host') as any).briardrTerminal.cell().h);
    const x = box.x + 60;
    const y0 = box.y + 200;
    const rows = 5;
    const before = (await bytesSeen()).join('');
    await host.dispatchEvent('pointerdown', { pointerId: 3, clientX: x, clientY: y0, pointerType: 'touch', isPrimary: true });
    const total = (rows + 0.5) * cellH; // cross exactly `rows` row boundaries
    for (let i = 1; i <= 10; i++) await host.dispatchEvent('pointermove', { pointerId: 3, clientX: x, clientY: y0 + (i * total) / 10, pointerType: 'touch', isPrimary: true });
    await host.dispatchEvent('pointerup', { pointerId: 3, clientX: x, clientY: y0 + total, pointerType: 'touch', isPrimary: true });
    await sleep(600);
    const added = (await bytesSeen()).join('').slice(before.length);
    const wheel = Buffer.from(added, 'hex').toString('latin1').match(/\x1b\[<64;(\d+);(\d+)M/g) || [];
    assert.equal(wheel.length, rows, `one wheel-up per row crossed (${wheel.length})`);
    assert.equal(new Set(wheel).size, 1, 'all anchored at the cell where the gesture started');
  });
});

describe('ownership (two devices)', () => {
  test('a second device watches; Take Over needs confirmation; the first is told', async () => {
    const a = await device('iphone', 'phone-a');
    await openTarget(a.page);
    const b = await device('mac', 'mac-b');
    await b.page.goto(s.base + '/workspaces');
    await b.page.locator('.tree-streams .row-main', { hasText: 'ui target' }).click();
    await b.page.waitForSelector('[data-ownership=watching]');
    await b.page.getByText('Another client is controlling this stream').waitFor();
    // Cancel first: nothing changes.
    await b.page.getByRole('button', { name: 'Take Over' }).click();
    await b.page.getByRole('button', { name: 'Cancel' }).click();
    await sleep(500);
    assert.equal(await a.page.locator('[data-ownership=controlling]').count(), 1);
    // Confirm: control moves, and the first device switches to watching with a notice.
    await b.page.getByRole('button', { name: 'Take Over' }).click();
    await b.page.getByRole('dialog').getByRole('button', { name: 'Take Over' }).click();
    await b.page.waitForSelector('[data-ownership=controlling]');
    await a.page.getByText('Another client took control').waitFor();
    assert.equal(await a.page.locator('[data-ownership=watching]').count(), 1);
    await a.ctx.close();
    await b.ctx.close();
  });

  test('backgrounding releases control; returning resumes by itself, never taking over', async () => {
    // A closed connection's control takes a moment to get back to Herdr; a device asking for control
    // before that only watches. So each step waits until the server has given control back.
    const released = (what: string) => waitUntil(what, () => s.run.hub.controllers === 0);
    const a = await device('iphone', 'phone-bg');
    await openTarget(a.page);
    await a.page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await released('the backgrounded phone’s control to go back');
    // The pane is free: another device can take control without a takeover.
    const b = await device('mac', 'mac-free');
    await b.page.goto(s.base + '/workspaces');
    await b.page.locator('.tree-streams .row-main', { hasText: 'ui target' }).click();
    await b.page.waitForSelector('[data-ownership=controlling]');
    await b.ctx.close();
    await released('the closed Mac’s control to go back');
    await a.page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await a.page.waitForSelector('[data-ownership=controlling]');
    assert.equal(await a.page.getByText('Paused').count(), 0);
    // Background again; this time another device takes the pane meanwhile. Returning only watches.
    await a.page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { value: 'hidden', configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await released('the backgrounded phone’s control to go back');
    const c = await device('mac', 'mac-holds');
    await c.page.goto(s.base + '/workspaces');
    await c.page.locator('.tree-streams .row-main', { hasText: 'ui target' }).click();
    await c.page.waitForSelector('[data-ownership=controlling]');
    await a.page.evaluate(() => {
      Object.defineProperty(document, 'visibilityState', { value: 'visible', configurable: true });
      document.dispatchEvent(new Event('visibilitychange'));
    });
    await a.page.waitForSelector('[data-ownership=watching]');
    assert.equal(await c.page.locator('[data-ownership=controlling]').count(), 1, 'no silent takeover');
    await c.ctx.close();
    await a.ctx.close();
  });

  test('after a relaunch, a restored stream attaches directly', async () => {
    const a = await device('iphone', 'phone-relaunch');
    await openTarget(a.page);
    await a.page.reload();
    await a.page.waitForSelector('[data-ownership=controlling]');
    assert.match(a.page.url(), /\/stream\//);
    assert.equal(await a.page.getByRole('button', { name: 'Resume' }).count(), 0);
    await a.ctx.close();
  });
});

describe('agents list, peek, banners and links', () => {
  test('Peek is read-only and never resizes the Mac’s pane', async () => {
    herdr('pane', 'report-agent', ws.pane, '--source', 'custom:briardr-ui', '--agent', 'claude', '--state', 'working');
    await sleep(500);
    const rowsBefore = herdr('pane', 'get', ws.pane).result.pane.scroll?.viewport_rows;
    const a = await device('iphone', 'phone-peek');
    await a.page.locator('.stream-row', { hasText: 'ui target' }).getByRole('button', { name: /More actions/ }).click();
    await a.page.getByRole('menuitem', { name: /Peek/ }).click();
    await a.page.getByText('Watching live · read-only').waitFor();
    assert.equal(herdr('pane', 'get', ws.pane).result.pane.scroll?.viewport_rows, rowsBefore);
    await a.page.getByRole('button', { name: 'Close' }).click();
    await a.ctx.close();
  });

  test('a stream that starts needing you shows an in-app banner when it isn’t on screen', async () => {
    const a = await device('iphone', 'phone-banner');
    await a.page.waitForSelector('.count-strip');
    herdr('pane', 'report-agent', ws.pane, '--source', 'custom:briardr-ui', '--agent', 'claude', '--state', 'blocked');
    await a.page.locator('.banner-attention', { hasText: 'ui target needs you' }).waitFor();
    await a.page.locator('.grp-attention .row-title', { hasText: 'ui target' }).waitFor();
    herdr('pane', 'release-agent', ws.pane, '--source', 'custom:briardr-ui', '--agent', 'claude');
    await a.ctx.close();
  });

  test('OSC 8 hyperlinks keep their real destination; soft-wrapped URLs are joined; text in a link’s place isn’t one', async () => {
    const opened: string[] = [];
    const a = await device('mac', 'mac-osc8');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await a.page.exposeFunction('__briardrOpened', (u: string) => opened.push(u));
    await a.page.addInitScript(() => {
      window.open = ((u: string) => ((window as any).__briardrOpened(String(u)), null)) as any;
    });
    await a.page.reload();
    await a.page.waitForSelector('.count-strip');
    await openTarget(a.page);
    await a.page.evaluate(() => (window.open = ((u: string) => ((window as any).__briardrOpened(String(u)), null)) as any));
    const long = 'https://example.com/' + 'x'.repeat(150) + '/end';
    herdr('pane', 'run', ws.pane, `clear; printf '\\e]8;;https://real.example.org/target\\e\\\\click-here\\e]8;;\\e\\\\\\n'; echo ${long}`);
    // The command's own line is gone once the output shows (it clears the screen first).
    const p1 = await cellOf(a.page, 'click-here');
    await a.page.mouse.move(p1.x, p1.y);
    await sleep(200);
    await a.page.mouse.click(p1.x, p1.y);
    await sleep(400);
    assert.deepEqual(opened, ['https://real.example.org/target'], 'opened the hyperlink’s real destination');
    // Click on the wrapped continuation of the long URL: the whole URL opens.
    const p2 = await cellOf(a.page, 'xxxx/end');
    await a.page.mouse.move(p2.x, p2.y);
    await sleep(200);
    await a.page.mouse.click(p2.x, p2.y);
    await sleep(400);
    assert.equal(opened[1], long);
    // Text that takes the link's place isn't a link (xterm keeps the old link data in the line).
    herdr('pane', 'run', ws.pane, 'clear; echo plain words');
    const p3 = await cellOf(a.page, 'plain');
    await a.page.mouse.click(p3.x, p3.y, { button: 'right' });
    await a.page.getByRole('menuitem', { name: 'Select Text…' }).waitFor();
    assert.equal(await a.page.getByRole('menuitem', { name: 'Open Link' }).count(), 0);
    await a.page.keyboard.press('Escape');
  });

  test('Select Text opens a frozen copy with the touched word selected, and sends nothing', async () => {
    herdr('pane', 'send-keys', ws.pane, 'ctrl+]', 'ctrl+c');
    await sleep(300);
    const a = await device('mac', 'mac-select');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    herdr('pane', 'run', ws.pane, 'clear; echo alpha bravo charlie');
    const pt = await cellOf(a.page, 'bravo');
    await sleep(500); // the prompt after it
    const before = await paneText('recent', 5);
    await a.page.mouse.click(pt.x, pt.y, { button: 'right' });
    await a.page.getByRole('menuitem', { name: 'Select Text…' }).click();
    const area = a.page.getByLabel('Screen text');
    await area.waitFor();
    await a.page.waitForFunction(() => {
      const t = document.querySelector('.select-area') as HTMLTextAreaElement | null;
      return !!t && t.value.length > 0 && t.selectionEnd > t.selectionStart;
    }, undefined, { timeout: 3000 }).catch(() => {});
    const st = await area.evaluate((t: HTMLTextAreaElement) => ({ sel: t.value.slice(t.selectionStart, t.selectionEnd), start: t.selectionStart, end: t.selectionEnd, text: t.value.slice(0, 80) }));
    assert.equal(st.sel, 'bravo', JSON.stringify({ ...st, pt }));
    await a.page.getByRole('button', { name: 'Done', exact: true }).click();
    assert.equal(await paneText('recent', 5), before, 'nothing was sent to the terminal');
    await a.ctx.close();
  });

  test('preview zoom scales the page on iPhone, preserves state, and resets', async () => {
    // Says it's HTML, like a real dev server: WebKit on Linux shows a page that starts with <meta> as text.
    const dev = http.createServer((_req, res) => res.writeHead(200, { 'content-type': 'text/html' }).end('<meta name="viewport" content="width=device-width, initial-scale=1"><button style="width:100px;height:40px" onclick="this.textContent=\'Clicked\'">Try me</button>'));
    await new Promise<void>((r) => dev.listen(0, '127.0.0.1', () => r()));
    onTestFinished(() => {
      dev.closeAllConnections();
      dev.close();
    });
    const a = await device('iphone', 'preview-zoom');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    await a.page.getByRole('tab', { name: 'Preview', exact: true }).click();
    const zoom = a.page.getByRole('group', { name: 'Preview zoom' });
    assert.equal(await zoom.count(), 0, 'zoom controls appear after opening');
    await a.page.getByRole('textbox', { name: 'Port', exact: true }).fill(String((dev.address() as any).port));
    await a.page.getByRole('button', { name: 'Open', exact: true }).click();
    const content = a.page.frameLocator('.preview-frame').getByRole('button');
    await content.click();
    const original = (await content.boundingBox())!;
    const zoomOut = zoom.getByRole('button', { name: 'Zoom out preview' });
    const zoomIn = zoom.getByRole('button', { name: 'Zoom in preview' });
    const reset = zoom.getByRole('button', { name: 'Reset preview zoom to 100%' });
    await zoomOut.click();
    assert.equal(await reset.textContent(), '75%');
    assert.ok(Math.abs((await content.boundingBox())!.width - original.width * 0.75) < 1, 'preview content shrinks');
    await zoomOut.click();
    assert.equal(await zoomOut.isDisabled(), true, 'minimum zoom is 50%');
    await reset.click();
    await zoomIn.click();
    assert.equal(await reset.textContent(), '125%');
    assert.ok(Math.abs((await content.boundingBox())!.width - original.width * 1.25) < 1, 'preview content grows');
    await a.page.screenshot({ path: 'test-results/preview-zoom-iphone.png' });
    for (let i = 0; i < 3; i++) await zoomIn.click();
    assert.equal(await zoomIn.isDisabled(), true, 'maximum zoom is 200%');
    const viewport = (await a.page.locator('.preview-viewport').boundingBox())!;
    const frame = (await a.page.locator('.preview-frame').boundingBox())!;
    assert.ok(Math.abs(frame.width - viewport.width) < 1 && Math.abs(frame.height - viewport.height) < 1, 'scaled frame fills the available space');
    assert.equal(await content.textContent(), 'Clicked', 'zoom keeps page state');
    await reset.click();
    assert.equal(await reset.textContent(), '100%');
    assert.ok(Math.abs((await content.boundingBox())!.width - original.width) < 1, 'reset restores original size');
    const controls = (await a.page.locator('.preview-tools').boundingBox())!;
    assert.ok(controls.x >= 0 && controls.x + controls.width <= a.page.viewportSize()!.width, 'controls fit the phone');
    await a.page.getByRole('button', { name: 'Close preview', exact: true }).click();
    await zoom.waitFor({ state: 'detached' });
  });

  test('tapping a localhost link opens it in Preview, keeping path and query', async () => {
    const dev = http.createServer((req, res) => res.writeHead(200, { 'content-type': 'text/html' }).end(`<h1>dev ${req.url}</h1>`));
    await new Promise<void>((r) => dev.listen(0, '127.0.0.1', () => r()));
    onTestFinished(() => {
      dev.closeAllConnections();
      dev.close();
    });
    const port = (dev.address() as any).port;
    const a = await device('mac', 'mac-link');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    herdr('pane', 'run', ws.pane, `clear; echo http://localhost:${port}/hello?x=1`);
    const pt = await cellOf(a.page, `localhost:${port}`, 4);
    await a.page.mouse.move(pt.x, pt.y);
    await sleep(300);
    await a.page.mouse.click(pt.x, pt.y);
    await a.page.waitForURL(/\/preview$/, { timeout: 8000 }).catch(async (e) => {
      await a.page.screenshot({ path: 'test-results/link-failure.png' });
      throw e;
    });
    const frame = a.page.frameLocator('.preview-frame');
    await frame.getByText('dev /hello?x=1').waitFor();
    await a.ctx.close();
    dev.close();
  });
});

describe('workspaces and menus', () => {
  test('a workspace expanded by default collapses with one click', async () => {
    const a = await device('mac', 'mac-tree');
    await a.page.goto(s.base + '/workspaces');
    // The test workspace; a handle, because its stream row (what finds it) goes away when collapsed.
    const item = (await a.page.locator('.tree-ws').filter({ has: a.page.locator('.tree-streams .row-main', { hasText: 'ui target' }) }).elementHandle())!;
    const toggle = (await item.$('.tree-toggle'))!;
    assert.equal(await item.getAttribute('aria-expanded'), 'true');
    await toggle.click();
    assert.equal(await item.getAttribute('aria-expanded'), 'false');
    await toggle.click();
    assert.equal(await item.getAttribute('aria-expanded'), 'true');
    await a.ctx.close();
  });

  test('a menu takes keyboard focus: arrow keys move through it, Escape closes it', async () => {
    const a = await device('mac', 'mac-menu');
    await a.page.goto(s.base + '/workspaces');
    await a.page.locator('.tree-streams .stream-row', { hasText: 'ui target' }).getByRole('button', { name: /More actions/ }).click();
    const menu = a.page.getByRole('menu');
    const items = await menu.locator('[role=menuitem]:not([disabled])').allTextContents();
    assert.ok(items.length > 1);
    const focused = () => a.page.evaluate(() => (document.activeElement?.getAttribute('role') === 'menuitem' ? document.activeElement.textContent : null));
    await a.page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'menuitem');
    assert.equal(await focused(), items[0]);
    await a.page.keyboard.press('ArrowDown');
    assert.equal(await focused(), items[1]);
    await a.page.keyboard.press('ArrowUp');
    await a.page.keyboard.press('ArrowUp');
    assert.equal(await focused(), items.at(-1));
    await a.page.keyboard.press('Escape');
    await menu.waitFor({ state: 'detached' });
    await a.ctx.close();
  });

  test('on a phone, a menu opened while typing leaves focus in the terminal; with a hardware keyboard it takes it', async () => {
    const a = await device('iphone', 'phone-menu');
    await openTarget(a.page);
    const textarea = a.page.locator('.xterm-helper-textarea');
    const insert = a.page.getByRole('button', { name: 'Insert text, image or file' });
    const activeRole = () => a.page.evaluate(() => (document.activeElement?.classList.contains('xterm-helper-textarea') ? 'terminal' : document.activeElement?.getAttribute('role')));
    // Taking focus from the terminal would close the on-screen keyboard.
    await textarea.focus();
    await insert.click();
    await a.page.getByRole('menu').waitFor();
    await sleep(100);
    assert.equal(await activeRole(), 'terminal');
    await a.page.locator('.menu-scrim').click({ position: { x: 14, y: 4 } });
    await a.page.getByRole('menu').waitFor({ state: 'detached' });
    // A key pressed while no on-screen keyboard shows means a hardware keyboard: menus take focus again.
    await textarea.focus();
    await a.page.keyboard.press('Shift');
    await insert.click();
    await a.page.waitForFunction(() => document.activeElement?.getAttribute('role') === 'menuitem');
    await a.page.keyboard.press('Escape');
    await a.page.getByRole('menu').waitFor({ state: 'detached' });
    assert.equal(await activeRole(), 'terminal', 'Escape returns focus to the terminal');
    await a.ctx.close();
  });
});

describe('the on-screen keyboard (iPhone)', () => {
  test('new-item sheets on the Workspaces screen end above the keyboard', async () => {
    const a = await device('iphone', 'phone-kb-sheets');
    await a.page.goto(s.base + '/workspaces');
    await a.page.getByRole('button', { name: /^(New )?workspace$/i }).click();
    const sheet = a.page.getByRole('dialog', { name: 'New workspace' });
    await showKeyboard(a.page, 300);
    await aboveKeyboard(a.page, 300, { Name: sheet.getByLabel('Name'), Folder: sheet.getByLabel(/^Folder/), Create: sheet.getByRole('button', { name: 'Create' }) });
    // Without the keyboard, the sheet goes back to the bottom of the screen.
    await showKeyboard(a.page, 0);
    const b = (await sheet.boundingBox())!;
    assert.equal(Math.round(b.y + b.height), a.page.viewportSize()!.height);
    await a.ctx.close();
  });

  test('pairing: the screen ends above the keyboard and the focused field scrolls into view', async () => {
    // A short screen (an iPhone on its side), so the card doesn't fit above the keyboard.
    const ctx = await browser.newContext({ ...devices['iPhone 15 Pro landscape'], ignoreHTTPSErrors: true, locale: 'en-US' });
    const page = await ctx.newPage();
    await page.goto(s.base + '/');
    const code = page.getByLabel('Pairing code');
    await code.waitFor();
    await showKeyboard(page, 160);
    const pair = page.locator('.pair');
    const box = (await pair.boundingBox())!;
    assert.equal(Math.round(box.y + box.height), page.viewportSize()!.height - 160);
    // The focused code field is above the keyboard: where the text above it is taller (Linux's fonts),
    // the screen scrolls to it.
    await aboveKeyboard(page, 160, { 'pairing code': code });
    // Scrolled to its top, the card starts in view rather than being cut off above it.
    await pair.evaluate((el) => (el.scrollTop = 0));
    const heading = (await page.getByRole('heading', { name: 'Briardr' }).boundingBox())!;
    assert.ok(heading.y >= 0, `the heading starts at ${heading.y}, above the screen`);
    await page.getByLabel('Name for this device').focus();
    await sleep(100);
    await aboveKeyboard(page, 160, { 'device name': page.getByLabel('Name for this device') });
    await ctx.close();
  });

  test('a field low on a scrolling screen scrolls above the keyboard when it opens', async () => {
    const ctx = await browser.newContext({ ...devices['iPhone 15 Pro landscape'], ignoreHTTPSErrors: true, locale: 'en-US' });
    const p = s.run.auth.pair(s.run.auth.createPairingCode().code, 'phone-kb-settings');
    if (!p.ok) throw new Error('pairing failed');
    await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    const page = await ctx.newPage();
    await page.goto(s.base + '/settings');
    const field = page.getByLabel(/^Path to the herdr executable/);
    await field.focus();
    const top = page.viewportSize()!.height - 160;
    assert.ok((await field.boundingBox())!.y + 44 > top, 'the field starts under where the keyboard opens');
    await showKeyboard(page, 160);
    await aboveKeyboard(page, 160, { 'Herdr path': field });
    await ctx.close();
  });
});

describe('settings', () => {
  test('About shows the build ID, branch and commit of the running page', async () => {
    const a = await device('mac', 'mac-settings');
    await a.page.goto(s.base + '/settings');
    const meta = (name: string) => a.page.locator(`meta[name="${name}"]`).getAttribute('content');
    const [build, branch, commit] = [await meta('briardr-version'), await meta('briardr-branch'), await meta('briardr-commit')];
    assert.match(build!, /^[0-9a-f]{12}$/);
    assert.ok(branch && !branch.includes('%'), `branch ${branch}`);
    assert.match(commit!, /^[0-9a-f]{7,}(-dirty)?$|^unknown$/);
    const values = await a.page.locator('#set-about .build-info dd').allInnerTexts();
    assert.equal(values[0], build);
    assert.equal(values[1], branch);
    assert.ok(values[2].startsWith(commit!.replace(/-dirty$/, '')));
    await a.ctx.close();
  });
});

describe('notes', () => {
  test('notes autosave on the Mac, show on another device, and never overwrite silently', async () => {
    const a = await device('iphone', 'phone-notes');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    await a.page.getByRole('tab', { name: 'Notes' }).click();
    await a.page.getByRole('button', { name: 'New note' }).click();
    const area = a.page.getByLabel('Note for ui target');
    await area.fill('Next: ask Claude to add retries\nCheck the flaky test');
    await a.page.getByText(`Saved on the ${host.noun}`).waitFor();
    // A second device sees the same note as a card and opens it.
    const b = await device('mac', 'mac-notes');
    onTestFinished(() => b.ctx.close().catch(() => {}));
    await b.page.goto(s.base + `/stream/${encodeURIComponent(ws.pane)}/notes`);
    await b.page.locator('.note-card', { hasText: 'add retries' }).getByRole('button', { name: 'Edit' }).click();
    const bArea = b.page.getByLabel('Note for ui target');
    await b.page.waitForFunction(() => (document.querySelector('.notes-area') as HTMLTextAreaElement)?.value.includes('add retries'));
    // The phone edits while offline; meanwhile the Mac saves another version. When the phone is back,
    // its draft is based on an older revision: Briardr must ask instead of overwriting.
    await a.ctx.setOffline(true);
    await area.fill('Phone version');
    await a.page.getByText('Not saved yet').waitFor();
    await bArea.fill('Mac version');
    await b.page.getByText(`Saved on the ${host.noun}`).waitFor();
    await a.ctx.setOffline(false);
    await a.page.getByText('Changed on another device').first().waitFor({ timeout: 10000 });
    assert.equal(await area.inputValue(), 'Phone version', 'the phone’s text is still there while you decide');
    await a.page.getByRole('button', { name: 'Keep mine' }).click();
    await a.page.getByText(`Saved on the ${host.noun}`).waitFor();
    // After a reload the open note is still there.
    await a.page.reload(); // relaunch restores the Notes surface and the open note
    await a.page.waitForFunction(() => (document.querySelector('.notes-area') as HTMLTextAreaElement)?.value === 'Phone version');
    // Back in the list, the Mac's card picks up the phone's version.
    await b.page.getByRole('button', { name: 'All notes' }).click();
    await b.page.locator('.note-card', { hasText: 'Phone version' }).waitFor();
  });

  test('a pasted image is saved with the note and shown in Preview; only the editor scrolls', async () => {
    const a = await device('iphone', 'phone-notes-image');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    await a.page.getByRole('tab', { name: 'Notes' }).click();
    await a.page.getByRole('button', { name: 'New note' }).click();
    const area = a.page.getByLabel('Note for ui target');
    await a.page.waitForFunction(() => document.activeElement?.classList.contains('notes-area')); // a new note is ready to type
    await area.fill(Array.from({ length: 80 }, (_, i) => `line ${i}`).join('\n'));
    // iOS zooms the page when a focused field's text is under 16px.
    assert.ok(parseFloat(await area.evaluate((e) => getComputedStyle(e).fontSize)) >= 16);
    const scroll = await a.page.evaluate(() => {
      const s = document.querySelector('.surface.notes')!;
      const t = document.querySelector('.notes-area')!;
      return { page: s.scrollHeight - s.clientHeight, editor: t.scrollHeight - t.clientHeight };
    });
    assert.equal(scroll.page, 0, 'the surface itself never scrolls');
    assert.ok(scroll.editor > 0, 'the editor scrolls its own text');
    await area.evaluate(async (el) => {
      const c = document.createElement('canvas');
      c.width = c.height = 40;
      c.getContext('2d')!.fillRect(0, 0, 40, 40);
      const blob: Blob = await new Promise((r) => c.toBlob((b) => r(b!), 'image/png'));
      const dt = new DataTransfer();
      dt.items.add(new File([blob], 'screenshot.png', { type: 'image/png' }));
      (el as HTMLTextAreaElement).setSelectionRange(0, 0);
      el.dispatchEvent(new ClipboardEvent('paste', { clipboardData: dt, bubbles: true, cancelable: true }));
    });
    await a.page.waitForFunction(() => /^!\[screenshot\]\(note:[a-f0-9]{24}\.png\)\n/.test((document.querySelector('.notes-area') as HTMLTextAreaElement).value));
    await a.page.getByText(`Saved on the ${host.noun}`).waitFor();
    await a.page.getByRole('button', { name: 'Preview' }).click();
    const img = a.page.locator('.notes-preview img[alt="screenshot"]');
    await a.page.waitForFunction(() => (document.querySelector('.notes-preview img') as HTMLImageElement | null)?.naturalWidth === 40);
    assert.match((await img.getAttribute('src'))!, /\/notes\/images\/[a-f0-9]{24}\.png$/);
    // In the list, the long note is a card cut off at a fixed height, with its image.
    await a.page.getByRole('button', { name: 'All notes' }).click();
    const card = a.page.locator('.note-card', { hasText: 'line 0' });
    await card.locator('img[alt="screenshot"]').waitFor();
    const body = await card.locator('.note-card-body').evaluate((e) => ({ h: e.clientHeight, full: e.scrollHeight, clipped: e.classList.contains('is-clipped') }));
    assert.ok(body.h <= 170 && body.full > body.h && body.clipped, `card is cut off (${body.h} of ${body.full}px)`);
  });

  test('Send to terminal pastes the note once and never presses Enter; a card can be sent, copied and deleted', async () => {
    const a = await device('iphone', 'phone-notes-insert');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    await startEcho();
    onTestFinished(stopEcho);
    await a.page.getByRole('tab', { name: 'Notes' }).click();
    await a.page.getByRole('button', { name: 'New note' }).click();
    await a.page.getByLabel('Note for ui target').fill('prompt from notes');
    await a.page.getByRole('button', { name: 'Send to terminal' }).click();
    await expectBytes(hex('\x1b[200~prompt from notes\x1b[201~'));
    await sleep(300);
    assert.ok(!(await bytesSeen()).join('').endsWith('0d'), 'no Enter');
    await a.page.getByText(`Saved on the ${host.noun}`).waitFor();
    // A second note; both show as cards, newest first.
    await a.page.getByRole('button', { name: 'All notes' }).click();
    await a.page.getByRole('button', { name: 'New note' }).click();
    await a.page.getByLabel('Note for ui target').fill('second prompt');
    await a.page.getByText(`Saved on the ${host.noun}`).waitFor();
    await a.page.getByRole('button', { name: 'All notes' }).click();
    const cards = a.page.locator('.note-card');
    await cards.filter({ hasText: 'second prompt' }).waitFor();
    assert.match((await cards.first().textContent())!, /second prompt/);
    const card = cards.filter({ hasText: 'prompt from notes' });
    await card.getByRole('button', { name: 'Send to terminal' }).click();
    await expectBytes(hex('\x1b[200~prompt from notes\x1b[201~'));
    await a.ctx.grantPermissions(['clipboard-read', 'clipboard-write']).catch(() => {});
    await card.getByRole('button', { name: 'Copy' }).click();
    await a.page.getByText('Copied').waitFor({ timeout: 3000 }).catch(() => {}); // WebKit may refuse the clipboard; the toast says so
    // Delete asks first, then removes it from the Mac.
    await card.getByRole('button', { name: 'Delete note' }).click();
    await a.page.getByRole('button', { name: 'Delete', exact: true }).click();
    await card.waitFor({ state: 'detached' });
    const left = await a.page.evaluate(async (p) => (await (await fetch(`/api/streams/${encodeURIComponent(p)}/notes`)).json()).notes.map((n: any) => n.text), ws.pane);
    assert.ok(!left.includes('prompt from notes') && left.includes('second prompt'), JSON.stringify(left));
  });

  test('a note’s image is sent to the terminal as the path of a copy, on its own line, so agents attach it', async () => {
    const a = await device('iphone', 'phone-notes-image-send');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    await startEcho();
    onTestFinished(stopEcho);
    // A note with an image, saved from another device.
    const name = await a.page.evaluate(async (p) => {
      const c = document.createElement('canvas');
      c.width = c.height = 8;
      const blob: Blob = await new Promise((r) => c.toBlob((b) => r(b!), 'image/png'));
      const base = `/api/streams/${encodeURIComponent(p)}/notes`;
      const { name } = await (await fetch(`${base}/images`, { method: 'POST', body: blob })).json();
      const text = `Fix this layout:\n![screenshot](note:${name})\nthen run the tests`;
      await fetch(`${base}/e5e5e5e5e5e5e5e5`, { method: 'PUT', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ text, baseRevision: 0 }) });
      return name as string;
    }, ws.pane);
    await a.page.getByRole('tab', { name: 'Notes' }).click();
    await a.page.locator('.note-card', { hasText: 'Fix this layout' }).getByRole('button', { name: 'Send to terminal' }).click();
    let m: RegExpMatchArray | undefined;
    for (let i = 0; i < 40 && !m; i++) {
      await sleep(100);
      const all = Buffer.from((await bytesSeen()).join(''), 'hex').toString('latin1');
      m = [...all.matchAll(/\x1b\[200~(.*?)\x1b\[201~/gs)].filter((x) => x[1].includes('Fix this layout')).pop();
    }
    assert.ok(m, 'the note went as one bracketed paste');
    const [first, p, last] = m![1].split('\n');
    assert.deepEqual([first, last], ['Fix this layout:', 'then run the tests']);
    assert.match(p, new RegExp(`briardr-uploads/[0-9a-f]{24}/${name}$`), 'the image is a bare path on its own line');
    const fs = await import('node:fs');
    assert.equal(fs.readFileSync(p).toString('latin1', 1, 4), 'PNG');
    await sleep(300);
    assert.ok(!(await bytesSeen()).join('').endsWith('0d'), 'no Enter');
    fs.rmSync(path.dirname(p), { recursive: true });
  });
});

describe('Files prompt', () => {
  test('lines picked in two files go to the terminal with a question, as one paste and without Enter', async () => {
    const fs = await import('node:fs');
    fs.mkdirSync(path.join(ws.dir, 'src'), { recursive: true });
    fs.writeFileSync(path.join(ws.dir, 'src', 'a.ts'), 'const one = 1;\nconst two = 2;\nconst three = 3;\nconst four = 4;\nconst five = 5;\n');
    fs.writeFileSync(path.join(ws.dir, 'notes.md'), 'Some text with ``` a fence\nsecond line\n');
    const a = await device('iphone', 'phone-files-prompt');
    onTestFinished(() => a.ctx.close().catch(() => {}));
    await openTarget(a.page);
    await startEcho();
    onTestFinished(stopEcho);
    await a.page.getByRole('tab', { name: 'Files' }).click();
    await a.page.getByRole('button', { name: 'src, folder' }).click();
    await a.page.getByRole('button', { name: 'a.ts', exact: true }).click();
    const line = (n: number) => a.page.locator('.code .cl').nth(n - 1);
    await line(1).waitFor();
    // Tapping a number picks that line, a second number makes a range; tapping the code doesn't.
    await line(4).click({ position: { x: 14, y: 4 } });
    await a.page.locator('.fv-sel-label', { hasText: 'Line 4' }).waitFor();
    await line(2).click({ position: { x: 14, y: 4 } });
    await a.page.locator('.fv-sel-label', { hasText: 'Lines 2–4' }).waitFor();
    await line(5).locator('.lc').click();
    await a.page.locator('.fv-sel-label', { hasText: 'Lines 2–4' }).waitFor();
    await a.page.getByRole('button', { name: 'Add to prompt' }).click();
    await a.page.getByRole('region', { name: 'Prompt' }).getByText('1 snippet from 1 file').waitFor();
    assert.equal(await a.page.locator('.code .cl.is-added').count(), 3);
    // Selecting text picks the lines it touches.
    await a.page.getByRole('button', { name: 'Back to folder' }).click();
    await a.page.getByRole('button', { name: 'notes.md', exact: true }).click();
    await line(2).waitFor();
    await a.page.evaluate(() => {
      const lines = document.querySelectorAll('.code .lc');
      const text = (el: Element) => document.createTreeWalker(el, NodeFilter.SHOW_TEXT).nextNode()!;
      const r = document.createRange();
      r.setStart(text(lines[0]), 2);
      r.setEnd(text(lines[1]), 3);
      document.getSelection()!.removeAllRanges();
      document.getSelection()!.addRange(r);
    });
    await a.page.locator('.fv-sel-label', { hasText: 'Lines 1–2' }).waitFor();
    await a.page.getByRole('button', { name: 'Add & ask…' }).click();
    await a.page.getByLabel('Question or instructions').fill('Why are these here?');
    await a.page.getByRole('button', { name: 'Send to terminal' }).click();
    const expected = 'Why are these here?\n\nsrc/a.ts 2-4\n```ts\nconst two = 2;\nconst three = 3;\nconst four = 4;\n```\n\nnotes.md 1-2\n````md\nSome text with ``` a fence\nsecond line\n````';
    await expectBytes(hex(`\x1b[200~${expected}\x1b[201~`));
    await sleep(300);
    assert.ok(!(await bytesSeen()).join('').endsWith('0d'), 'no Enter');
    // Sending clears the prompt and shows the terminal, where Enter is pressed.
    await a.page.waitForSelector('[role=tab][aria-selected=true]:has-text("Terminal")');
    const stored = await a.page.evaluate(() => localStorage.getItem('briardr.files.prompt'));
    assert.equal(stored, '{}');
  });
});
