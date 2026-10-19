// Safety rule: secrets (and other sensitive details) never appear in logs.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { closeWorkspace, closed, readPane, request, sleep, startServer, testPane, until, ws, type TestServer } from './helpers.ts';

let s: TestServer;
let pane: string;
let workspace: string;
const secrets: string[] = [];

beforeAll(async () => {
  s = await startServer({ pair: false });
  ({ pane, workspace } = testPane('briardr-test-diag'));
  await s.run.herdr.refresh();
});
afterAll(async () => {
  closeWorkspace(workspace);
  await s.close();
});

function allLogs(): string {
  const dir = path.join(s.dataDir, 'logs');
  return fs.readdirSync(dir).map((f) => fs.readFileSync(path.join(dir, f), 'utf8')).join('\n');
}

describe('diagnostics privacy', () => {
  test('real flows leave no codes, tokens, names, paths, addresses or terminal text in the logs', async () => {
    // Pair (records the attempt) with a recognizable device name.
    const { code } = s.run.auth.createPairingCode();
    secrets.push(code, code.replace('-', ''));
    const bad = await request(s, 'POST', '/api/pair', { code: 'WXYZ-WXYZ', name: 'Alice’s iPhone' }, { cookie: false });
    assert.equal(bad.status, 400);
    const ok = await request(s, 'POST', '/api/pair', { code, name: 'Alice’s iPhone' }, { cookie: false });
    s.cookie = String(ok.headers['set-cookie'][0]).split(';')[0];
    const token = s.cookie.split('=')[1];
    secrets.push(token, token.split('.')[1], 'Alice');
    // Terminal with distinctive content.
    const marker = `SECRET-OUTPUT-${Date.now()}`;
    secrets.push(marker);
    const t = ws(s, `/ws/term?v=1&pane=${encodeURIComponent(pane)}&mode=control&cols=80&rows=24&session=d1`);
    await until(t, (m) => m.t === 'live');
    t.send(JSON.stringify({ t: 'input', id: 1, text: `echo ${marker}\r` }));
    await until(t, (m) => m.t === 'input_ok');
    await sleep(500);
    assert.ok(readPane(pane).includes(marker));
    t.close();
    await closed(t);
    // An upload with a revealing file name.
    const png = fs.readFileSync(path.resolve('web/public/icons/icon-192.png'));
    const up = await request(s, 'POST', `/api/streams/${encodeURIComponent(pane)}/uploads?kind=image&name=${encodeURIComponent('bank-statement-alice.png')}`, undefined, { raw: png });
    assert.equal(up.status, 200);
    secrets.push('bank-statement', up.json.path);
    // A client trying to log sensitive values.
    await request(s, 'POST', '/api/diagnostics/client', { events: [{ ev: 'live_open', url: 'https://100.64.0.9:7443/?token=abc', user: os.userInfo().username, cmd: 'rm -rf ~/x', n: 3 }] });
    await sleep(300);
    const logs = allLogs();
    assert.ok(logs.length > 200, 'logs were written');
    const forbidden = [...secrets, os.userInfo().username, os.homedir(), s.dataDir, '127.0.0.1', '100.64.0.9', 'localhost', 'rm -rf', 'token=abc'];
    for (const f of forbidden) assert.ok(!logs.includes(f), `log must not contain ${JSON.stringify(f).slice(0, 40)}`);
    assert.match(logs, /"ev":"live_open".*"url":"redacted"/);
    assert.match(logs, /"ev":"term_live"/);
    assert.match(logs, /"stream":"a[0-9a-f]{10}"/, 'stream IDs are anonymized');
  });

  test('client diagnostics apply the same rules', async () => {
    const store = new Map<string, string>();
    (globalThis as any).localStorage = { getItem: (k: string) => store.get(k) ?? null, setItem: (k: string, v: string) => store.set(k, v) };
    const mod = await import('../web/src/diag.ts');
    mod.record('term_drop', { code: 1006, reason: 'https://evil/?t=x', net: 'wifi' });
    const out = mod.exportClientLog();
    assert.match(out, /"reason":"redacted"/);
    assert.match(out, /"net":"wifi"/);
    assert.match(String(mod.anon('w1:p1')), /^a[0-9a-f]{8}$/);
  });
});
