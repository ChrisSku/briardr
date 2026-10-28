import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { afterAll, beforeAll, test } from 'bun:test';
import { devices, webkit, type Browser } from 'playwright';
import { closeWorkspace, herdr, startServer, testPane, type TestServer } from '../test/helpers.ts';
import { pageErrors } from './harness.ts';

let server: TestServer;
let browser: Browser;
let workspace: ReturnType<typeof testPane>;

/** A minimal PDF with one line of text per page. */
function pdf(pages: string[]): Buffer {
  const objects = ['<< /Type /Catalog /Pages 2 0 R >>', `<< /Type /Pages /Kids [${pages.map((_, i) => `${4 + 2 * i} 0 R`).join(' ')}] /Count ${pages.length} >>`, '<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>'];
  pages.forEach((text, i) => {
    const content = `BT /F1 48 Tf 72 680 Td (${text}) Tj ET`;
    objects.push(`<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 3 0 R >> >> /Contents ${5 + 2 * i} 0 R >>`);
    objects.push(`<< /Length ${content.length} >>\nstream\n${content}\nendstream`);
  });
  let out = '%PDF-1.4\n';
  const offsets = objects.map((o, i) => {
    const at = out.length;
    out += `${i + 1} 0 obj\n${o}\nendobj\n`;
    return at;
  });
  const xref = out.length;
  out += `xref\n0 ${objects.length + 1}\n0000000000 65535 f \n${offsets.map((o) => `${String(o).padStart(10, '0')} 00000 n \n`).join('')}`;
  out += `trailer\n<< /Size ${objects.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return Buffer.from(out, 'latin1');
}

beforeAll(async () => {
  server = await startServer({ pair: false });
  workspace = testPane('briardr-files-pdf');
  herdr('pane', 'rename', workspace.pane, 'pdf viewer target');
  fs.writeFileSync(path.join(workspace.dir, 'paper.pdf'), pdf(['First page', 'Second page']));
  fs.writeFileSync(path.join(workspace.dir, 'broken.pdf'), 'not a pdf');
  await server.run.herdr.refresh();
  browser = await webkit.launch();
});

afterAll(async () => {
  await browser?.close();
  if (workspace) closeWorkspace(workspace.workspace);
  await server?.close();
});

for (const profile of ['iphone', 'mac'] as const) {
  test(`Files shows a PDF's pages and zooms them (${profile})`, async () => {
    const ctx = await browser.newContext({
      ...(profile === 'iphone' ? devices['iPhone 15 Pro'] : { viewport: { width: 1280, height: 800 } }),
      ignoreHTTPSErrors: true,
      serviceWorkers: 'block',
    });
    try {
      const paired = server.run.auth.pair(server.run.auth.createPairingCode().code, `files-pdf-${profile}`);
      assert.ok(paired.ok);
      await ctx.addCookies([{ name: '__Host-briardr', value: paired.cookie, domain: 'localhost', path: '/', secure: true, httpOnly: true, sameSite: 'Strict' }]);
      const page = await ctx.newPage();
      const errors = pageErrors(page);
      const refused: string[] = [];
      page.on('console', (m) => /Content Security Policy|Refused to/i.test(m.text()) && refused.push(m.text()));
      await page.goto(server.base + '/workspaces');
      await page.locator('.tree-streams .row-main', { hasText: 'pdf viewer target' }).click();
      await page.getByRole('tab', { name: 'Files', exact: true }).click();
      await page.getByRole('button', { name: 'paper.pdf', exact: true }).click();
      await page.locator('.fv-meta', { hasText: '2 pages' }).waitFor();
      const first = page.getByRole('img', { name: 'Page 1 of 2' });
      await first.locator('canvas').waitFor();
      // The text was drawn: the page isn't blank.
      const inked = await first.locator('canvas').evaluate((c: HTMLCanvasElement) => {
        const d = c.getContext('2d')!.getImageData(0, 0, c.width, c.height).data;
        let n = 0;
        for (let i = 0; i < d.length; i += 4) if (d[i] < 128) n++;
        return n;
      });
      assert.ok(inked > 100, `page 1 has ink (${inked} dark pixels)`);
      const before = (await first.boundingBox())!.width;
      const pane = (await page.locator('.fv-pdf').boundingBox())!.width;
      assert.ok(before <= pane - 32, `page 1 (${before}px) fits the pane (${pane}px)`);
      assert.equal((await first.locator('canvas').boundingBox())!.width, before);
      await page.getByRole('button', { name: 'Zoom in', exact: true }).click();
      await page.locator('.fv-meta', { hasText: '125%' }).waitFor();
      const after = (await first.boundingBox())!.width;
      assert.ok(Math.abs(after / before - 1.25) < 0.02, `zoomed from ${before} to ${after}`);
      fs.mkdirSync('test-results/files-pdf', { recursive: true });
      await page.screenshot({ path: `test-results/files-pdf/${profile}.png` });

      if (profile === 'iphone') await page.getByRole('button', { name: 'Back to folder' }).click();
      await page.getByRole('button', { name: 'broken.pdf', exact: true }).click();
      await page.getByRole('alert').filter({ hasText: 'Couldn’t read this PDF.' }).waitFor();
      assert.deepEqual(errors, []);
      assert.deepEqual(refused, []);
    } finally {
      await ctx.close();
    }
  });
}
