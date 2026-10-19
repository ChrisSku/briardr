// Smoke test for a running Briardr deployment, through the origin phones use (public access through
// Caddy, or WireGuard straight to Briardr), the way a phone uses it: pair, live updates, a terminal (input
// and resize), uploads, notes, changes, docs, a preview with its WebSocket, and optionally a Briardr-only
// restart that must leave Herdr's panes running. It finds out which mode it's talking to from Briardr.
//
// Run it on the server as the user Briardr runs as, from the checkout:
//   mise exec -- bun scripts/smoke-deployment.ts --origin https://briardr.example.com [--cacert FILE] [--restart]
//   mise exec -- bun scripts/smoke-deployment.ts --origin https://10.8.0.1:7443 --cacert ~/.local/share/briardr/secrets/tls/ca.crt
// --no-preview skips the preview, whose throwaway dev server must run where Briardr runs (not, say, in a
// network namespace that plays the phone).
// It creates a pairing code for itself, a workspace named "briardr-smoke" in a temporary folder, and a
// preview of a throwaway server; it removes all three (and revokes its device) when it's done.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';

const args = process.argv.slice(2);
const opt = (name: string) => {
  const i = args.indexOf(name);
  return i >= 0 ? args[i + 1] : undefined;
};
const ORIGIN = opt('--origin') || '';
if (!ORIGIN) {
  console.error('Usage: bun scripts/smoke-deployment.ts --origin https://briardr.example.com [--cacert FILE] [--restart]');
  process.exit(2);
}
const ca = opt('--cacert') ? fs.readFileSync(opt('--cacert')!, 'utf8') : undefined;
const tls = ca ? { ca } : undefined;
const repo = path.resolve(import.meta.dir, '..');
/** The server binary: BRIARDR_BIN, else where scripts/setup-linux.sh builds it, else this checkout's build. */
const briardrBin =
  [process.env.BRIARDR_BIN, path.join(process.env.XDG_STATE_HOME || path.join(os.homedir(), '.local', 'state'), 'briardr-setup', 'cargo-target', 'release', 'briardr'), path.join(repo, 'server', 'target', 'release', 'briardr')].find(
    (p) => p && fs.existsSync(p),
  ) || 'briardr';
let cookie = '';
let failures = 0;
const cleanups: (() => Promise<void> | void)[] = [];

function check(ok: unknown, what: string, detail = '') {
  console.log(`${ok ? '  ✓' : '  ✗'} ${what}${!ok && detail ? ` — ${detail}` : ''}`);
  if (!ok) failures++;
  return !!ok;
}

async function api(method: string, p: string, body?: unknown, o: { raw?: Uint8Array; cookie?: string | null } = {}) {
  const headers: Record<string, string> = { origin: ORIGIN! };
  const c = o.cookie === undefined ? cookie : o.cookie;
  if (c) headers.cookie = c;
  if (body !== undefined) headers['content-type'] = 'application/json';
  const res = await fetch(ORIGIN + p, { method, headers, body: o.raw ?? (body === undefined ? undefined : JSON.stringify(body)), tls, redirect: 'manual' } as RequestInit);
  const text = await res.text();
  let json: any = null;
  try {
    json = JSON.parse(text);
  } catch {}
  return { status: res.status, json, text, headers: res.headers };
}

function socket(url: string, headers: Record<string, string>): Promise<{ ws: WebSocket; next: (pred: (m: any) => boolean, ms?: number) => Promise<any> }> {
  const ws = new WebSocket(url, { headers, tls } as any);
  const seen: any[] = [];
  const waiters: { pred: (m: any) => boolean; resolve: (m: any) => void }[] = [];
  ws.addEventListener('message', (e) => {
    let m: any = { t: 'frame' };
    if (typeof e.data === 'string')
      try {
        m = JSON.parse(e.data);
      } catch {
        m = { t: 'text', data: e.data };
      }
    seen.push(m);
    for (const w of [...waiters]) if (w.pred(m)) (waiters.splice(waiters.indexOf(w), 1), w.resolve(m));
  });
  const next = (pred: (m: any) => boolean, ms = 10_000) =>
    new Promise<any>((resolve, reject) => {
      const hit = seen.find(pred);
      if (hit) return resolve(hit);
      const t = setTimeout(() => reject(new Error(`timeout; saw ${JSON.stringify(seen.slice(-3))}`)), ms);
      waiters.push({ pred, resolve: (m) => (clearTimeout(t), resolve(m)) });
    });
  return new Promise((resolve, reject) => {
    ws.addEventListener('open', () => resolve({ ws, next }), { once: true });
    ws.addEventListener('error', () => reject(new Error(`WebSocket to ${url} failed`)), { once: true });
  });
}

const wss = ORIGIN.replace(/^https:/, 'wss:');
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));

async function main() {
  console.log(`Briardr smoke test against ${ORIGIN}`);

  // ---------- pairing ----------
  console.log('\nPairing');
  const out = execFileSync(briardrBin, ['pair'], { encoding: 'utf8' });
  const code = out.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/)?.[0];
  check(out.includes(`Open ${ORIGIN} `), 'the pairing command prints the public origin');
  check((await api('GET', '/api/state', undefined, { cookie: null })).status === 401, 'nothing is readable without a device');
  const bad = await api('POST', '/api/pair', { code: 'ZZZZ-ZZZZ', name: 'smoke' }, { cookie: null });
  check(bad.status === 400 && bad.json?.error === 'invalid', 'a wrong code gets the generic answer');
  const paired = await api('POST', '/api/pair', { code, name: 'briardr-smoke' }, { cookie: null });
  const set = paired.headers.get('set-cookie') || '';
  check(paired.status === 200 && /HttpOnly/.test(set) && /Secure/.test(set) && /SameSite=Strict/.test(set), 'pairs, with a Secure HttpOnly SameSite=Strict cookie', String(paired.status));
  cookie = set.split(';')[0];
  const deviceId = paired.json?.device?.id;
  cleanups.push(async () => void (await api('DELETE', `/api/devices/${deviceId}`)));
  const session = await api('GET', '/api/session');
  check(session.json?.host?.noun === 'server' && session.json.host.dictation === false, 'the host reports itself as a server without Mac features');
  const settings = await api('GET', '/api/settings');
  const proxied = settings.json?.tls?.effective === 'proxy';
  console.log(`  · ${proxied ? 'public access: Briardr behind a reverse proxy' : `direct access: Briardr serves HTTPS itself (${settings.json?.tls?.effective})`}`);

  // ---------- live ----------
  console.log('\nLive updates');
  const live = await socket(`${wss}/ws/live?v=1`, { origin: ORIGIN, cookie });
  const model = await live.next((m) => m.t === 'model');
  check(model.herdr?.ok, `Herdr connected (${model.herdr?.version || model.herdr?.problem})`);
  const evil = await socket(`${wss}/ws/live?v=1`, { origin: 'https://evil.example', cookie }).then(
    (s) => (s.ws.close(), false),
    () => true,
  );
  check(evil, 'a WebSocket from another origin is refused');

  // ---------- work ----------
  console.log('\nTerminal');
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), 'briardr-smoke-'));
  cleanups.push(() => fs.rmSync(dir, { recursive: true, force: true }));
  execFileSync('git', ['init', '-q', dir]);
  fs.writeFileSync(path.join(dir, 'README.md'), '# Smoke\n\nHello from the smoke test.\n');
  execFileSync('git', ['-C', dir, '-c', 'user.name=smoke', '-c', 'user.email=smoke@localhost', 'add', '.']);
  execFileSync('git', ['-C', dir, '-c', 'user.name=smoke', '-c', 'user.email=smoke@localhost', 'commit', '-qm', 'init']);
  const ws = await api('POST', '/api/workspaces', { name: 'briardr-smoke', folder: dir });
  check(ws.status === 200 && ws.json?.streamId, 'created a workspace with a shell', ws.text.slice(0, 200));
  const stream = ws.json.streamId as string;
  const herdrCfg: { herdrPath: string; herdrSession: string } = JSON.parse(execFileSync(briardrBin, ['status'], { encoding: 'utf8' }));
  const herdrSession = herdrCfg.herdrSession ? ['--session', herdrCfg.herdrSession] : [];
  cleanups.push(() => void execFileSync(herdrCfg.herdrPath, [...herdrSession, 'workspace', 'close', ws.json.workspaceId], { stdio: 'ignore' }));
  await live.next((m) => m.t === 'model' && m.model?.streams?.some((s: any) => s.id === stream), 15_000);
  check(true, 'the new stream appears in live updates');
  const term = await socket(`${wss}/ws/term?v=1&pane=${encodeURIComponent(stream)}&mode=control&cols=100&rows=30&session=smoke`, { origin: ORIGIN, cookie });
  await term.next((m) => m.t === 'live', 15_000);
  const marker = `smoke-${Date.now()}`;
  term.ws.send(JSON.stringify({ t: 'input', id: 1, text: `echo ${marker}` }));
  check((await term.next((m) => m.id === 1 && (m.t === 'input_ok' || m.t === 'input_rejected'))).t === 'input_ok', 'input accepted');
  term.ws.send(JSON.stringify({ t: 'input', id: 2, keys: ['enter'] }));
  await term.next((m) => m.id === 2 && m.t === 'input_ok');
  let seen = false;
  for (let i = 0; i < 20 && !seen; i++) {
    await sleep(300);
    seen = ((await api('GET', `/api/streams/${encodeURIComponent(stream)}/history?lines=50`)).json?.text || '').split(marker).length >= 3;
  }
  check(seen, 'the command ran in the pane (seen in its history)');
  term.ws.send(JSON.stringify({ t: 'resize', cols: 60, rows: 20 }));
  await sleep(800);
  const info = await api('GET', `/api/streams/${encodeURIComponent(stream)}/info`);
  check(info.status === 200, 'stream info readable after a resize');
  term.ws.close();

  console.log('\nUploads, notes, changes, docs');
  const up = await api('POST', `/api/streams/${encodeURIComponent(stream)}/uploads?kind=file&name=hello.txt`, undefined, { raw: new TextEncoder().encode('uploaded by the smoke test\n') });
  check(up.status === 200 && fs.readFileSync(up.json.path, 'utf8') === 'uploaded by the smoke test\n', 'a file upload lands in a private folder on the server');
  const note = await api('PUT', `/api/streams/${encodeURIComponent(stream)}/notes/${crypto.randomUUID().replace(/-/g, '').slice(0, 16)}`, { text: 'Smoke note', baseRevision: 0 });
  check(note.status === 200, 'a note is saved', note.text.slice(0, 120));
  fs.appendFileSync(path.join(dir, 'README.md'), 'Changed by the smoke test.\n');
  const changes = await api('GET', `/api/streams/${encodeURIComponent(stream)}/changes`);
  check(changes.json?.files?.some((f: any) => f.path === 'README.md'), 'the change shows up for review');
  const diff = await api('GET', `/api/streams/${encodeURIComponent(stream)}/changes/diff?path=README.md`);
  check(diff.status === 200 && JSON.stringify(diff.json).includes('Changed by the smoke test'), 'its diff is readable');
  const docs = await api('GET', `/api/streams/${encodeURIComponent(stream)}/docs`);
  check(JSON.stringify(docs.json).includes('README.md'), 'docs list the README');
  const doc = await api('GET', `/api/streams/${encodeURIComponent(stream)}/docs/file?path=README.md`);
  check(doc.status === 200 && JSON.stringify(doc.json).includes('Hello from the smoke test'), 'a document is readable');

  // ---------- preview ----------
  if (args.includes('--no-preview')) console.log('\nPreview: skipped (--no-preview)');
  else await preview(proxied);

  // ---------- Briardr-only restart ----------
  if (args.includes('--restart')) {
    console.log('\nBriardr-only restart');
    live.ws.close();
    execFileSync('sudo', ['systemctl', 'restart', 'briardr']);
    let back = false;
    for (let i = 0; i < 30 && !back; i++) {
      await sleep(1000);
      back = (await api('GET', '/api/session').catch(() => ({ status: 0 }))).status === 200;
    }
    check(back, 'Briardr is back after the restart, and this device is still signed in');
    const h = await api('GET', `/api/streams/${encodeURIComponent(stream)}/history?lines=50`);
    check(h.status === 200 && h.json.text.includes(marker), 'the pane kept running in Herdr, with its output');
  } else live.ws.close();
}

async function preview(proxied: boolean) {
  console.log('\nPreview');
  const dev = Bun.serve({
    hostname: '127.0.0.1',
    port: 0,
    fetch(req, server) {
      if (new URL(req.url).pathname === '/hmr' && server.upgrade(req)) return;
      return new Response(`<p>preview ${new URL(req.url).pathname} cookie=${req.headers.get('cookie') || ''}</p>`, { headers: { 'content-type': 'text/html' } });
    },
    websocket: { message: (ws, m) => void ws.send(`echo:${m}`) },
  });
  cleanups.push(() => void dev.stop(true));
  const pv = await api('POST', '/api/previews', { port: dev.port });
  if (proxied) await proxiedPreview(pv);
  else await directPreview(pv);
}

/** Public access: the preview has its own origin and a one-time launch link; the device cookie doesn't open it. */
async function proxiedPreview(pv: Awaited<ReturnType<typeof api>>) {
  if (check(pv.status === 200 && pv.json?.preview?.launch, 'opened a preview on its own origin', pv.text.slice(0, 200))) {
    const { origin, launch, id } = pv.json.preview;
    cleanups.push(async () => void (await api('DELETE', `/api/previews/${id}`)));
    check(origin !== ORIGIN, `the preview has its own origin (${origin})`);
    const noAuth = await fetch(`${origin}/`, { headers: { cookie }, tls } as RequestInit);
    check(noAuth.status === 401, 'Briardr’s device cookie alone doesn’t open the preview');
    const l = await fetch(`${launch}&path=${encodeURIComponent('/app')}`, { redirect: 'manual', tls } as RequestInit);
    const pc = (l.headers.get('set-cookie') || '').split(';')[0];
    check(l.status === 303 && l.headers.get('location') === '/app' && pc.startsWith('__Host-briardr-preview='), 'the launch link signs in to the preview origin');
    const page = await fetch(`${origin}/app`, { headers: { cookie: `${pc}; ${cookie}; theme=dark` }, tls } as RequestInit);
    const body = await page.text();
    check(page.status === 200 && body.includes('preview /app') && body.includes('cookie=theme=dark'), 'the page loads; neither Briardr cookie reaches the dev server', body.slice(0, 120));
    try {
      const hmr = await socket(`${origin.replace(/^https:/, 'wss:')}/hmr`, { cookie: pc, origin });
      hmr.ws.send('ping');
      check((await hmr.next((m) => m.t === 'text' && m.data === 'echo:ping')).data === 'echo:ping', 'a dev-server WebSocket (HMR) works through the preview origin');
      hmr.ws.close();
    } catch (e) {
      check(false, 'a dev-server WebSocket (HMR) works through the preview origin', String(e));
    }
  }
}

/**
 * Direct access (WireGuard, LAN): the preview is on another port of Briardr's own host, with Briardr's
 * certificate, and the device cookie opens it (cookies don't depend on the port); the dev server never
 * sees that cookie.
 */
async function directPreview(pv: Awaited<ReturnType<typeof api>>) {
  if (!check(pv.status === 200 && pv.json?.preview?.listenPort, 'opened a preview on another port of this host', pv.text.slice(0, 200))) return;
  const { listenPort, id } = pv.json.preview;
  cleanups.push(async () => void (await api('DELETE', `/api/previews/${id}`)));
  const origin = `${new URL(ORIGIN).protocol}//${new URL(ORIGIN).hostname}:${listenPort}`;
  check(!pv.json.preview.launch, `no launch link is needed (${origin})`);
  const noAuth = await fetch(`${origin}/`, { tls } as RequestInit);
  check(noAuth.status === 401, 'without the device cookie the preview is refused');
  const page = await fetch(`${origin}/app`, { headers: { cookie: `${cookie}; theme=dark` }, tls } as RequestInit);
  const body = await page.text();
  check(page.status === 200 && body.includes('preview /app') && body.includes('cookie=theme=dark'), 'the page loads with the device cookie; the cookie doesn’t reach the dev server', body.slice(0, 120));
  try {
    const hmr = await socket(`${origin.replace(/^https:/, 'wss:')}/hmr`, { cookie, origin });
    hmr.ws.send('ping');
    check((await hmr.next((m) => m.t === 'text' && m.data === 'echo:ping')).data === 'echo:ping', 'a dev-server WebSocket (HMR) works through the preview port');
    hmr.ws.close();
  } catch (e) {
    check(false, 'a dev-server WebSocket (HMR) works through the preview port', String(e));
  }
}

main()
  .catch((e) => {
    failures++;
    console.error(`  ✗ ${e?.stack || e}`);
  })
  .finally(async () => {
    for (const c of cleanups.reverse())
      try {
        await c();
      } catch {}
    console.log(failures ? `\n${failures} check(s) failed.` : '\nAll checks passed.');
    process.exit(failures ? 1 : 0);
  });
