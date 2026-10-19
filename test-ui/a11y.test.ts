// Accessibility audit (axe-core) of every main screen, light and dark, at iPhone and Mac sizes.
// Fails on serious or critical violations (including colour contrast).
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { devices, webkit, type Browser, type Page } from 'playwright';
import { closeWorkspace, herdr, startServer, testPane, type TestServer } from '../test/helpers.ts';

const axeSource = fs.readFileSync(path.resolve('node_modules/axe-core/axe.min.js'), 'utf8');
let s: TestServer;
let browser: Browser;
let ws: { pane: string; workspace: string; dir: string };

beforeAll(async () => {
  s = await startServer({ pair: false });
  ws = testPane('briardr-a11y');
  herdr('pane', 'rename', ws.pane, 'a11y target');
  herdr('pane', 'report-agent', ws.pane, '--source', 'custom:briardr-a11y', '--agent', 'claude', '--state', 'blocked');
  fs.writeFileSync(path.join(ws.dir, 'README.md'), '# Title\n\n| a | b |\n|---|---|\n| 1 | 2 |\n\n- [x] done\n');
  await s.run.herdr.refresh();
  browser = await webkit.launch();
});
afterAll(async () => {
  await browser?.close();
  herdr('pane', 'release-agent', ws.pane, '--source', 'custom:briardr-a11y', '--agent', 'claude');
  closeWorkspace(ws.workspace);
  await s.close();
});

async function audit(page: Page, label: string): Promise<string[]> {
  await page.addScriptTag({ content: axeSource });
  const res = await page.evaluate(async () => {
    const r = await (window as any).axe.run(document, { resultTypes: ['violations'], rules: { region: { enabled: false } } });
    return r.violations.filter((v: any) => v.impact === 'serious' || v.impact === 'critical').map((v: any) => `${v.id} (${v.impact}): ${v.nodes.slice(0, 3).map((n: any) => n.target.join(' ')).join(' | ')}`);
  });
  return res.map((r: string) => `${label}: ${r}`);
}

for (const profile of ['iphone', 'mac'] as const)
  for (const scheme of ['light', 'dark'] as const)
    describe(`accessibility ${profile} ${scheme}`, () => {
      test('no serious or critical violations on any screen', async () => {
        const opts = profile === 'iphone' ? devices['iPhone 15 Pro'] : { viewport: { width: 1280, height: 800 } };
        const ctx = await browser.newContext({ ...opts, colorScheme: scheme, ignoreHTTPSErrors: true, bypassCSP: true }); // CSP blocks injected scripts, including axe
        const page = await ctx.newPage();
        const problems: string[] = [];
        await page.goto(s.base + '/');
        await page.getByLabel('Pairing code').waitFor();
        problems.push(...(await audit(page, 'pair')));
        await page.getByLabel('Pairing code').fill(s.run.auth.createPairingCode().code);
        await page.getByRole('button', { name: 'Sign in' }).click();
        await page.waitForSelector('.count-strip');
        await page.waitForSelector('.stream-row');
        problems.push(...(await audit(page, 'agents')));
        for (const [p, sel] of [['/workspaces', '.tree'], ['/usage', '.usage-card'], ['/settings', '.set-section']] as const) {
          await page.goto(s.base + p);
          await page.waitForSelector(sel);
          await page.waitForTimeout(400);
          problems.push(...(await audit(page, p)));
        }
        await page.goto(s.base + '/agents');
        await page.locator('.stream-row .row-main', { hasText: 'a11y target' }).click();
        await page.waitForSelector('[data-ownership=controlling]');
        problems.push(...(await audit(page, 'terminal')));
        await page.getByRole('tab', { name: 'Files' }).click();
        await page.locator('.ft-row', { hasText: 'README.md' }).first().click();
        await page.waitForSelector('.code .cl');
        problems.push(...(await audit(page, 'files')));
        await page.getByRole('tab', { name: 'Docs' }).click();
        await page.locator('.doc-list .row-main').first().click();
        await page.waitForSelector('.md');
        problems.push(...(await audit(page, 'docs')));
        await page.getByRole('tab', { name: 'Changes' }).click();
        await page.waitForTimeout(600);
        problems.push(...(await audit(page, 'changes')));
        await page.getByRole('tab', { name: 'Notes' }).click();
        await page.getByRole('button', { name: 'New note' }).click();
        await page.getByLabel('Note for a11y target').fill('## Plan\n- check retries');
        await page.waitForTimeout(1200);
        await page.screenshot({ path: `test-results/screens/${profile}-${scheme}-8-notes.png` });
        problems.push(...(await audit(page, 'notes')));
        await page.getByRole('button', { name: 'All notes' }).click();
        await page.waitForSelector('.note-card');
        await page.screenshot({ path: `test-results/screens/${profile}-${scheme}-9-notes-list.png` });
        problems.push(...(await audit(page, 'notes list')));
        await ctx.close();
        assert.deepEqual(problems, [], problems.join('\n'));
      });
    });
