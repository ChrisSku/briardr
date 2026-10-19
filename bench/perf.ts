// Frontend performance benchmark. Runs identical scenarios against one or more built web clients, each
// served by the fixture server (bench/server.ts), in Chromium and WebKit at desktop and iPhone sizes.
//
//   bun bench/perf.ts --impl base=bench/builds/main-<sha> [--impl cand=bench/builds/worktree] --out <dir>
//     [--configs chromium-desktop,chromium-iphone-4x,webkit-desktop,webkit-iphone] [--sizes 100,500]
//     [--rounds 4] [--per-round 5] [--warmup 2] [--scenarios launch,nav,...]
//
// Every sample is appended to <dir>/samples.jsonl; behaviour checks go to <dir>/checks.jsonl. With
// several implementations, rounds alternate their order (A B, B A, ...) and cold launches alternate
// per iteration, to spread order, thermal and background-load effects evenly.
import fs from 'node:fs';
import path from 'node:path';
import { chromium, devices, webkit, type Browser, type BrowserContext, type CDPSession, type Page } from 'playwright';
import { pageHarness } from './page-harness.ts';
import { FixtureServer } from './server.ts';
import * as F from './fixtures.ts';

// ---------- arguments ----------
const argv = process.argv.slice(2);
const opt = (name: string, def?: string) => {
  const i = argv.indexOf('--' + name);
  return i >= 0 ? argv[i + 1] : def;
};
const impls = argv.flatMap((a, i) => (a === '--impl' ? [argv[i + 1]] : [])).map((s) => {
  const [name, dir] = s.split('=');
  return { name, dir: path.resolve(dir) };
});
if (!impls.length) throw new Error('give at least one --impl name=dist');
const OUT = path.resolve(opt('out', `bench/results/run-${new Date().toISOString().replace(/[:.]/g, '-')}`)!);
const SIZES = opt('sizes', '100,500')!.split(',').map(Number);
const ROUNDS = Number(opt('rounds', '4'));
const PER_ROUND = Number(opt('per-round', '5'));
const WARMUP = Number(opt('warmup', '2'));
const LAUNCHES = Number(opt('launches', String(ROUNDS * PER_ROUND)));
const ALL_SCENARIOS = ['launch', 'nav', 'jump', 'filter', 'expand', 'live', 'burst', 'term-open', 'term-output', 'surfaces', 'notes', 'memory'];
const SCENARIOS = opt('scenarios', ALL_SCENARIOS.join(','))!.split(',');
/** Scenarios whose cost depends on the number of streams; the rest run at the smallest size only. */
const SIZE_DEPENDENT = new Set(['launch', 'nav', 'jump', 'filter', 'expand', 'live', 'burst', 'term-output']);

interface Config {
  id: string;
  engine: 'chromium' | 'webkit';
  ctx: Record<string, any>;
  cpu: number;
  compact: boolean;
}
const iphone = (() => {
  const { defaultBrowserType, ...d } = devices['iPhone 15 Pro'] as any;
  return d;
})();
const CONFIGS: Config[] = [
  { id: 'chromium-desktop', engine: 'chromium', ctx: { viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 }, cpu: 1, compact: false },
  { id: 'chromium-iphone-4x', engine: 'chromium', ctx: iphone, cpu: 4, compact: true },
  { id: 'webkit-desktop', engine: 'webkit', ctx: { viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 }, cpu: 1, compact: false },
  { id: 'webkit-iphone', engine: 'webkit', ctx: iphone, cpu: 1, compact: true },
];
const configs = CONFIGS.filter((c) => opt('configs', CONFIGS.map((x) => x.id).join(','))!.split(',').includes(c.id));

fs.mkdirSync(OUT, { recursive: true });
const samplesFile = path.join(OUT, 'samples.jsonl');
const checksFile = path.join(OUT, 'checks.jsonl');

// ---------- output ----------
interface Ctx {
  impl: string;
  config: Config;
  size: number;
  round: number;
}
let iterNo = 0;
function sample(c: Ctx, scenario: string, metric: string, value: number | null, unit = 'ms', extra: object = {}) {
  fs.appendFileSync(samplesFile, JSON.stringify({ impl: c.impl, config: c.config.id, engine: c.config.engine, cpu: c.config.cpu, size: c.size, round: c.round, iter: iterNo, scenario, metric, value: value === null ? null : Math.round(value * 1000) / 1000, unit, ...extra }) + '\n');
}
function check(c: Ctx, scenario: string, name: string, ok: boolean, detail: object = {}) {
  fs.appendFileSync(checksFile, JSON.stringify({ impl: c.impl, config: c.config.id, size: c.size, round: c.round, iter: iterNo, scenario, check: name, ok, ...detail }) + '\n');
  if (!ok) console.log(`  CHECK FAILED ${c.impl} ${c.config.id} ${c.size} ${scenario} ${name}`, JSON.stringify(detail).slice(0, 300));
}
const log = (...a: unknown[]) => console.log(new Date().toISOString().slice(11, 19), ...a);

// ---------- page helpers ----------
const sleep = (ms: number) => new Promise((r) => setTimeout(r, ms));
/** Let the page go quiet between iterations: two frames and a task. */
async function settle(page: Page, ms = 60) {
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(r, 0)))));
  if (ms) await sleep(ms);
}

const CDP_KEYS = ['ScriptDuration', 'TaskDuration', 'LayoutDuration', 'RecalcStyleDuration'] as const;
async function cdpMetrics(cdp: CDPSession | null): Promise<Record<string, number> | null> {
  if (!cdp) return null;
  const { metrics } = await cdp.send('Performance.getMetrics');
  const out: Record<string, number> = {};
  for (const m of metrics) out[m.name] = m.value;
  return out;
}

interface Arm {
  trigger: string;
  preds: Record<string, string>;
  timeout?: number;
}
/** Measure one interaction: arm in the page, perform the action, collect the result. */
async function measure(c: Ctx, page: Page, cdp: CDPSession | null, scenario: string, a: Arm, act: () => Promise<unknown>, record = true, extra: object = {}) {
  const m0 = record ? await cdpMetrics(cdp) : null;
  await page.evaluate(({ trigger, preds, timeout }) => (window as any).__bench.arm(trigger, preds, timeout), { trigger: a.trigger, preds: a.preds, timeout: a.timeout ?? 15000 });
  await act();
  const r = await page.evaluate(() => (window as any).__bench.pending);
  const m1 = record ? await cdpMetrics(cdp) : null;
  if (r.error) {
    check(c, scenario, 'completed', false, { error: r.error, preds: Object.keys(a.preds), seen: Object.keys(r.dom), ...extra });
    return null;
  }
  if (!record) return r;
  const names = Object.keys(a.preds);
  for (const n of names) sample(c, scenario, names.length === 1 ? 'dom' : `dom:${n}`, r.dom[n] - r.t0, 'ms', extra);
  const last = Math.max(...names.map((n) => r.dom[n]));
  sample(c, scenario, 'frame', r.frame - r.t0, 'ms', extra);
  sample(c, scenario, 'after-frame', r.after - r.t0, 'ms', extra);
  if (r.proc !== null) sample(c, scenario, 'proc', r.proc - r.t0, 'ms', extra);
  const lt = await page.evaluate(([a, b]) => (window as any).__bench.longtasksBetween(a, b), [r.t0, r.after]);
  if (c.config.engine === 'chromium') {
    sample(c, scenario, 'longtask-count', lt.length, 'count', extra);
    sample(c, scenario, 'longtask-ms', lt.reduce((s: number, x: number[]) => s + x[1], 0), 'ms', extra);
  }
  if (m0 && m1) for (const k of CDP_KEYS) sample(c, scenario, `cdp:${k}`, (m1[k] - m0[k]) * 1000, 'ms', extra);
  void last;
  return r;
}

const q = JSON.stringify;
const count = (sel: string) => `document.querySelectorAll(${q(sel)}).length`;
const text = (sel: string) => `(document.querySelector(${q(sel)})?.textContent || '')`;
/** Exact row title inside a container. */
const hasRow = (container: string, name: string) => `[...document.querySelectorAll(${q(container + ' .row-title')})].some((e) => e.textContent.trim() === ${q(name)})`;

// ---------- expectations from the fixture ----------
function expect(fx: F.Fixture) {
  const nWs = fx.model.workspaces.length;
  const visibleAgents = fx.counts.attention + fx.counts.working + fx.counts.finished; // Ready and Unknown start collapsed
  return {
    nWs,
    visibleAgents,
    agentsReady: `document.querySelector('.main h1')?.textContent === 'Agents' && ${count('.main .stream-row')} === ${visibleAgents}`,
    treeReady: `document.querySelector('.main h1')?.textContent === 'Workspaces' && ${count('.main .tree-ws')} === ${nWs} && ${count('.main .tree-streams .stream-row')} === ${fx.size}`,
    usageReady: `document.querySelector('.main h1')?.textContent === 'Usage' && ${count('.usage-card .meter-row')} === 7`,
    settingsReady: `document.querySelector('.main h1')?.textContent === 'Settings' && ${count('.set-section')} >= 8 && ${text('#set-herdr')}.includes('Herdr 0.9.3 is running')`,
  };
}

function launchPredicate(fx: F.Fixture) {
  return expect(fx).agentsReady + ` && document.querySelector('.count-strip') !== null`;
}

async function newPage(browser: Browser, cfg: Config, fx: F.Fixture): Promise<{ ctx: BrowserContext; page: Page; cdp: CDPSession | null }> {
  const ctx = await browser.newContext({ ...cfg.ctx, serviceWorkers: 'block', locale: 'en-US', colorScheme: 'light', ignoreHTTPSErrors: true });
  await ctx.addInitScript(pageHarness, { launchPredicate: launchPredicate(fx) });
  const page = await ctx.newPage();
  page.on('pageerror', (e) => console.log('  pageerror', e.message));
  let cdp: CDPSession | null = null;
  if (cfg.engine === 'chromium') {
    cdp = await ctx.newCDPSession(page);
    await cdp.send('Performance.enable');
    if (cfg.cpu > 1) await cdp.send('Emulation.setCPUThrottlingRate', { rate: cfg.cpu });
  }
  return { ctx, page, cdp };
}

// ---------- scenarios ----------
const mainNav = (page: Page, label: string) => page.locator('nav[aria-label="Main"] button', { hasText: label });

async function scenarioLaunch(browser: Browser, cfg: Config, size: number, srv: Map<string, FixtureServer>) {
  const fx = srv.get(impls[0].name)!.fixture;
  const total = WARMUP + LAUNCHES;
  for (let i = 0; i < total; i++) {
    const order = i % 2 === 0 ? impls : [...impls].reverse();
    for (const im of order) {
      const c: Ctx = { impl: im.name, config: cfg, size, round: i < WARMUP ? -1 : i - WARMUP };
      iterNo = i;
      const s = srv.get(im.name)!;
      const { ctx, page, cdp } = await newPage(browser, cfg, fx);
      await page.goto(s.url + '/agents', { waitUntil: 'commit' });
      const ok = await page.waitForFunction(() => (window as any).__bench?.launch.interactive !== null && (window as any).__bench?.launch.frame !== null, null, { timeout: 30000, polling: 50 }).then(() => true, () => false);
      if (!ok) check(c, 'launch', 'completed', false);
      else if (i >= WARMUP) {
        const r = await page.evaluate(() => {
          const nav = performance.getEntriesByType('navigation')[0] as PerformanceNavigationTiming;
          const fcp = performance.getEntriesByName('first-contentful-paint')[0];
          const B = (window as any).__bench;
          const lt = B.longtasksBetween(0, B.launch.interactive);
          return { fcp: fcp ? fcp.startTime : null, dcl: nav?.domContentLoadedEventEnd ?? null, load: nav?.loadEventEnd || null, populated: B.launch.populated, interactive: B.launch.interactive, frame: B.launch.frame, lcp: B.launch.lcp, lt: B.longtaskSupported ? lt : null, res: performance.getEntriesByType('resource').length };
        });
        const m = await cdpMetrics(cdp);
        sample(c, 'launch', 'fcp', r.fcp);
        sample(c, 'launch', 'dom-content-loaded', r.dcl);
        sample(c, 'launch', 'populated', r.populated);
        sample(c, 'launch', 'interactive', r.interactive);
        sample(c, 'launch', 'populated-frame', r.frame);
        if (r.lcp !== null) sample(c, 'launch', 'lcp', r.lcp);
        if (r.lt) {
          sample(c, 'launch', 'longtask-count', r.lt.length, 'count');
          sample(c, 'launch', 'longtask-ms', r.lt.reduce((a: number, x: number[]) => a + x[1], 0));
        }
        if (m) {
          for (const k of CDP_KEYS) sample(c, 'launch', `cdp:${k}`, m[k] * 1000);
          sample(c, 'launch', 'js-heap-used', m.JSHeapUsedSize, 'bytes');
        }
      }
      await ctx.close();
    }
  }
}

async function warmContext(browser: Browser, cfg: Config, s: FixtureServer) {
  const { ctx, page, cdp } = await newPage(browser, cfg, s.fixture);
  await page.goto(s.url + '/agents');
  await page.waitForFunction(() => (window as any).__bench?.launch.interactive !== null, null, { timeout: 30000, polling: 50 });
  await settle(page, 300);
  return { ctx, page, cdp };
}

type Runner = (c: Ctx, page: Page, cdp: CDPSession | null, s: FixtureServer, record: boolean) => Promise<void>;

const scenarios: Record<string, Runner> = {
  // Agents → Workspaces → Usage → Settings → Agents, through the main navigation.
  async nav(c, page, cdp, s, record) {
    const e = expect(s.fixture);
    for (const [label, pred, metric] of [
      ['Workspaces', e.treeReady, 'workspaces'],
      ['Usage', e.usageReady, 'usage'],
      ['Settings', e.settingsReady, 'settings'],
      ['Agents', e.agentsReady, 'agents'],
    ] as const) {
      await measure(c, page, cdp, `nav:${metric}`, { trigger: 'click', preds: { ready: pred } }, () => mainNav(page, label).click(), record);
      await settle(page);
    }
  },

  // Jump: open, then a matching query, a query with no match, and clearing the field.
  async jump(c, page, cdp, s, record) {
    await toAgents(page, s);
    const fx = s.fixture;
    const total = fx.size;
    await measure(c, page, cdp, 'jump:open', { trigger: 'click', preds: { open: `document.activeElement?.getAttribute('aria-label') === 'Search streams' && ${count('.jump-list [role=option]')} === ${Math.min(50, total)}` } }, () => page.locator('.main .screen-head [aria-label="Jump to a stream"]').click(), record);
    const field = page.getByLabel('Search streams');
    await settle(page);
    await measure(c, page, cdp, 'jump:match', { trigger: 'input', preds: { results: `${count('.jump-list [role=option]')} === ${Math.min(50, fx.picks.searchMatches)}` } }, () => field.fill(fx.picks.searchTerm), record);
    await settle(page);
    await measure(c, page, cdp, 'jump:no-match', { trigger: 'input', preds: { results: `${count('.jump-list [role=option]')} === 0 && ${text('.jump-list')}.includes('No matching streams.')` } }, () => field.fill('zz-no-such-stream'), record);
    await settle(page);
    await measure(c, page, cdp, 'jump:clear', { trigger: 'input', preds: { results: `${count('.jump-list [role=option]')} === ${Math.min(50, total)}` } }, () => field.fill(''), record);
    await settle(page);
    await page.keyboard.press('Escape');
    await page.waitForSelector('.sheet.jump', { state: 'detached' });
    await settle(page);
  },

  // Filters on the Agents count strip: Working on/off, Ready (a collapsed group) on/off.
  async filter(c, page, cdp, s, record) {
    await toAgents(page, s);
    const e = expect(s.fixture);
    const n = s.fixture.counts;
    const btn = (label: string) => page.locator('.count-strip button', { hasText: label });
    await measure(c, page, cdp, 'filter:working', { trigger: 'click', preds: { rows: `${count('.main .stream-row')} === ${n.working} && ${count('.main .grp-attention')} === 0` } }, () => btn('Working').click(), record);
    await settle(page);
    await measure(c, page, cdp, 'filter:clear', { trigger: 'click', preds: { rows: e.agentsReady } }, () => btn('Working').click(), record);
    await settle(page);
    await measure(c, page, cdp, 'filter:ready', { trigger: 'click', preds: { rows: `${count('.main .stream-row')} === ${n.ready} && ${count('.main .grp-ready')} === 1` } }, () => btn('Ready').click(), record);
    await settle(page);
    await measure(c, page, cdp, 'filter:clear', { trigger: 'click', preds: { rows: e.agentsReady } }, () => btn('Ready').click(), record);
    await settle(page);
  },

  // Workspaces tree: collapse and expand the first workspace.
  async expand(c, page, cdp, s, record) {
    const e = expect(s.fixture);
    const first = s.fixture.model.workspaces[0];
    const inFirst = s.fixture.model.streams.filter((x) => x.workspaceId === first.id).length;
    if (!(await page.locator('.main .tree').count())) {
      await mainNav(page, 'Workspaces').click();
      await page.waitForFunction(e.treeReady);
      await settle(page);
    }
    const ws = page.locator('.tree-ws').first();
    const toggle = ws.locator('.tree-toggle').first();
    // Builds before the WorkspaceTree toggle fix took two clicks to collapse a workspace expanded by
    // default. Collapse and expand once, unmeasured, so every build starts from the same stored state.
    if (!(await page.evaluate((id) => JSON.parse(localStorage.getItem('briardr.tree.expanded') || '{}')[id] === true, first.id))) {
      for (const want of ['false', 'true'])
        for (let i = 0; i < 2 && (await ws.getAttribute('aria-expanded')) !== want; i++) {
          await toggle.click();
          await settle(page, 100);
        }
    }
    await measure(c, page, cdp, 'expand:collapse', { trigger: 'click', preds: { rows: `document.querySelector('.tree-ws').getAttribute('aria-expanded') === 'false' && ${count('.main .tree-streams .stream-row')} === ${s.fixture.size - inFirst}` } }, () => toggle.click(), record);
    await settle(page);
    await measure(c, page, cdp, 'expand:expand', { trigger: 'click', preds: { rows: e.treeReady } }, () => toggle.click(), record);
    await settle(page);
  },

  // One stream's state changes on the Mac (Working ↔ Needs you); the row moves between groups.
  async live(c, page, cdp, s, record) {
    await toAgents(page, s);
    const id = s.fixture.picks.live;
    const name = s.fixture.model.streams.find((x) => x.id === id)!.name;
    for (const to of ['blocked', 'working'] as const) {
      const mark = `live-${c.round}-${iterNo}-${to}-${Math.random().toString(36).slice(2, 8)}`;
      const container = to === 'blocked' ? '.main .grp-attention' : '.main .grp-working';
      const other = to === 'blocked' ? '.main .grp-working' : '.main .grp-attention';
      await measure(
        c, page, cdp, `live:to-${to}`,
        { trigger: `recv:${mark}`, preds: { moved: `${hasRow(container, name)} && !${hasRow(other, name)}` } },
        async () =>
          s.pushModel((m) => {
            const st = m.streams.find((x) => x.id === id)!;
            st.state = to;
            st.stateChangeSeq = (st.stateChangeSeq || 0) + 1;
            st.stateSince = Date.now();
          }, mark),
        record,
      );
      await settle(page);
    }
  },

  // 20 full model updates, 80 ms apart (the real server's debounce), each renaming one stream.
  async burst(c, page, cdp, s, record) {
    await toAgents(page, s);
    const id = s.fixture.picks.burst;
    const tag = `b${c.round}x${iterNo}x${Math.random().toString(36).slice(2, 6)}`;
    const N = 20;
    await page.evaluate(([tag, N]) => {
      const B = (window as any).__bench;
      B.burst = { seen: {} as Record<number, number> };
      const mo = new MutationObserver(() => {
        const t = performance.now();
        for (const e of document.querySelectorAll('.main .row-title')) {
          const m = new RegExp(`^${tag}-(\\d+)$`).exec(e.textContent!.trim());
          if (m) for (let k = 0; k <= Number(m[1]); k++) if (B.burst.seen[k] === undefined) B.burst.seen[k] = t;
        }
        if (B.burst.seen[(N as number) - 1] !== undefined) mo.disconnect();
      });
      mo.observe(document, { subtree: true, childList: true, characterData: true });
      B.frames.start();
    }, [tag, N] as const);
    const m0 = record ? await cdpMetrics(cdp) : null;
    for (let k = 0; k < N; k++) {
      s.pushModel((m) => {
        const st = m.streams.find((x) => x.id === id)!;
        st.label = st.name = `${tag}-${k}`;
      }, `${tag}-${k}`);
      await sleep(80);
    }
    await page.waitForFunction((N) => (window as any).__bench.burst.seen[(N as number) - 1] !== undefined, N, { timeout: 15000 }).catch(() => null);
    await settle(page, 0);
    const r = await page.evaluate(([tag, N]) => {
      const B = (window as any).__bench;
      const f = B.frames.stop();
      const lat = Array.from({ length: N as number }, (_, k) => (B.burst.seen[k] !== undefined && B.recv[`${tag}-${k}`] !== undefined ? B.burst.seen[k] - B.recv[`${tag}-${k}`] : null));
      return { lat, f, total: B.burst.seen[(N as number) - 1] - B.recv[`${tag}-0`] };
    }, [tag, N] as const);
    const m1 = record ? await cdpMetrics(cdp) : null;
    const missing = r.lat.filter((x: number | null) => x === null).length;
    check(c, 'burst', 'all-updates-rendered', missing === 0, { missing });
    if (!record) return;
    const lat = r.lat.filter((x: number | null): x is number => x !== null).sort((a: number, b: number) => a - b);
    sample(c, 'burst', 'update-median', lat[Math.floor(lat.length / 2)]);
    sample(c, 'burst', 'update-max', lat[lat.length - 1]);
    sample(c, 'burst', 'last-update', r.lat[N - 1]);
    frameStats(c, 'burst', r.f);
    if (m0 && m1) for (const k of CDP_KEYS) sample(c, 'burst', `cdp:${k}`, (m1[k] - m0[k]) * 1000);
  },

  // Open a stream from the Agents list: xterm mounted, then live; then back to the list.
  async 'term-open'(c, page, cdp, s, record) {
    await toAgents(page, s);
    const e = expect(s.fixture);
    const id = s.fixture.picks.terminal;
    const name = s.fixture.model.streams.find((x) => x.id === id)!.name;
    const row = page.locator('.main .stream-row').filter({ has: page.locator('.row-title', { hasText: new RegExp(`^${name}$`) }) }).locator('.row-main');
    await measure(c, page, cdp, 'term-open', { trigger: 'click', preds: { mounted: `document.querySelector('.term-host .xterm-screen') !== null`, live: `document.querySelector('.conn.conn-controlling, [data-ownership=controlling]') !== null` } }, () => row.click(), record);
    await settle(page, 150);
    await measure(c, page, cdp, 'term-close', { trigger: 'click', preds: { list: e.agentsReady } }, () => page.locator('.stream-head .back').click(), record);
    await settle(page);
  },

  // Terminal output: 180 full-screen redraws, 16 ms apart (~60 fps for 3 s), into a controlling terminal.
  async 'term-output'(c, page, cdp, s, record) {
    const e = expect(s.fixture);
    await openStream(page, s);
    const N = 180;
    await page.evaluate(() => {
      const B = (window as any).__bench;
      B.binary = { count: 0, first: 0, last: 0 };
      B.frames.start();
    });
    const m0 = record ? await cdpMetrics(cdp) : null;
    await s.termBurst(N, 16);
    const done = await page
      .waitForFunction((N) => {
        const ctl = (document.querySelector('.term-host') as any)?.briardrTerminal;
        const line = ctl?.term.buffer.active.getLine(0)?.translateToString(true) || '';
        if (line.startsWith(`FRAME ${(N as number) - 1} `)) return performance.now();
        return false;
      }, N, { timeout: 20000, polling: 'raf' })
      .then((h) => h.jsonValue() as Promise<number>, () => null);
    await settle(page, 0);
    const r = await page.evaluate(() => {
      const B = (window as any).__bench;
      return { f: B.frames.stop(), bin: B.binary };
    });
    const m1 = record ? await cdpMetrics(cdp) : null;
    check(c, 'term-output', 'all-frames-shown', done !== null && r.bin.count === N, { frames: r.bin.count });
    if (record && done !== null) {
      sample(c, 'term-output', 'last-frame-shown', done - r.bin.first);
      sample(c, 'term-output', 'lag-after-last-frame', done - r.bin.last);
      frameStats(c, 'term-output', r.f);
      if (m0 && m1) for (const k of CDP_KEYS) sample(c, 'term-output', `cdp:${k}`, (m1[k] - m0[k]) * 1000);
    }
    await page.locator('.stream-head .back').click();
    await page.waitForFunction(e.agentsReady);
    await settle(page);
  },

  // Switching surfaces keeps the same terminal and connection.
  async surfaces(c, page, cdp, s, record) {
    const e = expect(s.fixture);
    await openStream(page, s);
    await page.evaluate(() => {
      const B = (window as any).__bench;
      B.keep = { screen: document.querySelector('.xterm-screen'), socket: B.openSockets().find((x: any) => x.url === '/ws/term')?.id };
    });
    const tab = (name: string) => page.getByRole('tab', { name, exact: true });
    for (const [name, pred] of [
      ['Changes', `${count('.surface.changes .file')} === 40`],
      ['Preview', `document.querySelector('.surface.preview') && !document.querySelector('.surface.preview').classList.contains('is-hidden') && document.querySelector('.term-surface').classList.contains('is-hidden')`],
      ['Docs', `${count('.doc-list .row')} === 20`],
      ['Notes', `${count('.note-card')} === 5`],
      ['Terminal', `!document.querySelector('.term-surface').classList.contains('is-hidden') && ${count('.surface.notes-list')} === 0`],
    ] as const) {
      await measure(c, page, cdp, `surface:${name.toLowerCase()}`, { trigger: 'click', preds: { ready: pred } }, () => tab(name).click(), record);
      await settle(page);
      const kept = await page.evaluate(() => {
        const B = (window as any).__bench;
        const sock = B.openSockets().filter((x: any) => x.url === '/ws/term');
        return { sameScreen: document.querySelector('.xterm-screen') === B.keep.screen && B.keep.screen.isConnected, sockets: sock.length, sameSocket: sock[0]?.id === B.keep.socket };
      });
      check(c, 'surfaces', `terminal-kept:${name}`, kept.sameScreen && kept.sockets === 1 && kept.sameSocket, kept);
    }
    await page.locator('.stream-head .back').click();
    await page.waitForFunction(e.agentsReady);
    await settle(page);
  },

  // Notes: a new note, ten keystrokes, a pasted Markdown body, Preview, autosave and back to the list.
  async notes(c, page, cdp, s, record) {
    const e = expect(s.fixture);
    const sid = s.fixture.picks.terminal;
    await openStream(page, s);
    await page.getByRole('tab', { name: 'Notes', exact: true }).click();
    await page.waitForFunction(`${count('.note-card')} === 5`);
    await settle(page);
    await measure(c, page, cdp, 'notes:new', { trigger: 'click', preds: { editor: `document.activeElement?.classList.contains('notes-area') === true` } }, () => page.getByRole('button', { name: 'New note' }).click(), record);
    await settle(page);
    s.resetLog();
    const keys = 'retry plan';
    for (let i = 0; i < keys.length; i++) {
      const pred = i === 0 ? `${text('.notes-status')}.includes('Editing')` : `document.querySelector('.notes-area').value.length === ${i + 1}`;
      await measure(c, page, cdp, i === 0 ? 'notes:first-key' : 'notes:key', { trigger: 'input', preds: { shown: pred } }, () => page.keyboard.type(keys[i]), record);
    }
    const body = F.noteText(7) + '\n\n' + F.docText('notes.md');
    await measure(c, page, cdp, 'notes:paste', { trigger: 'input', preds: { shown: `document.querySelector('.notes-area').value.length === ${body.length}` } }, () => page.getByLabel(/^Note for /).fill(body), record);
    await page.getByText('Saved on the Mac').waitFor({ timeout: 10000 });
    const savedAt = Date.now();
    await sleep(1500); // nothing more may be saved once it says Saved
    const puts = s.requests.filter((r) => r.method === 'PUT' && r.path.includes('/notes/'));
    const noteId = puts[0]?.path.split('/').pop();
    const stored = (s.notes.get(sid) || []).find((n) => n.id === noteId);
    const late = puts.filter((p) => p.t > savedAt + 50);
    check(c, 'notes', 'autosave', !!stored && stored.text === body && late.length === 0 && puts.every((p, i) => p.body.baseRevision === i), { puts: puts.length, late: late.length, revisions: puts.map((p) => p.body.baseRevision) });
    if (record) sample(c, 'notes', 'autosave-puts', puts.length, 'count');
    await measure(c, page, cdp, 'notes:preview', { trigger: 'click', preds: { rendered: `${count('.notes-preview table')} >= 9 && ${count('.notes-preview h2')} >= 9` } }, () => page.getByRole('button', { name: 'Preview', exact: true }).click(), record);
    await settle(page);
    await page.getByRole('button', { name: 'All notes' }).click();
    await page.waitForFunction(`${count('.note-card')} === 6`);
    // Put the fixture back: remove the new note on the "Mac" and tell the client.
    s.notes.set(sid, (s.notes.get(sid) || []).filter((n) => n.id !== noteId));
    s.broadcast({ t: 'notes', streamId: sid, noteId, revision: 0 });
    await page.waitForFunction(`${count('.note-card')} === 5`);
    await page.getByRole('tab', { name: 'Terminal', exact: true }).click();
    await page.locator('.stream-head .back').click();
    await page.waitForFunction(e.agentsReady);
    await settle(page);
  },

  // Repeated navigation: open and close a stream (and Peek) 30 times; resources must come back.
  async memory(c, page, cdp, s, record) {
    // A 30-cycle trend, once per round (on the first measured iteration).
    if (!record || iterNo % PER_ROUND !== 0) return;
    await toAgents(page, s);
    const e = expect(s.fixture);
    const id = s.fixture.picks.terminal;
    const name = s.fixture.model.streams.find((x) => x.id === id)!.name;
    const snap = async (cycle: number) => {
      let heap: number | null = null;
      let dom: any = null;
      if (cdp) {
        await cdp.send('HeapProfiler.collectGarbage');
        await cdp.send('HeapProfiler.collectGarbage');
        heap = (await cdp.send('Runtime.getHeapUsage')).usedSize;
        dom = await cdp.send('Memory.getDOMCounters');
      }
      await settle(page, 300);
      const p = await page.evaluate(() => {
        const B = (window as any).__bench;
        const socks = B.openSockets();
        return { sockets: socks.length, termSockets: socks.filter((x: any) => x.url === '/ws/term').length, listeners: B.listeners(), intervals: B.intervals(), resizeObservers: B.resizeObservers(), elements: document.getElementsByTagName('*').length, xterms: document.querySelectorAll('.xterm').length };
      });
      const srvSocks = s.openSockets();
      const extra = { cycle };
      if (heap !== null) sample(c, 'memory', 'js-heap-used', heap, 'bytes', extra);
      if (dom) {
        sample(c, 'memory', 'dom-nodes', dom.nodes, 'count', extra);
        sample(c, 'memory', 'js-event-listeners', dom.jsEventListeners, 'count', extra);
      }
      sample(c, 'memory', 'elements', p.elements, 'count', extra);
      sample(c, 'memory', 'open-sockets', p.sockets, 'count', extra);
      sample(c, 'memory', 'server-term-sockets', srvSocks.term, 'count', extra);
      sample(c, 'memory', 'global-listeners', Object.values(p.listeners as Record<string, number>).reduce((a, b) => a + b, 0), 'count', extra);
      sample(c, 'memory', 'intervals', p.intervals, 'count', extra);
      sample(c, 'memory', 'resize-observers', p.resizeObservers, 'count', extra);
      return { ...p, srvSocks };
    };
    const base = await snap(0);
    const row = page.locator('.main .stream-row').filter({ has: page.locator('.row-title', { hasText: new RegExp(`^${name}$`) }) });
    for (let i = 1; i <= 30; i++) {
      await row.locator('.row-main').click();
      await page.waitForSelector('.conn.conn-controlling, [data-ownership=controlling]');
      await settle(page, 30);
      await page.locator('.stream-head .back').click();
      await page.waitForFunction(e.agentsReady);
      if (i % 3 === 0) {
        await row.getByRole('button', { name: /More actions/ }).click();
        await page.getByRole('menuitem', { name: /Peek/ }).click();
        await page.getByText('Watching live · read-only').waitFor();
        await page.getByRole('button', { name: 'Close' }).click();
        await page.waitForSelector('.sheet.peek', { state: 'detached' });
      }
      if (i % 10 === 0) {
        const now = await snap(i);
        const same = now.sockets === base.sockets && now.termSockets === 0 && now.xterms === 0 && now.intervals === base.intervals && now.resizeObservers === base.resizeObservers && JSON.stringify(now.listeners) === JSON.stringify(base.listeners);
        check(c, 'memory', `released-after-${i}`, same, { base: { ...base }, now });
      }
    }
  },
};

function frameStats(c: Ctx, scenario: string, f: { gaps: number[]; longtasks: number[][]; t0: number; t1: number }) {
  const g = [...f.gaps].sort((a, b) => a - b);
  if (g.length) {
    sample(c, scenario, 'frame-gap-p95', g[Math.min(g.length - 1, Math.floor(g.length * 0.95))]);
    sample(c, scenario, 'frame-gap-max', g[g.length - 1]);
    sample(c, scenario, 'frames-over-50ms', g.filter((x) => x > 50).length, 'count');
    sample(c, scenario, 'frames', g.length, 'count');
  }
  if (c.config.engine === 'chromium') {
    sample(c, scenario, 'longtask-count', f.longtasks.length, 'count');
    sample(c, scenario, 'longtask-ms', f.longtasks.reduce((a, x) => a + x[1], 0));
  }
}

/** Back to the Agents list (not measured) if a previous scenario left another screen open. */
async function toAgents(page: Page, s: FixtureServer) {
  const e = expect(s.fixture);
  if (await page.evaluate(e.agentsReady)) return;
  if (await page.locator('.stream-head .back').count()) await page.locator('.stream-head .back').click();
  else await mainNav(page, 'Agents').click();
  await page.waitForFunction(e.agentsReady);
  await settle(page);
}

async function openStream(page: Page, s: FixtureServer) {
  await toAgents(page, s);
  const id = s.fixture.picks.terminal;
  const name = s.fixture.model.streams.find((x) => x.id === id)!.name;
  await page.locator('.main .stream-row').filter({ has: page.locator('.row-title', { hasText: new RegExp(`^${name}$`) }) }).locator('.row-main').click();
  await page.waitForSelector('.conn.conn-controlling, [data-ownership=controlling]');
  await settle(page, 150);
}

// ---------- main ----------
const browsers: Record<string, Browser> = {};
async function browserFor(engine: 'chromium' | 'webkit') {
  return (browsers[engine] ||= await (engine === 'chromium' ? chromium : webkit).launch());
}

const meta = {
  startedAt: new Date().toISOString(),
  impls: impls.map((i) => ({ ...i, version: fs.readFileSync(path.join(i.dir, 'index.html'), 'utf8').match(/briardr-version" content="([^"]+)"/)?.[1], js: fs.readdirSync(path.join(i.dir, 'assets')).filter((f) => f.endsWith('.js')) })),
  configs: configs.map((c) => ({ id: c.id, engine: c.engine, cpuThrottle: c.cpu, viewport: c.ctx.viewport, deviceScaleFactor: c.ctx.deviceScaleFactor, isMobile: !!c.ctx.isMobile, hasTouch: !!c.ctx.hasTouch })),
  sizes: SIZES,
  rounds: ROUNDS,
  perRound: PER_ROUND,
  warmup: WARMUP,
  launches: LAUNCHES,
  scenarios: SCENARIOS,
  network: 'loopback HTTP, no throttling, fixture server in the runner process',
  serviceWorkers: 'blocked',
  browsers: {} as Record<string, string>,
};

for (const cfg of configs) {
  const browser = await browserFor(cfg.engine);
  meta.browsers[cfg.engine] = browser.version();
  fs.writeFileSync(path.join(OUT, 'meta.json'), JSON.stringify(meta, null, 2));
  for (const size of SIZES) {
    const servers = new Map(impls.map((im) => [im.name, new FixtureServer(im.dir, size).start()]));
    const want = SCENARIOS.filter((x) => (size === SIZES[0] || SIZE_DEPENDENT.has(x)));
    if (want.includes('launch')) {
      log(`${cfg.id} ${size} launch`);
      await scenarioLaunch(browser, cfg, size, servers);
    }
    const interactive = want.filter((x) => x !== 'launch');
    if (interactive.length)
      for (let round = 0; round < ROUNDS; round++) {
        const order = round % 2 === 0 ? impls : [...impls].reverse();
        for (const im of order) {
          const s = servers.get(im.name)!;
          const c: Ctx = { impl: im.name, config: cfg, size, round };
          log(`${cfg.id} ${size} round ${round + 1}/${ROUNDS} ${im.name}`);
          const { ctx, page, cdp } = await warmContext(browser, cfg, s);
          try {
            for (const sc of interactive) {
              const t0 = Date.now();
              for (let i = 0; i < WARMUP + PER_ROUND; i++) {
                iterNo = round * PER_ROUND + i - WARMUP;
                try {
                  await scenarios[sc](c, page, cdp, s, i >= WARMUP);
                } catch (err: any) {
                  check(c, sc, 'completed', false, { error: String(err?.message || err).slice(0, 300) });
                  // Recover to the Agents list for the next iteration.
                  await page.goto(s.url + '/agents').catch(() => {});
                  await page.waitForFunction(() => (window as any).__bench?.launch.interactive !== null, null, { timeout: 30000 }).catch(() => {});
                }
              }
              log(`  ${sc} ${((Date.now() - t0) / 1000).toFixed(1)} s`);
            }
          } finally {
            await ctx.close();
          }
        }
      }
    for (const s of servers.values()) s.stop();
  }
}
meta['finishedAt' as keyof typeof meta] = new Date().toISOString() as any;
fs.writeFileSync(path.join(OUT, 'meta.json'), JSON.stringify(meta, null, 2));
for (const b of Object.values(browsers)) await b.close();
log('done', OUT);
