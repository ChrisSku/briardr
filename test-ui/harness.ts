// Browser harness for UI checks: launches WebKit (Safari's engine) or Chromium at device sizes,
// pairs through the real pairing screen, and saves screenshots.
import { execFileSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { chromium, devices, webkit, type Browser, type BrowserContext, type Page } from 'playwright';

export const BASE = process.env.BRIARDR_URL || 'https://localhost:17443';
export const DATA_DIR = process.env.BRIARDR_DATA_DIR || path.resolve('.dev-data');
export const SHOTS = process.env.BRIARDR_SHOTS || path.resolve('test-results/screens');

export type Profile = 'iphone' | 'iphone-landscape' | 'ipad' | 'ipad-split' | 'mac';

/** The server binary: `BRIARDR_SERVER_BIN`, else server/target/debug/briardr (`mise run build:test-server`). */
export const SERVER_BIN = process.env.BRIARDR_SERVER_BIN || path.resolve('server/target/debug/briardr');

/** The command for a server subcommand, for tests that start their own server. */
export function serverArgv(cmd: string): [string, string[]] {
  return [SERVER_BIN, [cmd]];
}

// WebKit logs a fetch that a navigation cancels as "Fetch API cannot load https://host/ due to access control
// checks.", and Playwright reports that as a page error. The page's update check (GET /, which catches
// its own failures) is often in flight when a test navigates, so that one isn't the page's error.
const CANCELLED_UPDATE_CHECK = /^Fetch API cannot load https:\/\/[^/\s]+\/ due to access control checks\./;

/** The page's uncaught errors, collected as they happen. */
export function pageErrors(page: Page): string[] {
  const errors: string[] = [];
  page.on('pageerror', (e) => {
    if (!CANCELLED_UPDATE_CHECK.test(e.stack ?? '')) errors.push(e.message);
  });
  return errors;
}

/** A pairing code for the dev server's data dir (`BRIARDR_PAIR_CMD` replaces the command). */
export function pairingCode(): string {
  const env = { ...process.env, BRIARDR_DATA_DIR: DATA_DIR };
  const out = process.env.BRIARDR_PAIR_CMD
    ? execFileSync('sh', ['-c', process.env.BRIARDR_PAIR_CMD], { env, encoding: 'utf8' })
    : execFileSync(...serverArgv('pair'), { env, encoding: 'utf8' });
  const m = out.match(/[A-Z0-9]{4}-[A-Z0-9]{4}/);
  if (!m) throw new Error('no pairing code');
  return m[0];
}

export async function launch(profile: Profile, scheme: 'light' | 'dark' = 'light'): Promise<{ browser: Browser; ctx: BrowserContext; page: Page }> {
  const engine = profile === 'mac' ? webkit : webkit;
  const browser = await engine.launch();
  const base = { ignoreHTTPSErrors: true, colorScheme: scheme, locale: 'en-US' } as const;
  let opts: any;
  switch (profile) {
    case 'iphone':
      opts = { ...devices['iPhone 15 Pro'], ...base };
      break;
    case 'iphone-landscape':
      opts = { ...devices['iPhone 15 Pro landscape'], ...base };
      break;
    case 'ipad':
      opts = { ...devices['iPad Pro 11'], ...base };
      break;
    case 'ipad-split':
      opts = { ...devices['iPad Pro 11'], viewport: { width: 507, height: 1194 }, ...base };
      break;
    default:
      opts = { viewport: { width: 1440, height: 900 }, deviceScaleFactor: 2, ...base };
  }
  const ctx = await browser.newContext(opts);
  const page = await ctx.newPage();
  return { browser, ctx, page };
}

export async function pair(page: Page, name = 'UI test'): Promise<void> {
  await page.goto(BASE + '/');
  await page.getByLabel('Pairing code').fill(pairingCode());
  await page.getByLabel('Name for this device').fill(name);
  await page.getByRole('button', { name: 'Sign in' }).click();
  await page.waitForURL(/\/agents/);
  await page.waitForSelector('.count-strip');
}

export async function shot(page: Page, name: string): Promise<string> {
  fs.mkdirSync(SHOTS, { recursive: true });
  const file = path.join(SHOTS, `${name}.png`);
  await page.screenshot({ path: file });
  return file;
}

export { chromium, webkit };
