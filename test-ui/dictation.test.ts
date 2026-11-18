// Dictation, held on the key bar's Enter, in WebKit at iPhone and iPad sizes: what happens when the key or
// the terminal goes away mid-hold. The server says it's a Mac (it runs as one on Linux too: no test here
// gets as far as the host transcribing), and the microphone is a stand-in whose tracks show whether
// Briardr let go of it.
import assert from 'node:assert/strict';
import { afterAll, beforeAll, describe, onTestFinished, test } from 'bun:test';
import { devices, webkit, type Browser, type Page } from 'playwright';
import { closeWorkspace, herdr, sleep, startServer, testPane, type TestServer } from '../test/helpers.ts';

let s: TestServer;
let browser: Browser;
let ws: { pane: string; workspace: string };

beforeAll(async () => {
  s = await startServer({ pair: false, platform: 'macos' });
  ws = testPane('briardr-dictation-ui');
  herdr('pane', 'rename', ws.pane, 'dictation target');
  await s.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  closeWorkspace(ws.workspace);
  await s.close();
});

/**
 * A paired device controlling the stream, with a stand-in microphone (a tone from the page's own audio),
 * there at once or, with `late`, only when the test calls `giveMic()`, as after a permission prompt.
 * `sent` collects what reached the host: transcriptions asked for, and input for the terminal.
 */
async function onStream(profile: 'iPhone 15 Pro' | 'iPad Pro 11', name: string, opts: { late?: boolean; transcribe?: Promise<string> } = {}) {
  // A test that answers the transcription itself (page.route) needs the service worker out of the way.
  const ctx = await browser.newContext({ ...devices[profile], ignoreHTTPSErrors: true, locale: 'en-US', serviceWorkers: opts.transcribe ? 'block' : 'allow' });
  onTestFinished(() => ctx.close().catch(() => {}));
  const p = s.run.auth.pair(s.run.auth.createPairingCode().code, name);
  if (!p.ok) throw new Error('pairing failed');
  await ctx.addCookies([{ name: '__Host-briardr', value: p.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
  await ctx.addInitScript((late) => {
    const w = window as any;
    w.mics = [];
    // On the prototype: WebKit ignores the method set on navigator.mediaDevices itself.
    MediaDevices.prototype.getUserMedia = async () => {
      w.micAsked = true;
      if (late) await new Promise((r) => (w.giveMic = r));
      const audio = new AudioContext();
      const out = audio.createMediaStreamDestination();
      audio.createOscillator().connect(out);
      w.mics.push(out.stream);
      return out.stream;
    };
  }, !!opts.late);
  const page = await ctx.newPage();
  const sent: string[] = [];
  page.on('request', (r) => r.url().includes('/api/dictation') && sent.push('transcription'));
  page.on('websocket', (sock) => sock.on('framesent', ({ payload }) => typeof payload === 'string' && payload.includes('"t":"input"') && sent.push(payload)));
  const text = opts.transcribe;
  if (text) await page.route('**/api/dictation*', async (route) => route.fulfill({ json: { text: await text } }).catch(() => {}));
  await page.goto(s.base + '/workspaces');
  await page.locator('.tree-streams .row-main', { hasText: 'dictation target' }).click();
  await page.waitForSelector('[data-ownership=controlling]', { timeout: 10_000 });
  return { page, sent };
}

/** Puts the mouse down on the key bar's Enter and keeps it there. */
async function holdEnter(page: Page) {
  const box = (await page.getByRole('button', { name: 'Enter. Hold to dictate.' }).boundingBox())!;
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
}

/** For each microphone the page was given: whether it has been let go of. */
const micsOff = (page: Page) => page.evaluate(() => (window as any).mics.map((m: MediaStream) => m.getTracks().every((t) => t.readyState === 'ended')));

/** Leaves the stream, as the back gesture does. */
async function leave(page: Page) {
  await page.evaluate(() => history.back());
  await page.locator('.term-surface').waitFor({ state: 'detached' });
}

describe('dictation when the key or the terminal goes away', () => {
  test('leaving the stream while Enter is held drops the recording, lets go of the microphone and sends nothing', async () => {
    const { page, sent } = await onStream('iPhone 15 Pro', 'phone-dictation-leave');
    await holdEnter(page);
    await page.locator('.key-enter.is-dictating').waitFor();
    await leave(page);
    await page.mouse.up();
    assert.deepEqual(await micsOff(page), [true]);
    await sleep(500);
    assert.deepEqual(sent, [], 'nothing transcribed or typed');
  });

  test('a microphone that comes after the stream was left is let go of at once', async () => {
    const { page, sent } = await onStream('iPhone 15 Pro', 'phone-dictation-late', { late: true });
    await holdEnter(page);
    await page.waitForFunction(() => (window as any).micAsked);
    await leave(page);
    await page.mouse.up();
    await page.evaluate(() => (window as any).giveMic());
    await page.waitForFunction(() => (window as any).mics.length === 1);
    assert.deepEqual(await micsOff(page), [true]);
    await sleep(500);
    assert.deepEqual(sent, []);
  });

  test('the key bar going away while Enter is held drops the recording too', async () => {
    // On an iPad, a key typed on a hardware keyboard hides the key bar.
    const { page, sent } = await onStream('iPad Pro 11', 'ipad-dictation-keyboard');
    await holdEnter(page);
    await page.locator('.key-enter.is-dictating').waitFor();
    await page.locator('.term-screen textarea').focus();
    await page.keyboard.press('Shift');
    await page.locator('.keybar').waitFor({ state: 'detached' });
    await page.mouse.up();
    assert.deepEqual(await micsOff(page), [true]);
    await sleep(500);
    assert.deepEqual(sent, []);
  });

  test('a transcription that comes back after the stream was left is never typed', async () => {
    let answer!: (text: string) => void;
    const { page, sent } = await onStream('iPhone 15 Pro', 'phone-dictation-transcribing', { transcribe: new Promise((r) => (answer = r)) });
    await holdEnter(page);
    await page.locator('.key-enter.is-dictating').waitFor();
    await page.mouse.up();
    await page.waitForFunction(() => (window as any).mics[0]?.getTracks()[0].readyState === 'ended');
    assert.deepEqual(sent, ['transcription']);
    await leave(page);
    answer('words said before leaving');
    await sleep(500);
    assert.deepEqual(sent, ['transcription'], 'nothing typed');
    assert.equal(await page.locator('.toast').count(), 0, 'and no message about it');
  });
});
