// Markdown rendering for Docs and Notes. Raw HTML in documents keeps only an allowlist of tags and
// attributes, rebuilt from scratch so nothing in a document can run; any other tag shows as text. Remote
// images are not loaded, relative images load through Briardr's authenticated asset route, and relative
// links navigate within Docs, in Markdown and in raw HTML alike. In Notes, `note:<name>.png` images load
// from the note's own images.
import MarkdownIt from 'markdown-it';

export function resolveRel(fromDoc: string, target: string): string | null {
  const clean = target.split('#')[0].split('?')[0];
  if (!clean) return null;
  const base = fromDoc.includes('/') ? fromDoc.slice(0, fromDoc.lastIndexOf('/') + 1) : '';
  const parts = (clean.startsWith('/') ? clean.slice(1) : base + clean).split('/');
  const out: string[] = [];
  for (const p of parts) {
    if (p === '' || p === '.') continue;
    if (p === '..') {
      if (!out.length) return null; // would leave the stream's folder
      out.pop();
    } else out.push(decodeURIComponentSafe(p));
  }
  return out.join('/');
}

function decodeURIComponentSafe(s: string) {
  try {
    return decodeURIComponent(s);
  } catch {
    return s;
  }
}

const isExternal = (u: string) => /^[a-z][a-z0-9+.-]*:/i.test(u) || u.startsWith('//');

export const NOTE_IMAGE = /^note:([a-f0-9]{24}\.png)$/;

const HTML_TAGS = new Set('a abbr b bdo blockquote br caption cite code dd del details dfn div dl dt em figcaption figure h1 h2 h3 h4 h5 h6 hr i img ins kbd li mark ol p picture pre q rp rt ruby s samp small span strike strong sub summary sup table tbody td tfoot th thead time tr tt u ul var wbr'.split(' '));
const HTML_VOID = new Set(['br', 'hr', 'img', 'wbr']);
// Removed instead of shown: their contents and <picture>'s alternatives mean nothing to a reader.
const HTML_HIDDEN = new Set(['script', 'style', 'source']);
const HTML_ATTRS = new Set('abbr align colspan datetime dir height lang open rowspan scope start title type valign width'.split(' '));
// A comment, a script or style with its contents, or one well-formed tag (close, name, attributes).
const HTML_TOKEN = /<!--[\s\S]*?(?:-->|$)|<(script|style)(?=[\s/>])[\s\S]*?(?:<\/\1\s*>|$)|<(\/?)([a-z][a-z0-9-]*)((?:\s+[a-z_:][\w:.-]*(?:\s*=\s*(?:"[^"]*"|'[^']*'|[^\s"'=<>`]+))?)*)\s*\/?>/gi;
const HTML_ATTR = /([a-z_:][\w:.-]*)(?:\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s"'=<>`]+)))?/gi;

export function renderMarkdown(src: string, opts: { docPath: string; streamId: string; notes?: boolean }): string {
  const md = new MarkdownIt({ html: true, linkify: true, typographer: false, breaks: false });
  const esc = md.utils.escapeHtml;

  const image = (srcAttr: string, rawAlt: string, attrs = '') => {
    const alt = esc(rawAlt);
    const note = opts.notes ? srcAttr.match(NOTE_IMAGE) : null;
    if (note) return `<img src="/api/streams/${esc(encodeURIComponent(opts.streamId))}/notes/images/${note[1]}" alt="${alt}"${attrs} decoding="async">`;
    if (isExternal(srcAttr)) return `<span class="md-remote-img" role="img" aria-label="Remote image not loaded">Remote image not loaded${alt ? `: ${alt}` : ''}</span>`;
    const rel = resolveRel(opts.docPath, srcAttr);
    if (!rel) return `<span class="md-remote-img">Image outside this folder${alt ? `: ${alt}` : ''}</span>`;
    const url = `/api/streams/${encodeURIComponent(opts.streamId)}/docs/asset?path=${encodeURIComponent(rel)}`;
    return `<img src="${esc(url)}" alt="${alt}"${attrs} loading="lazy" decoding="async">`;
  };
  md.renderer.rules.image = (tokens, idx) => image(String(tokens[idx].attrGet('src') || ''), tokens[idx].content || '');

  const linkAttrs = (href: string): [string, string][] => {
    if (href.startsWith('#')) return [['href', href], ['data-anchor', href.slice(1)]];
    if (isExternal(href)) return [['href', /^(https?|mailto):/i.test(href) ? href : '#'], ['target', '_blank'], ['rel', 'noopener noreferrer'], ['data-external', '1']];
    return [['href', '#'], ['data-doc', resolveRel(opts.docPath, href) || '']];
  };

  const linkOpen = md.renderer.rules.link_open || ((tokens, idx, o, _e, self) => self.renderToken(tokens, idx, o));
  md.renderer.rules.link_open = (tokens, idx, o, env, self) => {
    const t = tokens[idx];
    for (const [name, value] of linkAttrs(String(t.attrGet('href') || ''))) t.attrSet(name, value);
    return linkOpen(tokens, idx, o, env, self);
  };

  // Every `<` in the output comes from a tag built here: text is escaped, and a tag keeps only its
  // allowed attributes, re-escaped. Entities stay, since they can only ever decode to text.
  const htmlText = (s: string) => s.replace(/&(?!#?\w+;)/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
  const htmlTag = (whole: string, close: string, tag: string, rawAttrs: string) => {
    const name = tag.toLowerCase();
    if (HTML_HIDDEN.has(name)) return '';
    if (!HTML_TAGS.has(name)) return htmlText(whole);
    if (close) return HTML_VOID.has(name) ? '' : `</${name}>`;
    const attrs = new Map<string, string>();
    for (const m of rawAttrs.matchAll(HTML_ATTR)) {
      const attr = m[1].toLowerCase();
      if (!attrs.has(attr)) attrs.set(attr, md.utils.unescapeAll(m[2] ?? m[3] ?? m[4] ?? ''));
    }
    const kept = [...attrs].filter(([attr]) => HTML_ATTRS.has(attr));
    const href = attrs.get('href');
    if (name === 'a' && href !== undefined) kept.push(...linkAttrs(href));
    // Prefixed like the headings' ids: in-document links find it, and it can't shadow the app's own.
    const anchor = attrs.get('id') || attrs.get('name');
    if (anchor) kept.push(['id', `md-${anchor}`]);
    const out = kept.map(([attr, value]) => ` ${attr}="${esc(value)}"`).join('');
    return name === 'img' ? image(attrs.get('src') || '', attrs.get('alt') || '', out) : `<${name}${out}>`;
  };
  md.renderer.rules.html_block = md.renderer.rules.html_inline = (tokens, idx) => {
    const raw = tokens[idx].content;
    let out = '';
    let last = 0;
    for (const m of raw.matchAll(HTML_TOKEN)) {
      out += htmlText(raw.slice(last, m.index));
      if (m[3]) out += htmlTag(m[0], m[2], m[3], m[4]);
      last = m.index + m[0].length;
    }
    return out + htmlText(raw.slice(last));
  };

  // Mermaid diagrams stay code here; renderDiagrams (diagrams.ts) draws them once they're in the page.
  const fence = md.renderer.rules.fence!;
  md.renderer.rules.fence = (tokens, idx, o, env, self) =>
    tokens[idx].info.trim().split(/\s+/)[0] === 'mermaid' ? `<pre class="md-diagram"><code>${esc(tokens[idx].content)}</code></pre>\n` : fence(tokens, idx, o, env, self);

  md.renderer.rules.table_open = () => '<div class="md-table" tabindex="0" role="region" aria-label="Table"><table>';
  md.renderer.rules.table_close = () => '</table></div>';

  // Headings get anchors so in-document links work.
  md.renderer.rules.heading_open = (tokens, idx) => {
    const inline = tokens[idx + 1];
    const slug = (inline?.content || '').toLowerCase().trim().replace(/[^\p{L}\p{N}\s-]/gu, '').replace(/\s+/g, '-');
    return `<${tokens[idx].tag} id="md-${esc(slug)}">`;
  };

  // Task lists: "- [ ] item" / "- [x] item" render as read-only checkboxes.
  md.renderer.rules.task_box = (tokens, idx) => tokens[idx].content;
  md.core.ruler.after('inline', 'tasks', (state) => {
    const toks = state.tokens;
    for (let i = 2; i < toks.length; i++) {
      if (toks[i].type !== 'inline' || toks[i - 1].type !== 'paragraph_open' || toks[i - 2].type !== 'list_item_open') continue;
      const m = toks[i].content.match(/^\[([ xX])\]\s+/);
      if (!m) continue;
      const checked = m[1] !== ' ';
      const first = toks[i].children?.[0];
      if (first && first.type === 'text') first.content = first.content.replace(/^\[[ xX]\]\s+/, '');
      const box = new state.Token('task_box', '', 0);
      box.content = `<input type="checkbox" disabled${checked ? ' checked' : ''} aria-label="${checked ? 'Done' : 'Not done'}"> `;
      toks[i].children?.unshift(box);
      toks[i - 2].attrJoin('class', 'task');
    }
  });

  return md.render(src);
}
