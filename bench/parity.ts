// Visual and behavioural parity between two builds. Both are served the same fixture with a frozen
// clock (server and page), then every screen is captured at iPhone, iPad and Mac sizes in light and
// dark mode. For each screen it compares pixels, the visible text and the accessibility tree.
//   bun bench/parity.ts base=bench/builds/main-<sha> cand=bench/builds/worktree --out bench/results/parity
import fs from 'node:fs';
import path from 'node:path';
import { chromium, devices, webkit, type Browser, type Page } from 'playwright';
import { FixtureServer } from './server.ts';

const argv = process.argv.slice(2);
const OUT = path.resolve(argv.includes('--out') ? argv[argv.indexOf('--out') + 1] : 'bench/results/parity');
// Full-resolution screenshots are large; --screens puts them elsewhere (e.g. the ignored bench/screens-full/).
const SCREENS = path.resolve(argv.includes('--screens') ? argv[argv.indexOf('--screens') + 1] : path.join(OUT, 'screens'));
const builds = argv.filter((a) => a.includes('=') && !a.startsWith('--')).map((a) => ({ name: a.split('=')[0], dir: path.resolve(a.split('=')[1]) }));
const FIXED = Date.UTC(2026, 9, 8, 10, 0, 0);

const PROFILES: Record<string, any> = {
  iphone: (({ defaultBrowserType, ...d }) => d)(devices['iPhone 15 Pro'] as any),
  ipad: (({ defaultBrowserType, ...d }) => d)(devices['iPad Pro 11'] as any),
  mac: { viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2 },
};

type Step = { name: string; run: (page: Page, s: FixtureServer) => Promise<void> };
const nav = (label: string) => async (page: Page) => {
  await page.locator('nav[aria-label="Main"] button', { hasText: label }).click();
};
const busyStream = 's3';
const streamName = (s: FixtureServer) => s.fixture.model.streams.find((x) => x.id === s.fixture.picks.terminal)!.name;
const STEPS: Step[] = [
  { name: '01-agents', run: async () => {} },
  { name: '02-workspaces', run: nav('Workspaces') },
  { name: '03-usage', run: nav('Usage') },
  { name: '04-settings', run: nav('Settings') },
  {
    name: '05-jump',
    run: async (page) => {
      await nav('Agents')(page);
      await page.locator('.main [aria-label="Jump to a stream"]').click();
      await page.getByLabel('Search streams').fill('flaky');
    },
  },
  {
    name: '06-row-menu',
    run: async (page, s) => {
      await page.keyboard.press('Escape');
      await page.waitForSelector('.sheet.jump', { state: 'detached' });
      await page.locator('.main .stream-row', { hasText: streamName(s) }).getByRole('button', { name: /More actions/ }).click();
    },
  },
  {
    name: '07-peek',
    run: async (page) => {
      await page.getByRole('menuitem', { name: /Peek/ }).click();
      await page.getByText('Watching live · read-only').waitFor();
    },
  },
  {
    name: '08-terminal',
    run: async (page, s) => {
      await page.getByRole('button', { name: 'Close' }).click();
      await page.locator('.main .stream-row .row-main', { hasText: streamName(s) }).click();
      await page.waitForSelector('.conn.conn-controlling, [data-ownership=controlling]');
    },
  },
  {
    name: '09-stream-actions',
    run: async (page) => {
      await page.getByRole('button', { name: 'Stream actions' }).click();
    },
  },
  {
    name: '10-changes',
    run: async (page) => {
      await page.keyboard.press('Escape');
      // Builds before the MenuHost focus fix ignore Escape until the menu is clicked; tap outside instead.
      if (await page.locator('.menu-scrim').count()) await page.locator('.menu-scrim').click({ position: { x: 4, y: 4 } });
      await page.getByRole('tab', { name: 'Changes', exact: true }).click();
      await page.locator('.file-toggle').first().click();
      await page.waitForSelector('.diff');
    },
  },
  { name: '11-preview', run: async (page) => void (await page.getByRole('tab', { name: 'Preview', exact: true }).click()) },
  {
    name: '12-docs',
    run: async (page) => {
      await page.getByRole('tab', { name: 'Docs', exact: true }).click();
      await page.locator('.doc-list .row-main').first().click();
      await page.waitForSelector('.md h2');
    },
  },
  {
    name: '13-notes',
    run: async (page) => {
      await page.getByRole('tab', { name: 'Notes', exact: true }).click();
      await page.waitForSelector('.note-card');
    },
  },
  {
    name: '14-note-editor',
    run: async (page) => {
      await page.locator('.note-card').first().getByRole('button', { name: 'Edit' }).click();
      await page.waitForSelector('.notes-area');
      await page.getByText('Saved on the Mac').waitFor();
      await page.evaluate(() => (document.activeElement as HTMLElement | null)?.blur());
    },
  },
  {
    name: '15-note-preview',
    run: async (page) => {
      await page.getByRole('button', { name: 'Preview', exact: true }).click();
      await page.waitForSelector('.notes-preview h2');
    },
  },
  {
    name: '16-delete-confirm',
    run: async (page) => {
      await page.getByRole('button', { name: 'All notes' }).click();
      await page.locator('.note-card').first().getByRole('button', { name: 'Delete note' }).click();
      await page.waitForSelector('.sheet.confirm');
    },
  },
  {
    name: '17-paste-sheet',
    run: async (page) => {
      await page.getByRole('button', { name: 'Cancel' }).click();
      await page.getByRole('tab', { name: 'Terminal', exact: true }).click();
      const plus = page.getByRole('button', { name: 'Insert text, image or file' });
      if (await plus.count()) {
        await plus.click();
        await page.getByRole('menuitem', { name: /Paste text/ }).click();
        await page.getByLabel('Text to paste').fill('echo parity\nline two');
      }
    },
  },
  {
    name: '18-history',
    run: async (page) => {
      if (await page.locator('.sheet-head [aria-label="Close"]').count()) await page.locator('.sheet-head [aria-label="Close"]').click();
      await page.getByRole('button', { name: 'Stream actions' }).click();
      await page.getByRole('menuitem', { name: 'History' }).click();
      await page.waitForSelector('.history-text');
    },
  },
  {
    // Another client controls this stream: watching, the Mac-sized screen scaled to fit, and the
    // banner in the overlay above the header.
    name: '19-watching',
    run: async (page, s) => {
      await page.locator('.sheet-head [aria-label="Close"]').click();
      await page.locator('.stream-head .back').click();
      const name = s.fixture.model.streams.find((x) => x.id === busyStream)!.name;
      await page.goto(s.url + `/stream/${busyStream}`);
      await page.waitForSelector('.conn.conn-watching, [data-ownership=watching]');
      await page.locator('.watch-banner').waitFor();
      void name;
    },
  },
  {
    name: '20-takeover-confirm',
    run: async (page) => {
      await page.locator('.watch-banner').getByRole('button', { name: 'Take over' }).click();
      await page.waitForSelector('.sheet.confirm');
    },
  },
];

async function settle(page: Page) {
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => setTimeout(r, 50)))));
  await page.waitForTimeout(250);
}

async function capture(browser: Browser, b: { name: string; dir: string }, profile: string, scheme: 'light' | 'dark') {
  const s = new FixtureServer(b.dir, 100, { fixedNow: FIXED, busy: [busyStream] }).start();
  const ctx = await browser.newContext({ ...PROFILES[profile], colorScheme: scheme, locale: 'en-US', timezoneId: 'UTC', serviceWorkers: 'block', reducedMotion: 'reduce' });
  const page = await ctx.newPage();
  page.setDefaultTimeout(8000);
  await page.clock.setFixedTime(FIXED);
  const errors: string[] = [];
  page.on('pageerror', (e) => errors.push(e.message));
  page.on('console', (m) => m.type() === 'error' && errors.push(m.text()));
  await page.goto(s.url + '/agents');
  await page.waitForSelector('.stream-row');
  const out: Record<string, { png: string; text: string; aria: string; error?: string }> = {};
  for (const step of STEPS) {
    try {
      await step.run(page, s);
      await settle(page);
      // Settings → About names the build (ID, branch, commit), which differs between any two builds.
      await page.evaluate(() => document.querySelectorAll('.build-info dd').forEach((d) => (d.textContent = 'build')));
      const png = path.join(SCREENS, `${profile}-${scheme}-${step.name}-${b.name}.png`);
      await page.screenshot({ path: png, animations: 'disabled', caret: 'hide' });
      const text = await page.evaluate(() => document.body.innerText);
      const aria = await page.locator('body').ariaSnapshot();
      out[step.name] = { png, text, aria };
    } catch (e: any) {
      out[step.name] = { png: '', text: '', aria: '', error: String(e.message).split('\n')[0] };
    }
  }
  await ctx.close();
  s.stop();
  return { out, errors };
}

/** Pixels that differ between two screenshots, compared in a browser canvas. */
async function pixelDiff(page: Page, a: string, b: string): Promise<{ differing: number; total: number; sizeMatch: boolean; diffPng?: string }> {
  const da = 'data:image/png;base64,' + fs.readFileSync(a).toString('base64');
  const db = 'data:image/png;base64,' + fs.readFileSync(b).toString('base64');
  const r = await page.evaluate(
    async ([da, db]) => {
      const load = (u: string) => new Promise<HTMLImageElement>((res) => {
        const i = new Image();
        i.onload = () => res(i);
        i.src = u;
      });
      const [ia, ib] = await Promise.all([load(da), load(db)]);
      if (ia.width !== ib.width || ia.height !== ib.height) return { differing: -1, total: ia.width * ia.height, sizeMatch: false };
      const c = (img: HTMLImageElement) => {
        const cv = document.createElement('canvas');
        cv.width = img.width;
        cv.height = img.height;
        const x = cv.getContext('2d')!;
        x.drawImage(img, 0, 0);
        return x.getImageData(0, 0, img.width, img.height);
      };
      const pa = c(ia);
      const pb = c(ib);
      const out = new ImageData(ia.width, ia.height);
      let n = 0;
      for (let i = 0; i < pa.data.length; i += 4) {
        const d = Math.abs(pa.data[i] - pb.data[i]) + Math.abs(pa.data[i + 1] - pb.data[i + 1]) + Math.abs(pa.data[i + 2] - pb.data[i + 2]);
        if (d > 0) {
          n++;
          out.data[i] = 255;
          out.data[i + 3] = 255;
        } else {
          out.data[i] = out.data[i + 1] = out.data[i + 2] = pa.data[i] / 3;
          out.data[i + 3] = 255;
        }
      }
      let url: string | undefined;
      if (n) {
        const cv = document.createElement('canvas');
        cv.width = ia.width;
        cv.height = ia.height;
        cv.getContext('2d')!.putImageData(out, 0, 0);
        url = cv.toDataURL('image/png');
      }
      return { differing: n, total: ia.width * ia.height, sizeMatch: true, url };
    },
    [da, db] as const,
  );
  let diffPng: string | undefined;
  if ((r as any).url) {
    diffPng = b.replace(/-[a-z]+\.png$/, '-DIFF.png');
    fs.writeFileSync(diffPng, Buffer.from((r as any).url.split(',')[1], 'base64'));
  }
  return { differing: r.differing, total: r.total, sizeMatch: r.sizeMatch, diffPng };
}

fs.mkdirSync(SCREENS, { recursive: true });
fs.mkdirSync(OUT, { recursive: true });
const browser = await webkit.launch();
const judge = await (await chromium.launch()).newPage();
const report: any[] = [];
const errors: Record<string, string[]> = {};
for (const profile of Object.keys(PROFILES))
  for (const scheme of ['light', 'dark'] as const) {
    const [A, B] = builds;
    const ra = await capture(browser, A, profile, scheme);
    const rb = await capture(browser, B, profile, scheme);
    errors[`${profile}-${scheme}-${A.name}`] = ra.errors;
    errors[`${profile}-${scheme}-${B.name}`] = rb.errors;
    for (const step of STEPS) {
      const a = ra.out[step.name];
      const b = rb.out[step.name];
      const row: any = { profile, scheme, screen: step.name, errorA: a.error, errorB: b.error };
      if (a.png && b.png) Object.assign(row, await pixelDiff(judge, a.png, b.png));
      row.textSame = a.text === b.text;
      row.ariaSame = a.aria === b.aria;
      if (!row.textSame) row.textDiff = { [A.name]: a.text.slice(0, 2000), [B.name]: b.text.slice(0, 2000) };
      if (!row.ariaSame) row.ariaDiff = { [A.name]: a.aria, [B.name]: b.aria };
      report.push(row);
      console.log(`${profile}-${scheme} ${step.name}: pixels ${row.differing ?? '?'} text ${row.textSame ? 'same' : 'DIFF'} aria ${row.ariaSame ? 'same' : 'DIFF'}${a.error || b.error ? ' ERROR ' + (a.error || b.error) : ''}`);
    }
  }
const identical = report.filter((r) => r.differing === 0 && r.textSame && r.ariaSame).length;
fs.writeFileSync(path.join(OUT, 'parity.json'), JSON.stringify({ builds, fixedTime: new Date(FIXED).toISOString(), engine: 'webkit', screenshots: SCREENS, summary: { comparisons: report.length, identical }, report, pageErrors: errors }, null, 1));
console.log(`parity: ${identical} of ${report.length} comparisons identical`);
await browser.close();
await judge.context().browser()!.close();
