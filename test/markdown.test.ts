// Docs and Notes render a document's Markdown and its raw HTML, keeping only tags and attributes that can't
// run anything: no scripts, no event handlers, no script URLs, no remote images. Relative links and images stay
// in the stream's folder, and Mermaid diagrams are left as code for diagrams.ts to draw.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { renderMarkdown } from '../web/src/markdown.ts';

const render = (src: string, notes = false) => renderMarkdown(src, { docPath: 'docs/guide.md', streamId: 'w1:p2', notes });

const TAG = /<\/?([a-z][a-z0-9-]*)([^>]*)>/gi;

/** Every tag in the output is one nothing can run from, with nothing loaded from elsewhere; all other `<` are text. */
function assertInert(html: string, src: string) {
  for (const [, name, attrs] of html.matchAll(TAG)) {
    assert.doesNotMatch(name, /^(script|style|iframe|object|embed|svg|math|form|link|meta|base)$/i, src);
    for (const [, attr, value = ''] of attrs.matchAll(/\s([^\s="]+)(?:="([^"]*)")?/g)) {
      assert.doesNotMatch(attr, /^(on|style$|srcset$|action$|formaction$)/i, `${src}: ${attr}`);
      if (attr === 'href') assert.match(value, /^(#[\w-]*|https?:|mailto:)/, src);
      if (attr === 'src') assert.match(value, /^\/api\/streams\//, src);
    }
  }
  assert.doesNotMatch(html.replace(TAG, ''), /</, src);
}

test('scripts, styles and comments in raw HTML are removed with their contents', () => {
  for (const src of ['<script>alert(1)</script>', '<style>body { display: none }</style>', '<!-- <script>alert(1)</script> -->', 'Text <script src="https://example.com/x.js"></script> more']) {
    const html = render(src);
    assert.doesNotMatch(html, /alert|display|example\.com/, src);
    assertInert(html, src);
  }
  assert.equal(render('a <script>alert(1)</script> b'), '<p>a alert(1) b</p>\n', 'inline: what was inside shows as text');
});

test('event handlers and styles are dropped; allowed attributes are kept and escaped', () => {
  assert.equal(render('<img src=x onerror=alert(1)>'), '<img src="/api/streams/w1%3Ap2/docs/asset?path=docs%2Fx" alt="" loading="lazy" decoding="async">');
  assert.equal(render('<div onclick="steal()" style="position: fixed" title="Tip" id="top">Hi</div>'), '<div title="Tip" id="md-top">Hi</div>');
  assert.equal(render('<details open ontoggle="alert(1)"><summary>More</summary></details>'), '<details open=""><summary>More</summary></details>');
  // A quote in a value can't end the attribute and start a new one.
  const html = render(`<abbr title='x" onmouseover="alert(1)'>A</abbr>`);
  assert.match(html, /<abbr title="x&quot; onmouseover=&quot;alert\(1\)">A<\/abbr>/);
  assertInert(html, 'quote in a value');
});

test('tags outside the allowlist show as text', () => {
  for (const src of ['<iframe src="https://example.com"></iframe>', '<svg onload="alert(1)"><circle r="1"/></svg>', '<form action="/api/x"><button>Go</button></form>', '<object data="x.swf"></object>', '<scr<script>ipt>alert(1)</script>']) {
    const html = render(src);
    assert.match(html, /&lt;/, src);
    assertInert(html, src);
  }
});

test('script and other non-web URLs never become links', () => {
  const urls = ['javascript:alert(1)', 'JaVaScRiPt:alert(1)', '&#106;avascript:alert(1)', 'java&#x09;script:alert(1)', ' javascript:alert(1)', 'vbscript:msgbox(1)', 'data:text/html,<script>alert(1)</script>', 'file:///etc/passwd', '//example.com/x'];
  for (const url of urls) {
    assertInert(render(`<a href="${url}">x</a>`), url);
    assertInert(render(`[x](${url})`), url);
    assertInert(render(`<${url}>`), url);
  }
  assert.equal(render('<a href="javascript:alert(1)">x</a>'), '<p><a href="#" target="_blank" rel="noopener noreferrer" data-external="1">x</a></p>\n');
});

test('web links open outside; relative links stay in the folder; anchors stay in the document', () => {
  assert.equal(render('[Site](https://example.com/a)'), '<p><a href="https://example.com/a" target="_blank" rel="noopener noreferrer" data-external="1">Site</a></p>\n');
  assert.equal(render('https://example.com'), '<p><a href="https://example.com" target="_blank" rel="noopener noreferrer" data-external="1">https://example.com</a></p>\n', 'a bare URL');
  assert.equal(render('[Mail](mailto:someone@example.com)'), '<p><a href="mailto:someone@example.com" target="_blank" rel="noopener noreferrer" data-external="1">Mail</a></p>\n');
  assert.equal(render('[Next](setup/next%20steps.md#install)'), '<p><a href="#" data-doc="docs/setup/next steps.md">Next</a></p>\n');
  assert.equal(render('[Readme](../README.md)'), '<p><a href="#" data-doc="README.md">Readme</a></p>\n');
  assert.equal(render('[Root](/notes.md)'), '<p><a href="#" data-doc="notes.md">Root</a></p>\n', 'from the stream’s folder');
  assert.equal(render('[Out](../../secret.md)'), '<p><a href="#" data-doc="">Out</a></p>\n', 'outside the folder: nowhere');
  assert.equal(render('<a href="other.md">Other</a>'), '<p><a href="#" data-doc="docs/other.md">Other</a></p>\n', 'in raw HTML too');
  assert.equal(render('[Up](#install)'), '<p><a href="#install" data-anchor="install">Up</a></p>\n');
  assert.equal(render('## Install & Run!'), '<h2 id="md-install-run">Install &amp; Run!</h2>\n');
});

test('images load from the folder through Briardr; remote ones and ones outside the folder don’t load', () => {
  assert.equal(render('![Logo](img/logo.png)'), '<p><img src="/api/streams/w1%3Ap2/docs/asset?path=docs%2Fimg%2Flogo.png" alt="Logo" loading="lazy" decoding="async"></p>\n');
  assert.equal(render('![Chart](https://example.com/c.png)'), '<p><span class="md-remote-img" role="img" aria-label="Remote image not loaded: Chart" title="Remote image not loaded: Chart">Chart</span></p>\n');
  assert.equal(render('<img src="https://example.com/c.png" alt="Chart">'), '<span class="md-remote-img" role="img" aria-label="Remote image not loaded: Chart" title="Remote image not loaded: Chart">Chart</span>');
  assert.equal(render('<img src="logo.png" srcset="https://example.com/x.png 2x" alt="L">'), '<img src="/api/streams/w1%3Ap2/docs/asset?path=docs%2Flogo.png" alt="L" loading="lazy" decoding="async">');
  assert.equal(render('![Key](../../id_rsa.png)'), '<p><span class="md-remote-img">Image outside this folder: Key</span></p>\n');
  assertInert(render('![x](data:image/png;base64,AAAA)'), 'a data: image');
});

test('a note’s own images load from its images; elsewhere a note: image is remote', () => {
  const src = '![Sketch](note:0123456789abcdef01234567.png)';
  assert.equal(render(src, true), '<p><img src="/api/streams/w1%3Ap2/notes/images/0123456789abcdef01234567.png" alt="Sketch" decoding="async"></p>\n');
  assert.match(render(src), /md-remote-img/);
  assert.match(render('![x](note:../../x.png)', true), /md-remote-img/, 'not a note image name');
});

test('Mermaid fences stay code, marked for drawing; other fences are code', () => {
  assert.equal(render('```mermaid\ngraph TD\n  A-->B["<img src=x onerror=alert(1)>"]\n```'), '<pre class="md-diagram"><code>graph TD\n  A--&gt;B[&quot;&lt;img src=x onerror=alert(1)&gt;&quot;]\n</code></pre>\n');
  assert.equal(render('```mermaid  title="Flow"\nA\n```'), '<pre class="md-diagram"><code>A\n</code></pre>\n', 'the first word of the info string');
  assert.equal(render('```js\nif (a < b) run();\n```'), '<pre><code class="language-js">if (a &lt; b) run();\n</code></pre>\n');
  assert.equal(render('```\nmermaid\n```'), '<pre><code>mermaid\n</code></pre>\n');
});

test('task lists show read-only checkboxes; tables scroll in a region', () => {
  assert.equal(
    render('- [ ] Write\n- [x] Test'),
    '<ul>\n<li class="task"><input type="checkbox" disabled aria-label="Not done"> Write</li>\n<li class="task"><input type="checkbox" disabled checked aria-label="Done"> Test</li>\n</ul>\n',
  );
  assert.match(render('| a |\n| - |\n| 1 |'), /^<div class="md-table" tabindex="0" role="region" aria-label="Table"><table>[\s\S]*<\/table><\/div>$/);
});
