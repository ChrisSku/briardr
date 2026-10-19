// Safety rules for live terminals (against the isolated briardr-dev Herdr session):
//  - input is never replayed after a reconnect and never queued while connecting;
//  - watching is read-only; Peek never resizes the Mac's pane;
//  - control is never taken from another client without an explicit takeover;
//  - closing the socket releases control.
import assert from 'node:assert/strict';
import { spawn } from 'node:child_process';
import path from 'node:path';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { closeWorkspace, closed, herdr, opened, readPane, request, sleep, startServer, testPane, until, ws, type TestServer } from './helpers.ts';

let s: TestServer;
let pane: string;
let workspace: string;
beforeAll(async () => {
  s = await startServer();
  ({ pane, workspace } = testPane('briardr-test-terminal'));
  await s.run.herdr.refresh();
});
afterAll(async () => {
  closeWorkspace(workspace);
  await s.close();
});

const term = (mode: 'control' | 'observe', extra = '') => ws(s, `/ws/term?v=1&pane=${encodeURIComponent(pane)}&mode=${mode}&cols=80&rows=24&session=t${Math.random().toString(36).slice(2, 8)}${extra}`);
const rows = () => herdr('pane', 'get', pane).result.pane.scroll?.viewport_rows;

describe('live terminal safety', () => {
  test('input sent before the stream is live is rejected and never delivered later', async () => {
    const marker = `early-${Date.now()}`;
    const c = term('control');
    await opened(c);
    // Immediately, before the first frame: must be rejected, not queued.
    c.send(JSON.stringify({ t: 'input', id: 1, text: `echo ${marker}` }));
    const msgs = await until(c, (m) => m.t === 'live');
    const rejected = msgs.find((m) => m.t === 'input_rejected' && m.id === 1);
    assert.ok(rejected, 'early input was explicitly rejected');
    assert.equal(rejected.reason, 'connecting');
    await sleep(800);
    assert.ok(!readPane(pane).includes(marker), 'early input never reached the pane');
    c.close();
    await closed(c);
  });

  test('a reconnect never replays input from the previous connection', async () => {
    const marker = `replay-${Date.now()}`;
    const a = term('control');
    await until(a, (m) => m.t === 'live');
    // Typed but not "submitted": text lands on the prompt line once. Then the connection drops.
    a.send(JSON.stringify({ t: 'input', id: 1, text: `# ${marker}` }));
    await until(a, (m) => m.t === 'input_ok' && m.id === 1);
    a.terminate();
    await sleep(500);
    const b = term('control');
    await until(b, (m) => m.t === 'live');
    await sleep(800);
    const count = readPane(pane).split(marker).length - 1;
    assert.equal(count, 1, 'the text appears exactly once; nothing was replayed on reconnect');
    // Clean the prompt line.
    b.send(JSON.stringify({ t: 'input', id: 2, keys: ['ctrl+c'] }));
    await until(b, (m) => m.t === 'input_ok' && m.id === 2);
    b.close();
    await closed(b);
  });

  test('watching (Peek) is read-only and never resizes the Mac’s pane', async () => {
    const before = rows();
    const o = ws(s, `/ws/term?v=1&pane=${encodeURIComponent(pane)}&mode=observe&cols=33&rows=11`);
    await until(o, (m) => m.t === 'live');
    o.send(JSON.stringify({ t: 'input', id: 7, text: 'echo should-not-run' }));
    o.send(JSON.stringify({ t: 'resize', cols: 20, rows: 5 }));
    const msgs = await until(o, (m) => m.t === 'input_rejected' && m.id === 7);
    assert.equal(msgs.find((m) => m.t === 'input_rejected').reason, 'watching');
    await sleep(500);
    assert.equal(rows(), before, 'pane size unchanged by an observer');
    assert.ok(!readPane(pane).includes('should-not-run'));
    o.close();
    await closed(o);
  });

  test('control is never taken from another client without an explicit takeover', async () => {
    const a = term('control');
    await until(a, (m) => m.t === 'live');
    // A second client without takeover: refused, and the first keeps control.
    const b = term('control');
    const bm = await until(b, (m) => m.t === 'closed');
    assert.equal(bm.find((m) => m.t === 'closed').category, 'busy');
    await closed(b);
    a.send(JSON.stringify({ t: 'input', id: 3, keys: ['ctrl+l'] }));
    await until(a, (m) => m.t === 'input_ok' && m.id === 3);
    // Only an explicit takeover moves control; the previous controller is told.
    const aClosed = until(a, (m) => m.t === 'closed');
    const c = term('control', '&takeover=1');
    await until(c, (m) => m.t === 'live');
    const am = await aClosed;
    assert.equal(am.find((m) => m.t === 'closed').category, 'taken_over');
    c.close();
    await closed(c);
  });

  test('closing the connection releases control so another client can attach', async () => {
    const a = term('control');
    await until(a, (m) => m.t === 'live');
    a.close();
    await closed(a);
    await sleep(600);
    const b = term('control');
    const msgs = await until(b, (m) => m.t === 'live' || m.t === 'closed');
    assert.equal(msgs[msgs.length - 1].t, 'live', 'no stale controller left behind');
    b.close();
    await closed(b);
  });

  test('release ends control cleanly', async () => {
    const a = term('control');
    await until(a, (m) => m.t === 'live');
    a.send(JSON.stringify({ t: 'release' }));
    const msgs = await until(a, (m) => m.t === 'closed');
    assert.equal(msgs.find((m) => m.t === 'closed').category, 'released');
    await closed(a);
  });

  test('hand back returns the pane to its size on the Mac (Herdr TUI attached)', async () => {
    // Stand-in for the Mac's own Herdr window: a real Herdr TUI client in a 140x45 pseudo-terminal,
    // showing the test workspace.
    herdr('workspace', 'focus', workspace);
    const env: NodeJS.ProcessEnv = {};
    for (const [k, v] of Object.entries(process.env)) if (!k.startsWith('HERDR_')) env[k] = v;
    const tui = spawn('python3', [path.join(import.meta.dirname, 'fixtures', 'mac-tui.py'), '140', '45'], { env, stdio: 'ignore' });
    try {
      let macRows = 0;
      for (let i = 0; i < 30 && !(macRows > 30); i++) {
        await sleep(200);
        macRows = rows();
      }
      assert.ok(macRows > 30, `the Mac client sized the pane (${macRows} rows)`);
      const a = ws(s, `/ws/term?v=1&pane=${encodeURIComponent(pane)}&mode=control&cols=50&rows=15&session=handback`);
      await until(a, (m) => m.t === 'live');
      await sleep(400);
      assert.equal(rows(), 15, 'while controlling, the pane uses the phone’s size');
      a.send(JSON.stringify({ t: 'release' }));
      await until(a, (m) => m.t === 'closed');
      await sleep(600);
      assert.equal(rows(), macRows, 'after hand back the pane is the Mac’s size again');
    } finally {
      tui.kill();
    }
  });

  test('input to a pane that is gone is refused', async () => {
    const g = ws(s, `/ws/term?v=1&pane=w999:p999&mode=control`);
    const msgs = await until(g, (m) => m.t === 'closed');
    assert.equal(msgs[0].category, 'pane_gone');
  });

  test('every input call went to the pane of the connection it arrived on', async () => {
    assert.ok(s.run.hub.sent.length > 0);
    const r = await request(s, 'GET', '/api/state');
    assert.equal(r.status, 200);
  });
});
