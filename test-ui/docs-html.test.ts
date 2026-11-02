// Raw HTML and Mermaid diagrams in a document, in a real browser: a README's centred header renders and
// its links work, diagrams are drawn, and nothing in the document runs or reaches out.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, test } from 'bun:test';
import { webkit, type Browser } from 'playwright';
import { closeWorkspace, herdr, startServer, testPane, type TestServer } from '../test/helpers.ts';
import { pageErrors } from './harness.ts';

let server: TestServer;
let browser: Browser;
let workspace: ReturnType<typeof testPane>;

const README = `<p align="center">
  <img src="docs/logo.png" width="64" alt="Logo" />
</p>

<h1 align="center">Project</h1>

<p align="center">
  <a href="https://example.invalid/badge"><img src="https://example.invalid/badge.svg" alt="Badge" /></a>
  <a href="docs/guide.md">Guide</a> · <a href="#usage">Usage</a>
</p>

<img src=x onerror="window.ran = 1"><script>window.ran = 2</script><svg onload="window.ran = 3"></svg>
<a id="evil" href="javascript:window.ran = 4" onclick="window.ran = 5">Bad link</a>
<details style="position:fixed;inset:0" class="sheet"><summary>More</summary>Hidden text</details>

${'Filler.\n\n'.repeat(80)}
## Usage

The end.
`;

const DIAGRAMS = `# Diagrams

\`\`\`mermaid
flowchart LR
    A["First<br/>box"] <-->|label| B["Second"]
\`\`\`

\`\`\`mermaid
%%{init: {"securityLevel": "loose", "htmlLabels": true, "themeCSS": "svg { background: url(https://example.invalid/css.png) }"}}%%
flowchart LR
    H["<img src='https://example.invalid/label.png' onerror='window.ran = 6'>Hostile<script>window.ran = 7</script>"] --> L["Linked"]
    click H href "javascript:window.ran = 8"
    click L href "https://example.invalid/link"
\`\`\`

\`\`\`mermaid
this is not a diagram (
\`\`\`
`;

beforeAll(async () => {
  server = await startServer({ pair: false });
  workspace = testPane('briardr-docs-html');
  herdr('pane', 'rename', workspace.pane, 'docs html target');
  fs.mkdirSync(path.join(workspace.dir, 'docs'), { recursive: true });
  fs.writeFileSync(path.join(workspace.dir, 'README.md'), README);
  fs.writeFileSync(path.join(workspace.dir, 'docs/guide.md'), '# The guide\n');
  fs.writeFileSync(path.join(workspace.dir, 'DIAGRAMS.md'), DIAGRAMS);
  // A 1×1 PNG.
  fs.writeFileSync(path.join(workspace.dir, 'docs/logo.png'), Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M/wHwAEBgIApD5fRAAAAABJRU5ErkJggg==', 'base64'));
  await server.run.herdr.refresh();
  browser = await webkit.launch();
});

afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await server?.close();
});

test('a README with raw HTML renders, and nothing in it runs or reaches out', async () => {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, ignoreHTTPSErrors: true, serviceWorkers: 'block' });
  try {
    const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, 'docs-html');
    assert.ok(paired.ok);
    await ctx.addCookies([{ name: '__Host-briardr', value: paired.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    const page = await ctx.newPage();
    const errors = pageErrors(page);
    const remote: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('example.invalid')) remote.push(r.url());
    });
    await page.goto(server.base + '/workspaces');
    await page.locator('.tree-streams .row-main', { hasText: 'docs html target' }).click();
    await page.getByRole('tab', { name: 'Docs', exact: true }).click();
    await page.locator('.doc-list .row-main', { hasText: 'README.md' }).click();
    const doc = page.locator('article.md');

    // The header: centred, with its local image loaded at the size it asks for.
    await doc.getByRole('heading', { name: 'Project', level: 1 }).waitFor();
    assert.equal(await doc.locator('h1').evaluate((h) => getComputedStyle(h).textAlign.replace('-webkit-', '')), 'center');
    const logo = doc.getByRole('img', { name: 'Logo' });
    await page.waitForFunction(() => (document.querySelector('article.md img') as HTMLImageElement | null)?.naturalWidth === 1);
    assert.equal((await logo.boundingBox())?.width, 64);

    // The rest of it: a remote image stays a label, and the document's handlers, scripts and styling are gone.
    await doc.getByText('Remote image not loaded: Badge').waitFor();
    assert.equal(await doc.locator('script, style, svg, [onerror], [onload], [onclick], [style], .sheet, #evil').count(), 0);
    await doc.getByRole('link', { name: 'Bad link' }).click();
    await doc.getByText('More', { exact: true }).click();
    await doc.getByText('Hidden text').waitFor();
    assert.equal(await page.evaluate(() => (window as any).ran), undefined);
    assert.deepEqual(remote, []);

    // An in-document link scrolls to its heading, and a link to another document opens it.
    const usage = doc.getByRole('heading', { name: 'Usage' });
    await doc.getByRole('link', { name: 'Usage' }).click();
    await page.waitForFunction(() => {
      const box = document.querySelector('#md-usage')?.getBoundingClientRect();
      return !!box && box.top >= 0 && box.bottom <= innerHeight;
    });
    assert.ok(await usage.isVisible());
    await doc.getByRole('link', { name: 'Guide' }).click();
    await doc.getByRole('heading', { name: 'The guide' }).waitFor();
    assert.deepEqual(errors, []);
  } finally {
    await ctx.close();
  }
});

test('Mermaid diagrams are drawn for the theme, hostile ones stay harmless, and bad ones stay code', async () => {
  const ctx = await browser.newContext({ viewport: { width: 1280, height: 800 }, colorScheme: 'light', ignoreHTTPSErrors: true, serviceWorkers: 'block' });
  try {
    const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, 'docs-diagrams');
    assert.ok(paired.ok);
    await ctx.addCookies([{ name: '__Host-briardr', value: paired.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
    const page = await ctx.newPage();
    const errors = pageErrors(page);
    const remote: string[] = [];
    page.on('request', (r) => {
      if (r.url().includes('example.invalid')) remote.push(r.url());
    });
    await page.goto(server.base + '/workspaces');
    await page.locator('.tree-streams .row-main', { hasText: 'docs html target' }).click();
    await page.getByRole('tab', { name: 'Docs', exact: true }).click();
    await page.locator('.doc-list .row-main', { hasText: 'DIAGRAMS.md' }).click();
    const doc = page.locator('article.md');
    const views = doc.locator('.md-diagram-view');
    await views.nth(1).locator('svg').waitFor();
    await page.waitForFunction(() => !document.querySelector('[id^="dmd-diagram"]'));

    // Two diagrams drawn, labels in place; the one Mermaid can't read is still there as code.
    assert.equal(await views.count(), 2);
    assert.match((await views.nth(0).textContent()) ?? '', /First[\s\S]*Second/);
    assert.match((await views.nth(1).textContent()) ?? '', /Linked/);
    await doc.locator('pre.md-diagram:visible', { hasText: 'this is not a diagram' }).waitFor();
    assert.equal(await doc.locator('pre.md-diagram:visible').count(), 1);

    // The hostile one: nothing ran, nothing was fetched, no links, and its settings were ignored.
    assert.equal(await views.locator('a, script, foreignObject, [onerror], [onclick]').count(), 0);
    await views.nth(1).click();
    assert.equal(await page.evaluate(() => (window as any).ran), undefined);
    assert.deepEqual(remote, []);

    // A change of theme draws them again, in place; so does coming back from the source view.
    const fill = () => views.nth(0).locator('rect, path.basic').first().evaluate((n) => getComputedStyle(n).fill);
    const light = await fill();
    await page.emulateMedia({ colorScheme: 'dark' });
    await page.waitForFunction((before) => {
      const node = document.querySelector('.md-diagram-view rect, .md-diagram-view path.basic');
      return !!node && getComputedStyle(node).fill !== before;
    }, light);
    assert.equal(await views.count(), 2);
    await page.getByRole('button', { name: 'Source' }).click();
    await page.locator('.md-source').waitFor();
    await page.getByRole('button', { name: 'Rendered' }).click();
    await views.nth(1).locator('svg').waitFor();
    assert.equal(await views.count(), 2);
    assert.deepEqual(errors, []);
  } finally {
    await ctx.close();
  }
});
