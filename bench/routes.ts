// Route payloads: which shipped files a browser actually requests on a fresh Agents load, and which
// additional files each further screen or surface requests. Also checks which content encodings the
// real Briardr server (server/src/app.rs) applies to static files.
//   bun bench/routes.ts <name>=<dist> [...] --out <file.json>
import fs from 'node:fs';
import path from 'node:path';
import { chromium, webkit, type Page } from 'playwright';
import { FixtureServer } from './server.ts';
import { measure } from './sizes.ts';

const argv = process.argv.slice(2);
const outFile = argv.includes('--out') ? argv[argv.indexOf('--out') + 1] : null;
const builds = argv.filter((a) => a.includes('=') && !a.startsWith('--')).map((a) => {
  const [name, dir] = a.split('=');
  return { name, dir: path.resolve(dir) };
});

async function steps(page: Page, s: FixtureServer, log: () => string[]) {
  const out: { step: string; files: string[] }[] = [];
  const mark = (step: string) => out.push({ step, files: log().splice(0) });
  const fx = s.fixture;
  const name = fx.model.streams.find((x) => x.id === fx.picks.terminal)!.name;
  await page.goto(s.url + '/agents');
  await page.waitForSelector('.stream-row');
  await page.waitForLoadState('networkidle');
  mark('fresh load: Agents');
  for (const label of ['Workspaces', 'Settings']) {
    await page.locator('nav[aria-label="Main"]').getByRole('button', { name: label, exact: true }).click();
    await page.waitForTimeout(600);
    mark(label);
  }
  await page.locator('nav[aria-label="Main"] button', { hasText: 'Agents' }).click();
  await page.locator('.main [aria-label="Jump to a stream"]').click();
  await page.waitForFunction(() => document.activeElement?.getAttribute('aria-label') === 'Search streams');
  await page.waitForTimeout(200); // let the sheet attach its key handler (native autofocus can come first)
  await page.keyboard.press('Escape');
  await page.waitForSelector('.sheet.jump', { state: 'detached' });
  mark('Jump');
  await page.locator('.stream-row', { hasText: name }).getByRole('button', { name: /More actions/ }).click();
  await page.getByRole('menuitem', { name: /Peek/ }).click();
  await page.getByText('Watching live · read-only').waitFor();
  await page.getByRole('button', { name: 'Close' }).click();
  mark('Peek');
  await page.locator('.main .stream-row .row-main', { hasText: name }).click();
  await page.waitForSelector('.conn.conn-controlling, [data-ownership=controlling]');
  await page.waitForTimeout(500);
  mark('Stream: Terminal');
  for (const tab of ['Changes', 'Preview', 'Docs', 'Notes']) {
    await page.getByRole('tab', { name: tab, exact: true }).click();
    await page.waitForTimeout(600);
    if (tab === 'Changes') await page.locator('.file-toggle').first().click();
    if (tab === 'Docs') await page.locator('.doc-list .row-main').first().click();
    if (tab === 'Notes') await page.getByRole('button', { name: 'New note' }).click();
    await page.waitForTimeout(600);
    mark(`Stream: ${tab}`);
  }
  return out;
}

const result: any = { measuredAt: new Date().toISOString(), builds: {} };
for (const b of builds) {
  const sizes = new Map(measure(b.dir).map((f) => [f.file, f]));
  result.builds[b.name] = {};
  for (const engine of [chromium, webkit]) {
    const s = new FixtureServer(b.dir, 100).start();
    const browser = await engine.launch();
    const ctx = await browser.newContext({ viewport: { width: 1440, height: 900 }, serviceWorkers: 'block' });
    const page = await ctx.newPage();
    const seen: string[] = [];
    const transfer: Record<string, { encoding: string | null; bodyBytes: number; sentBytes: number | null }> = {};
    page.on('response', async (r) => {
      const u = new URL(r.url());
      if (u.origin !== s.url || u.pathname.startsWith('/api/') || u.pathname.startsWith('/ws/')) return;
      const rel = u.pathname === '/' || !path.extname(u.pathname) ? 'index.html' : u.pathname.slice(1);
      seen.push(rel);
      const body = await r.body().catch(() => Buffer.alloc(0));
      // Body bytes as sent (compressed, if the server compressed it).
      const sent = await r.request().sizes().then((z) => z.responseBodySize, () => null);
      transfer[rel] = { encoding: (await r.allHeaders())['content-encoding'] || null, bodyBytes: body.length, sentBytes: sent };
    });
    const list = await steps(page, s, () => seen);
    const firstSeen = new Set<string>();
    result.builds[b.name][engine.name()] = list.map(({ step, files }) => {
      const fresh = [...new Set(files)].filter((f) => !firstSeen.has(f));
      fresh.forEach((f) => firstSeen.add(f));
      const rows = fresh.map((f) => ({ file: f, ...(sizes.get(f) || { raw: transfer[f]?.bodyBytes ?? 0, gzip: null, brotli: null }), transfer: transfer[f] }));
      const sum = (k: 'raw' | 'gzip' | 'brotli') => rows.reduce((a, r: any) => a + (r[k] || 0), 0);
      const sent = rows.reduce((a, r: any) => a + (r.transfer?.sentBytes ?? r.raw), 0);
      return { step, files: rows, raw: sum('raw'), gzip: sum('gzip'), brotli: sum('brotli'), sent };
    });
    await browser.close();
    s.stop();
  }
}

// The real server: does it compress static files for browsers that accept gzip or Brotli?
{
  process.env.NODE_TLS_REJECT_UNAUTHORIZED = '0';
  const { startServer } = await import('../test/helpers.ts');
  const srv = await startServer({ pair: false });
  const dist = process.env.BRIARDR_WEB_DIST ? path.resolve(process.env.BRIARDR_WEB_DIST) : path.resolve('web/dist'); // what the server serves
  const js = fs.readdirSync(path.join(dist, 'assets')).find((f) => f.endsWith('.js'))!;
  const checks: any[] = [];
  for (const p of ['/', `/assets/${js}`, '/sw.js']) {
    const r = await fetch(srv.base + p, { headers: { 'accept-encoding': 'gzip, deflate, br, zstd' } });
    const buf = Buffer.from(await r.arrayBuffer());
    checks.push({ path: p, status: r.status, contentEncoding: r.headers.get('content-encoding'), contentLength: r.headers.get('content-length'), bytes: buf.length });
  }
  result.realServer = { note: 'Briardr server (server/src/app.rs serve_static) with Accept-Encoding: gzip, deflate, br, zstd', webDir: process.env.BRIARDR_WEB_DIST || 'web/dist', checks };
  await srv.close();
}

const text = JSON.stringify(result, null, 2);
if (outFile) fs.writeFileSync(outFile, text + '\n');
for (const [name, engines] of Object.entries(result.builds) as any)
  for (const [engine, list] of Object.entries(engines) as any) for (const s of list) console.log(name, engine, s.step.padEnd(20), s.files.map((f: any) => f.file).join(', ') || '—', s.raw);
console.log(JSON.stringify(result.realServer, null, 1));
process.exit(0);
