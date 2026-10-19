// Functional check in real Mobile Safari (iOS Simulator), driven through safaridriver.
// Prerequisites: a booted simulator that trusts .dev-data's CA (xcrun simctl keychain booted
// add-root-cert .dev-data/secrets/tls/ca.crt), `safaridriver -p 4444`, and a Briardr server on
// :17443 with BRIARDR_DATA_DIR=.dev-data and BRIARDR_WEB_DIST pointing at the build to check.
//   SIM_UDID=<udid> bun bench/simulator.ts
// Clicks are scripted (simulated taps select text in Safari); revoke the paired device afterwards.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
const UDID = process.env.SIM_UDID || execFileSync('xcrun', ['simctl', 'list', 'devices', 'booted'], { encoding: 'utf8' }).match(/\(([0-9A-F-]{36})\) \(Booted\)/)![1];
const WD = 'http://localhost:4444';
const BASE = 'https://localhost:17443';
const OUT = 'bench/results/simulator';
fs.mkdirSync(OUT, { recursive: true });
const wd = async (method: string, path: string, body?: unknown) => {
  const r = await fetch(WD + path, { method, headers: { 'content-type': 'application/json' }, body: body === undefined ? undefined : JSON.stringify(body) });
  const j: any = await r.json();
  if (j.value?.error) throw new Error(`${path}: ${j.value.error} ${j.value.message}`);
  return j.value;
};
const s = await wd('POST', '/session', { capabilities: { alwaysMatch: { browserName: 'safari', platformName: 'iOS', 'safari:useSimulator': true, 'safari:deviceUDID': UDID } } });
const sid = s.sessionId;
const run = (script: string, args: unknown[] = []) => wd('POST', `/session/${sid}/execute/sync`, { script, args });
const runAsync = (script: string, args: unknown[] = []) => wd('POST', `/session/${sid}/execute/async`, { script, args });
const go = (url: string) => wd('POST', `/session/${sid}/url`, { url });
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
const until = async (what: string, js: string, ms = 15000) => {
  for (const t0 = Date.now(); ; await sleep(250)) {
    if (await run(`return !!(${js})`)) return;
    if (Date.now() - t0 > ms) throw new Error('timed out: ' + what);
  }
};
const shot = (name: string) => execFileSync('xcrun', ['simctl', 'io', UDID, 'screenshot', `${OUT}/${name}.png`], { stdio: 'ignore' });
const results: Record<string, unknown> = {};
try {
  await go(BASE + '/');
  await until('app loaded', `document.querySelector('meta[name="briardr-version"]')`);
  results.version = await run(`return document.querySelector('meta[name="briardr-version"]').content`);
  // Bytes sent for the app bundle (compressed when the server sends the build's .br/.gz copy).
  results.bundleTransfer = await run(`const e = performance.getEntriesByType('resource').find((r) => /\\/assets\\/main\\..*\\.js$/.test(r.name)); return e && { encoded: e.encodedBodySize, decoded: e.decodedBodySize }`);
  const code = execFileSync(path.resolve('server/target/release/briardr'), ['pair'], { env: { ...process.env, BRIARDR_DATA_DIR: '.dev-data' }, encoding: 'utf8' }).match(/[A-Z0-9]{4}-[A-Z0-9]{4}/)![0];
  const paired = await runAsync(`const done = arguments[arguments.length - 1]; fetch('/api/pair', { method: 'POST', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ code: arguments[0], name: 'Simulator (Svelte check)' }) }).then((r) => done(r.status), (e) => done(String(e)));`, [code]);
  results.paired = paired;
  await go(BASE + '/agents');
  await until('agents', `document.querySelector('.count-strip')`);
  await sleep(800);
  shot('1-agents');
  await go(BASE + '/workspaces');
  await until('tree', `document.querySelector('.tree-streams .row-main')`);
  shot('2-workspaces');
  // Open "refactor auth" from the tree (a JS click: simulated taps select text instead).
  await run(`[...document.querySelectorAll('.tree-streams .row-main')].find((b) => b.textContent.includes('refactor auth')).click()`);
  await until('terminal live', `document.querySelector('.conn.conn-controlling, [data-ownership=controlling]')`);
  await sleep(1200);
  results.terminal = await run(`const c = document.querySelector('.term-host').briardrTerminal; return { cols: c.term.cols, rows: c.term.rows, webgl: !!document.querySelector('.xterm canvas'), line: c.term.buffer.active.getLine(0).translateToString(true).slice(0, 40) }`);
  shot('3-terminal');
  await run(`document.querySelector('[aria-label="Insert text, image or file"]').click()`);
  await until('insert menu', `document.querySelector('[role=menu]')`);
  await sleep(300);
  // Without a hardware keyboard a focused text field keeps focus (the on-screen keyboard stays up);
  // otherwise the first item takes it.
  results.insertMenuFocus = await run(`const a = document.activeElement; return a ? a.tagName.toLowerCase() + (a.getAttribute('role') ? '[role=' + a.getAttribute('role') + ']' : '') : null`);
  shot('4-insert-menu');
  await run(`document.querySelector('.menu-scrim').click()`);
  await run(`[...document.querySelectorAll('[role=tab]')].find((b) => b.textContent.includes('Notes')).click()`);
  await until('notes', `document.querySelector('.notes-list')`);
  await run(`[...document.querySelectorAll('button')].find((b) => b.textContent.includes('New note')).click()`);
  await until('editor', `document.querySelector('.notes-area')`);
  results.noteFieldFontPx = await run(`return parseFloat(getComputedStyle(document.querySelector('.notes-area')).fontSize)`);
  results.noteFocused = await run(`return document.activeElement?.classList.contains('notes-area')`);
  shot('5-note-editor');
  await run(`document.querySelector('.notes-back').click()`);
  await run(`[...document.querySelectorAll('[role=tab]')].find((b) => b.textContent.includes('Changes')).click()`);
  await until('changes', `document.querySelector('.surface.changes .surface-head')`);
  await sleep(800);
  shot('6-changes');
  await run(`document.querySelector('.stream-head [aria-label="Jump to a stream"]').click()`);
  await until('jump', `document.querySelector('.sheet.jump input')`);
  results.jumpFieldFontPx = await run(`return parseFloat(getComputedStyle(document.querySelector('.sheet.jump input')).fontSize)`);
  await sleep(500);
  shot('7-jump');
  results.pageErrors = await run(`return window.__errors || []`);
  // Larger accessibility text sizes keep form fields at 16 px or more (no focus zoom).
  execFileSync('xcrun', ['simctl', 'ui', UDID, 'content_size', 'extra-small']);
  await sleep(800);
  results.jumpFieldFontPxExtraSmall = await run(`return parseFloat(getComputedStyle(document.querySelector('.sheet.jump input')).fontSize)`);
  execFileSync('xcrun', ['simctl', 'ui', UDID, 'content_size', 'large']);
  console.log(JSON.stringify(results, null, 1));
  fs.writeFileSync(`${OUT}/results.json`, JSON.stringify({ checkedAt: new Date().toISOString(), device: UDID, ...results }, null, 1) + '\n');
} finally {
  await wd('DELETE', `/session/${sid}`).catch(() => {});
}
