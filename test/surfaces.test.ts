// Docs, uploads, preview, creating work, and stream actions, end to end against the test run's Herdr session.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import http from 'node:http';
import https from 'node:https';
import path from 'node:path';
import { afterAll, beforeAll, describe, test } from 'bun:test';
import { closeWorkspace, herdr, request, sleep, startServer, testPane, tmpDir, type TestServer } from './helpers.ts';

let s: TestServer;
let pane: string;
let workspace: string;
let dir: string;
beforeAll(async () => {
  s = await startServer();
  ({ pane, workspace, dir } = testPane('briardr-test-surfaces'));
  fs.mkdirSync(path.join(dir, 'docs'));
  fs.writeFileSync(path.join(dir, 'README.md'), '# Readme\n\nSee [guide](docs/guide.md).\n\n![logo](docs/logo.png)\n\n![remote](https://example.com/x.png)\n');
  fs.writeFileSync(path.join(dir, 'docs', 'guide.md'), '# Guide\n\n- [x] done\n- [ ] todo\n\n| a | b |\n|---|---|\n| 1 | 2 |\n');
  fs.copyFileSync(path.resolve('web/public/icons/icon-192.png'), path.join(dir, 'docs', 'logo.png'));
  fs.writeFileSync(path.join(dir, 'CHANGELOG.md'), '# Changes\n');
  await s.run.herdr.refresh();
});
afterAll(async () => {
  closeWorkspace(workspace);
  await s.close();
  if (process.env.BRIARDR_DEBUG_HANDLES) setTimeout(() => console.log('HANDLES', (process as any)._getActiveHandles().map((h: any) => h.constructor.name + (h._idleTimeout ? ':' + h._idleTimeout : '') + (h.remotePort ? ':' + h.remotePort : '') + (h.localPort ? ':' + h.localPort : ''))), 500);
});

const api = (m: string, p: string, b?: unknown) => request(s, m, p, b);
const enc = encodeURIComponent;

describe('docs', () => {
  test('files outside the stream’s folder cannot be read, including through symlinks', async () => {
    const secret = path.join(tmpDir(), 'secret.md');
    fs.writeFileSync(secret, '# secret');
    fs.symlinkSync(secret, path.join(dir, 'link.md'));
    try {
      const file = (rel: string) => api('GET', `/api/streams/${enc(pane)}/docs/file?path=${enc(rel)}`);
      assert.equal((await file('link.md')).status, 403);
      assert.ok([403, 404].includes((await file('../../etc/passwd.md')).status));
      assert.ok([403, 404].includes((await file(secret)).status), 'an absolute path outside the folder');
      const asset = await api('GET', `/api/streams/${enc(pane)}/docs/asset?path=${enc('link.md')}`);
      assert.ok([403, 404].includes(asset.status));
    } finally {
      fs.rmSync(path.join(dir, 'link.md'));
    }
  });

  test('enforces a per-file size limit', async () => {
    fs.writeFileSync(path.join(dir, 'big.md'), 'x'.repeat(1024 * 1024 + 10));
    const r = await api('GET', `/api/streams/${enc(pane)}/docs/file?path=${enc('big.md')}`);
    assert.equal(r.status, 413);
    fs.rmSync(path.join(dir, 'big.md'));
  });

  test('serves a document and its relative images through the authenticated API', async () => {
    const d = await api('GET', `/api/streams/${enc(pane)}/docs/file?path=README.md`);
    assert.equal(d.status, 200);
    assert.match(d.json.text, /See \[guide\]/);
    const img = await api('GET', `/api/streams/${enc(pane)}/docs/asset?path=${enc('docs/logo.png')}`);
    assert.equal(img.status, 200);
    assert.equal(img.headers['content-type'], 'image/png');
    const unauth = await request(s, 'GET', `/api/streams/${enc(pane)}/docs/asset?path=${enc('docs/logo.png')}`, undefined, { cookie: false });
    assert.equal(unauth.status, 401);
  });

  test('Markdown rendering escapes HTML, blocks remote images and keeps relative links inside Docs', async () => {
    const { renderMarkdown } = await import('../web/src/markdown.ts');
    const html = renderMarkdown('<script>alert(1)</script>\n\n![x](https://evil/x.png) ![y](img/a.png) [g](docs/guide.md) [w](https://example.com)\n\n- [x] done', { docPath: 'README.md', streamId: 'w1:p1' });
    assert.ok(!html.includes('<script>'));
    assert.match(html, /Remote image not loaded/);
    assert.match(html, /\/api\/streams\/w1%3Ap1\/docs\/asset\?path=img%2Fa\.png/);
    assert.match(html, /data-doc="docs\/guide\.md"/);
    assert.match(html, /target="_blank"/);
    assert.match(html, /type="checkbox" disabled checked/);
  });

  test('Markdown keeps safe raw HTML, the way a README uses it', async () => {
    const { renderMarkdown } = await import('../web/src/markdown.ts');
    const render = (src: string) => renderMarkdown(src, { docPath: 'docs/README.md', streamId: 'w1:p1' });
    const head = render('<p align="center">\n  <img src="logo.png" width="112" alt="Logo" />\n</p>\n\n<h1 align="center">Name</h1>\n\n<a href="guide.md">Guide</a> · <a href="#usage">Usage</a> · <a href="https://example.com/?a=1&amp;b=2"><img src="https://evil/badge.svg" alt="Badge" /></a>\n');
    assert.match(head, /<p align="center">/);
    assert.match(head, /<img src="\/api\/streams\/w1%3Ap1\/docs\/asset\?path=docs%2Flogo\.png" alt="Logo" width="112"/);
    assert.match(head, /<h1 align="center">Name<\/h1>/);
    assert.match(head, /<a href="#" data-doc="docs\/guide\.md">Guide<\/a>/);
    assert.match(head, /<a href="#usage" data-anchor="usage">Usage<\/a> ·/);
    assert.match(head, /href="https:\/\/example\.com\/\?a=1&amp;b=2" target="_blank" rel="noopener noreferrer"/);
    assert.match(head, /aria-label="Remote image not loaded: Badge" title="Remote image not loaded: Badge">Badge<\/span>/);
    assert.match(render('![](https://evil/x.png)'), />Remote image not loaded<\/span>/);
    assert.match(render('![a"><b>](https://evil/x.png)'), /aria-label="Remote image not loaded: a&quot;&gt;&lt;b&gt;"[^<]*>a&quot;&gt;&lt;b&gt;<\/span>/);
    assert.ok(!head.includes('evil'));
    assert.match(render('<details open><summary>More</summary>\n\n**Inner**\n\n</details>\n\nPress <kbd>Esc</kbd><br>\n\n<a name="here"></a>'), /<details open="">[\s\S]*<strong>Inner<\/strong>[\s\S]*<kbd>Esc<\/kbd><br>[\s\S]*<a id="md-here">/);
    assert.match(render('<p>a&nbsp;b &amp; c & d</p>'), /<p>a&nbsp;b &amp; c &amp; d<\/p>/);
    assert.match(render('<picture><source srcset="https://evil/a.png"><img src="a.png"></picture>'), /<picture><img src="[^"]*asset[^"]*"[^>]*><\/picture>/);
    assert.match(render('A Vec<T> and <video src="a.mp4"></video>'), /Vec&lt;T&gt; and &lt;video src="a\.mp4"&gt;/);
  });

  test('Markdown leaves Mermaid diagrams as code blocks to draw, and other code alone', async () => {
    const { renderMarkdown } = await import('../web/src/markdown.ts');
    const html = renderMarkdown('```mermaid\nflowchart LR\n  A["<b>x</b>"] --> B\n```\n\n```ts\nmermaid\n```', { docPath: 'README.md', streamId: 'w1:p1' });
    assert.match(html, /<pre class="md-diagram"><code>flowchart LR\n  A\[&quot;&lt;b&gt;x&lt;\/b&gt;&quot;\] --&gt; B\n<\/code><\/pre>/);
    assert.match(html, /<pre><code class="language-ts">mermaid/);
  });

  test('Markdown raw HTML can never run or reach out', async () => {
    const { renderMarkdown } = await import('../web/src/markdown.ts');
    const attacks = [
      '<script>alert(1)</script>',
      'text <script>alert(1)</script> more',
      '<SCRIPT SRC=https://evil/x.js></SCRIPT>',
      '<script>alert(1)',
      '<style>body{display:none}</style>',
      '<img src=x onerror=alert(1)>',
      '<img src="x"onerror=alert(1)>',
      '<img src=x/onerror=alert(1)>',
      '<img/src=x/onerror=alert(1)>',
      '<img srcset="https://evil/a.png 1x" src="a.png">',
      '<p onclick="alert(1)" style="position:fixed" class="sheet" id="app">x</p>',
      '<a href="javascript:alert(1)">x</a>',
      '<a href="JaVaScRiPt:alert(1)">x</a>',
      '<a href="java&#115;cript:alert(1)">x</a>',
      '<a href="&#106;avascript:alert(1)">x</a>',
      '<a href="  javascript:alert(1)">x</a>',
      '<a href="java\tscript:alert(1)">x</a>',
      '<a href="data:text/html,<script>alert(1)</script>">x</a>',
      '<a href="vbscript:x" target="_top">x</a>',
      '<a href="//evil/x">x</a>',
      '<img src="//evil/x.png">',
      '<img src="data:image/svg+xml,<svg onload=alert(1)>">',
      '<iframe src="https://evil"></iframe>',
      '<object data="https://evil"></object>',
      '<embed src="https://evil">',
      '<svg onload=alert(1)><script>alert(1)</script></svg>',
      '<math><mtext></form><style><img src=x onerror=alert(1)>',
      '<form action="https://evil"><input name=a><button>Go</button></form>',
      '<textarea><img src=x onerror=alert(1)></textarea>',
      '<title><img src=x onerror=alert(1)></title>',
      '<base href="https://evil/">',
      '<meta http-equiv="refresh" content="0;url=https://evil">',
      '<link rel="stylesheet" href="https://evil/x.css">',
      '<!-- x --><img src=x onerror=alert(1)><!-- y -->',
      '<!-- <img src=x onerror=alert(1)',
      '<scr<!-- -->ipt>alert(1)</scr<!-- -->ipt>',
      '<<script>script>alert(1)<</script>/script>',
      '<div title="&quot; onmouseover=&quot;alert(1)">x</div>',
      '<div title=\'"><script>alert(1)</script>\'>x</div>',
      '<img alt="&quot;><script>alert(1)</script>" src="a.png">',
      '<a href="x" href="javascript:alert(1)" onclick=alert(1)>x</a>',
      '<a href="#&quot; onclick=&quot;alert(1)">x</a>',
      '<div\nonclick=alert(1)\n>x</div>',
      '</article><script>alert(1)</script>',
      '<?php echo 1 ?><![CDATA[<script>alert(1)</script>]]><!DOCTYPE html>',
    ];
    for (const attack of attacks) {
      for (const src of [attack, `Some text ${attack} and more`, `- item\n\n  ${attack}\n\n| a |\n|---|\n| ${attack} |`]) {
        const html = renderMarkdown(src, { docPath: 'README.md', streamId: 'w1:p1' });
        // Each real tag, as a browser would see it: only allowed elements, nothing active from the
        // document, and a URL only where Briardr put one.
        for (const [tag, name, attrs] of html.matchAll(/<\/?([a-z][a-z0-9-]*)((?:"[^"]*"|[^">])*)>/gi)) {
          assert.ok(!/^(script|style|iframe|object|embed|svg|math|form|input|button|textarea|title|base|meta|link|article|source)$/i.test(name), `${attack} made ${tag}`);
          const bare = attrs.replace(/"[^"]*"/g, '""');
          assert.ok(!/\s(on\w+|style|srcset|action|formaction)\b/i.test(bare), `${attack} made ${tag}`);
          assert.ok(!/\sclass=/.test(attrs) || /class="(md-table|md-remote-img)"/.test(attrs), `${attack} made ${tag}`);
          assert.ok(!/\sid=/.test(attrs) || /\sid="md-/.test(attrs), `${attack} made ${tag}`);
          for (const [, url] of attrs.matchAll(/\s(?:href|src)="([^"]*)"/gi)) {
            assert.match(url, name === 'img' ? /^\/api\/streams\// : /^(#|https?:\/\/)/, `${attack} made ${tag}`);
          }
        }
      }
    }
  });
});

describe('files', () => {
  test('lists one folder at a time, folders first, with changed marks and no .git', async () => {
    fs.mkdirSync(path.join(dir, 'src'), { recursive: true });
    fs.writeFileSync(path.join(dir, 'src', 'main.rs'), 'fn main() {}\n');
    const top = await api('GET', `/api/streams/${enc(pane)}/files`);
    assert.equal(top.status, 200);
    const names = top.json.entries.map((e: any) => e.name);
    assert.ok(names.indexOf('docs') < names.indexOf('README.md'), 'folders come first');
    assert.ok(!names.includes('.git'));
    const src = await api('GET', `/api/streams/${enc(pane)}/files?path=src`);
    assert.deepEqual(src.json.entries.map((e: any) => [e.path, e.dir]), [['src/main.rs', false]]);
  });

  test('reads text, flags binary files, and refuses paths outside the folder', async () => {
    const f = await api('GET', `/api/streams/${enc(pane)}/files/file?path=${enc('src/main.rs')}`);
    assert.equal(f.status, 200);
    assert.equal(f.json.text, 'fn main() {}\n');
    assert.equal(f.json.binary, false);
    const png = await api('GET', `/api/streams/${enc(pane)}/files/file?path=${enc('docs/logo.png')}`);
    assert.equal(png.json.binary, true);
    assert.equal(png.json.text, null);
    const secret = path.join(tmpDir(), 'secret.txt');
    fs.writeFileSync(secret, 'secret');
    fs.symlinkSync(secret, path.join(dir, 'link.txt'));
    try {
      assert.equal((await api('GET', `/api/streams/${enc(pane)}/files/file?path=link.txt`)).status, 403);
      assert.equal((await api('GET', `/api/streams/${enc(pane)}/files?path=..`)).status, 403);
      const top = await api('GET', `/api/streams/${enc(pane)}/files`);
      assert.ok(!top.json.entries.some((e: any) => e.name === 'link.txt'), 'escaping links are not listed');
    } finally {
      fs.rmSync(path.join(dir, 'link.txt'));
    }
    const unauth = await request(s, 'GET', `/api/streams/${enc(pane)}/files`, undefined, { cookie: false });
    assert.equal(unauth.status, 401);
  });

  test('highlighting splits into lines and reopens spans that cross a line break', async () => {
    const { highlightLines, splitLines } = await import('../plugins/files/src/highlight.ts');
    assert.deepEqual(splitLines('<span class="a">x\n<span class="b">y\nz</span></span>w'), [
      '<span class="a">x</span>',
      '<span class="a"><span class="b">y</span></span>',
      '<span class="a"><span class="b">z</span></span>w',
    ]);
    const ts = highlightLines('const a = "<b>";\n// done\n', 'typescript');
    assert.equal(ts.lines.length, 2, 'a final newline adds no line');
    assert.match(ts.lines[0], /hljs-keyword/);
    assert.ok(!ts.lines[0].includes('<b>'), 'text is escaped');
    assert.deepEqual(highlightLines('a<b\nc', null), { lines: ['a&lt;b', 'c'], highlighted: false });
  });
});

describe('uploads', () => {
  test('images go to a private, uniquely named folder and must be PNG within limits', async () => {
    const png = fs.readFileSync(path.resolve('web/public/icons/icon-192.png'));
    const a = await request(s, 'POST', `/api/streams/${enc(pane)}/uploads?kind=image&name=${enc("my photo's.heic")}`, undefined, { raw: png });
    const b = await request(s, 'POST', `/api/streams/${enc(pane)}/uploads?kind=image&name=photo.png`, undefined, { raw: png });
    assert.equal(a.status, 200);
    assert.notEqual(path.dirname(a.json.path), path.dirname(b.json.path), 'unique folder per upload');
    assert.equal(fs.statSync(path.dirname(a.json.path)).mode & 0o777, 0o700);
    assert.equal(fs.statSync(a.json.path).mode & 0o777, 0o600);
    assert.ok(a.json.path.endsWith("my photo's.png"));
    assert.match(a.json.quoted, /^'.*'\\''s\.png'$/, 'quoted safely for a single paste');
    const notPng = await request(s, 'POST', `/api/streams/${enc(pane)}/uploads?kind=image&name=x.png`, undefined, { raw: Buffer.from('GIF89a......................') });
    assert.equal(notPng.status, 415);
  });

  test('a cancelled upload removes only its own partial file', async () => {
    const root = path.join(s.dataDir, 'briardr-uploads');
    const before = new Set(fs.existsSync(root) ? fs.readdirSync(root) : []);
    await new Promise<void>((resolve) => {
      const req = https.request(`https://127.0.0.1:${s.port}/api/streams/${enc(pane)}/uploads?kind=file&name=big.bin`, {
        method: 'POST',
        rejectUnauthorized: false,
        headers: { host: `localhost:${s.port}`, origin: s.base, cookie: s.cookie, 'content-length': String(50 * 1024 * 1024) },
      });
      req.on('error', () => resolve());
      req.write(Buffer.alloc(256 * 1024, 1));
      setTimeout(() => {
        req.destroy();
        resolve();
      }, 300);
    });
    await sleep(500);
    const after = fs.readdirSync(root).filter((d) => !before.has(d));
    assert.deepEqual(after, [], 'no leftover folder or partial file');
    for (const d of before) assert.ok(fs.existsSync(path.join(root, d)), 'other uploads untouched');
  });
});

describe('preview', () => {
  test('nothing listening gives a clear error', async () => {
    const r = await api('POST', '/api/previews', { port: 1 });
    assert.equal(r.status, 404);
    assert.equal(r.json.message, 'Nothing is listening on port 1.');
  });

  test('proxies the dev server through Briardr’s authenticated connection and releases it on close', async () => {
    const seen: { cookie?: string; host?: string; url?: string }[] = [];
    const dev = http.createServer((req, res) => {
      seen.push({ cookie: req.headers.cookie, host: req.headers.host, url: req.url });
      res.setHeader('x-frame-options', 'DENY');
      res.end(`hello from dev ${req.url}`);
    });
    await new Promise<void>((r) => dev.listen(0, '127.0.0.1', () => r()));
    const port = (dev.address() as any).port;
    const r = await api('POST', '/api/previews', { port });
    assert.equal(r.status, 200);
    const lp = r.json.preview.listenPort;
    const get = (cookie: string | null, p = '/app?x=1') =>
      new Promise<{ status: number; body: string; xfo?: string }>((resolve, reject) => {
        const req = https.request(`https://127.0.0.1:${lp}${p}`, { rejectUnauthorized: false, headers: { host: `localhost:${lp}`, ...(cookie ? { cookie } : {}) } }, (res) => {
          let b = '';
          res.on('data', (c) => (b += c));
          res.on('end', () => resolve({ status: res.statusCode || 0, body: b, xfo: res.headers['x-frame-options'] as string }));
        });
        req.on('error', reject);
        req.end();
      });
    assert.equal((await get(null)).status, 401, 'preview port requires the device cookie too');
    const ok = await get(`${s.cookie}; theme=dark`);
    assert.equal(ok.status, 200);
    assert.equal(ok.body, 'hello from dev /app?x=1', 'path and query preserved');
    assert.equal(ok.xfo, undefined, 'can be framed by Briardr');
    assert.equal(seen.at(-1)!.cookie, 'theme=dark', 'Briardr’s cookie is never forwarded to the dev server');
    assert.equal(seen.at(-1)!.host, `localhost:${port}`);
    const c = await api('DELETE', `/api/previews/${r.json.preview.id}`);
    assert.equal(c.json.ok, true);
    await sleep(200);
    await assert.rejects(get(s.cookie), 'listener closed with the preview');
    dev.close();
  });
});

describe('creating work and stream actions', () => {
  const focus = () => {
    const snap = herdr('api', 'snapshot').result.snapshot;
    return `${snap.focused_workspace_id}|${snap.focused_tab_id}|${snap.focused_pane_id}`;
  };

  test('creates a workspace, project and shell stream without changing the Mac’s focus', async () => {
    const f0 = focus();
    const folder = tmpDir('briardr-create-');
    const w = await api('POST', '/api/workspaces', { name: 'briardr-test-create', folder });
    assert.equal(w.status, 200, JSON.stringify(w.json));
    assert.equal(w.json.focusUnchanged, true);
    const p = await api('POST', '/api/projects', { workspaceId: w.json.workspaceId, name: 'backend' });
    assert.equal(p.status, 200, JSON.stringify(p.json));
    assert.equal(p.json.focusUnchanged, true);
    const st = await api('POST', '/api/streams', { projectId: p.json.projectId, kind: 'shell', name: 'tests' });
    assert.equal(st.status, 200, JSON.stringify(st.json));
    assert.equal(st.json.focusUnchanged, true);
    assert.equal(focus(), f0, 'the Mac’s focused workspace/tab/pane never changed');
    const model = (await api('GET', '/api/state')).json.model;
    const created = model.streams.find((x: any) => x.id === st.json.streamId);
    assert.equal(created.name, 'tests');
    assert.equal(created.projectId, p.json.projectId);
    assert.equal(model.projects.find((x: any) => x.id === p.json.projectId).name, 'backend');
    closeWorkspace(w.json.workspaceId);
  });

  test('a missing folder is reported clearly', async () => {
    const r = await api('POST', '/api/workspaces', { name: 'x', folder: '/definitely/not/here' });
    assert.equal(r.status, 400);
    assert.match(r.json.message, /doesn’t exist/);
  });

  test('rename, info and searchable history', async () => {
    assert.equal((await api('POST', `/api/streams/${enc(pane)}/rename`, { name: 'renamed stream' })).status, 200);
    const info = await api('GET', `/api/streams/${enc(pane)}/info`);
    assert.equal(info.json.stream.name, 'renamed stream');
    assert.equal(info.json.stream.cwd && fs.realpathSync(info.json.stream.cwd), fs.realpathSync(dir));
    herdr('pane', 'run', pane, 'echo history-marker-123');
    await sleep(600);
    const h = await api('GET', `/api/streams/${enc(pane)}/history?lines=500`);
    assert.equal(h.status, 200, JSON.stringify(h.json));
    assert.match(h.json.text, /history-marker-123/);
  });

  test('changes for the stream’s folder', async () => {
    const r = await api('GET', `/api/streams/${enc(pane)}/changes`);
    assert.equal(r.json.state, 'not_repo');
  });
});

describe('usage reset', () => {
  test('using a Codex reset needs an explicit confirmation and Codex turned on', async () => {
    assert.equal((await api('POST', '/api/usage/codex/reset-credit', {})).status, 400);
    assert.equal((await api('POST', '/api/usage/codex/reset-credit', { confirm: true })).status, 409, 'Codex usage is off in the test config');
  });
});

describe('notes', () => {
  const A = 'a1a1a1a1a1a1a1a1';
  const B = 'b2b2b2b2b2b2b2b2';
  test('a stream has many notes: create, read back, and refuse a save based on an older revision', async () => {
    const e = encodeURIComponent(pane);
    assert.deepEqual((await request(s, 'GET', `/api/streams/${e}/notes`)).json.notes, []);
    const a = await request(s, 'PUT', `/api/streams/${e}/notes/${A}`, { text: 'secret-note-text first', baseRevision: 0 });
    assert.equal(a.status, 200);
    assert.equal(a.json.note.revision, 1);
    await sleep(5);
    assert.equal((await request(s, 'PUT', `/api/streams/${e}/notes/${B}`, { text: 'another note', baseRevision: 0 })).status, 200);
    // A second device that still has revision 0 can't overwrite it.
    const stale = await request(s, 'PUT', `/api/streams/${e}/notes/${A}`, { text: 'from the other device', baseRevision: 0 });
    assert.equal(stale.status, 409);
    assert.equal(stale.json.note.text, 'secret-note-text first');
    const b = await request(s, 'PUT', `/api/streams/${e}/notes/${A}`, { text: 'second', baseRevision: 1 });
    assert.equal(b.json.note.revision, 2);
    assert.equal((await request(s, 'GET', `/api/streams/${e}/notes/${A}`)).json.note.text, 'second');
    const list = (await request(s, 'GET', `/api/streams/${e}/notes`)).json.notes;
    assert.deepEqual(list.map((n: any) => [n.id, n.text]), [[B, 'another note'], [A, 'second']], 'newest first; editing keeps the order');
  });

  test('deleting a note removes it everywhere; a pending save from another device is told so', async () => {
    const e = encodeURIComponent(pane);
    assert.equal((await request(s, 'DELETE', `/api/streams/${e}/notes/${B}`)).status, 200);
    assert.equal((await request(s, 'GET', `/api/streams/${e}/notes/${B}`)).status, 404);
    const late = await request(s, 'PUT', `/api/streams/${e}/notes/${B}`, { text: 'edited offline', baseRevision: 1 });
    assert.equal(late.status, 409);
    assert.equal(late.json.note, null);
    assert.deepEqual((await request(s, 'GET', `/api/streams/${e}/notes`)).json.notes.map((n: any) => n.id), [A]);
    // IDs that aren't ours are refused.
    assert.equal((await request(s, 'PUT', `/api/streams/${e}/notes/..%2Fx`, { text: 'x', baseRevision: 0 })).status, 404);
    assert.equal((await request(s, 'PUT', `/api/streams/${e}/notes/images`, { text: 'x', baseRevision: 0 })).status, 404);
  });

  test('limits size, needs auth, and never logs note text', async () => {
    const enc = encodeURIComponent(pane);
    const big = await request(s, 'PUT', `/api/streams/${enc}/notes/${A}`, { text: 'x'.repeat(256 * 1024 + 1), baseRevision: 2 });
    assert.equal(big.status, 413);
    assert.equal((await request(s, 'GET', `/api/streams/${enc}/notes`, undefined, { cookie: false })).status, 401);
    const logs = fs.readdirSync(path.join(s.dataDir, 'logs')).map((f) => fs.readFileSync(path.join(s.dataDir, 'logs', f), 'utf8')).join('');
    assert.ok(!logs.includes('secret-note-text'));
    const stored = fs.readdirSync(path.join(s.dataDir, 'state', 'notes')).filter((f) => f.endsWith('.json'));
    assert.equal(fs.statSync(path.join(s.dataDir, 'state', 'notes', stored[0])).mode & 0o777, 0o600);
  });

  test('images are stored with the note, PNG only, served back only by their exact name', async () => {
    const enc = encodeURIComponent(pane);
    const png = fs.readFileSync(path.resolve('web/public/icons/icon-192.png'));
    const up = await request(s, 'POST', `/api/streams/${enc}/notes/images`, undefined, { raw: png });
    assert.equal(up.status, 200);
    assert.match(up.json.name, /^[a-f0-9]{24}\.png$/);
    const got = await request(s, 'GET', `/api/streams/${enc}/notes/images/${up.json.name}`);
    assert.equal(got.status, 200);
    assert.equal(got.headers['content-type'], 'image/png');
    assert.equal((await request(s, 'GET', `/api/streams/${enc}/notes/images/${up.json.name}`, undefined, { cookie: false })).status, 401);
    // Another stream can't read it, and names that aren't ours are refused.
    assert.equal((await request(s, 'GET', `/api/streams/${encodeURIComponent('w9:p9')}/notes/images/${up.json.name}`)).status, 404);
    assert.equal((await request(s, 'GET', `/api/streams/${enc}/notes/images/${encodeURIComponent('../' + up.json.name)}`)).status, 404);
    const notPng = await request(s, 'POST', `/api/streams/${enc}/notes/images`, undefined, { raw: Buffer.from('GIF89a......................') });
    assert.equal(notPng.status, 415);
    const dir = path.join(s.dataDir, 'state', 'notes', 'images');
    const [streamDir] = fs.readdirSync(dir);
    assert.deepEqual(fs.readdirSync(path.join(dir, streamDir)), [up.json.name], 'no partial or temp files left behind');
    assert.equal(fs.statSync(path.join(dir, streamDir, up.json.name)).mode & 0o777, 0o600);
  });

  test('sent to a terminal, images become paths of private copies an agent can open', async () => {
    const enc = encodeURIComponent(pane);
    const png = fs.readFileSync(path.resolve('web/public/icons/icon-192.png'));
    const { name } = (await request(s, 'POST', `/api/streams/${enc}/notes/images`, undefined, { raw: png })).json;
    const unknown = 'c'.repeat(24) + '.png';
    const text = `Login is broken:\n![screenshot](note:${name})\nSame as note:${name}. Gone: ![old](note:${unknown})`;
    const r = await request(s, 'POST', `/api/streams/${enc}/notes/paste`, { text });
    assert.equal(r.status, 200);
    const copy = r.json.text.split('\n')[1];
    assert.equal(r.json.text, `Login is broken:\n${copy}\nSame as ${copy}. Gone: ![old](note:${unknown})`, 'each image is its path, on the same line; one copy per image');
    assert.deepEqual([r.json.images, r.json.missing], [1, 1]);
    assert.ok(copy.startsWith(path.join(s.dataDir, 'briardr-uploads') + '/'), copy);
    assert.ok(fs.readFileSync(copy).equals(png));
    assert.equal(fs.statSync(path.dirname(copy)).mode & 0o777, 0o700);
    assert.equal(fs.statSync(copy).mode & 0o777, 0o600);
    fs.rmSync(path.dirname(copy), { recursive: true });
    // Text without images comes back as it is; another stream's images aren't found.
    assert.deepEqual((await request(s, 'POST', `/api/streams/${enc}/notes/paste`, { text: 'just text' })).json, { text: 'just text', images: 0, missing: 0 });
    assert.equal((await request(s, 'POST', `/api/streams/${enc}/notes/paste`, { text }, { cookie: false })).status, 401);
  });
});
