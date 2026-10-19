// Captures the main screens at iPhone, iPad and Mac sizes, in light and dark mode, for review.
// Run with the dev server up (mise run dev):  bun test-ui/screens.ts [profile] [scheme]
import { BASE, launch, pair, shot, type Profile } from './harness.ts';

const profiles = (process.argv[2] ? [process.argv[2]] : ['iphone', 'ipad', 'mac']) as Profile[];
const schemes = (process.argv[3] ? [process.argv[3]] : ['light', 'dark']) as ('light' | 'dark')[];
const STREAM = process.env.BRIARDR_STREAM || 'w1:p2';

for (const profile of profiles)
  for (const scheme of schemes) {
    const { browser, page } = await launch(profile, scheme);
    const errors: string[] = [];
    page.on('pageerror', (e) => errors.push(e.message));
    await pair(page, `${profile}-${scheme}`);
    await page.waitForTimeout(700);
    const tag = `${profile}-${scheme}`;
    await shot(page, `${tag}-1-agents`);
    await page.goto(BASE + '/workspaces');
    await page.waitForTimeout(500);
    await shot(page, `${tag}-2-workspaces`);
    // Open a stream the way a user does (from the list), so it attaches without Resume.
    await page.goto(BASE + '/agents');
    await page.waitForTimeout(500);
    await page.locator('.stream-row .row-main', { hasText: 'refactor auth' }).first().click();
    await page.waitForTimeout(1500);
    await shot(page, `${tag}-3-stream`);
    await page.getByRole('tab', { name: 'Changes' }).click();
    await page.waitForTimeout(1200);
    await page.locator('.file-toggle').first().click().catch(() => {});
    await page.waitForTimeout(800);
    await shot(page, `${tag}-4-changes`);
    await page.getByRole('tab', { name: 'Docs' }).click();
    await page.waitForTimeout(800);
    await page.locator('.doc-list .row-main', { hasText: 'ARCH.md' }).click().catch(() => {});
    await page.waitForTimeout(800);
    await shot(page, `${tag}-5-docs`);
    await page.goto(BASE + '/usage');
    await page.waitForTimeout(1200);
    await shot(page, `${tag}-6-usage`);
    await page.goto(BASE + '/settings');
    await page.waitForTimeout(800);
    await shot(page, `${tag}-7-settings`);
    if (errors.length) console.log(tag, 'page errors:', errors);
    console.log('captured', tag, STREAM);
    await browser.close();
  }
