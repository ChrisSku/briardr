// Markdown rendering for Docs and Notes. Raw HTML in documents is escaped (never executed), remote
// images are not loaded, relative images load through Briardr's authenticated asset route, and relative
// links navigate within Docs. In Notes, `note:<name>.png` images load from the note's own images.
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

export function renderMarkdown(src: string, opts: { docPath: string; streamId: string; notes?: boolean }): string {
  const md = new MarkdownIt({ html: false, linkify: true, typographer: false, breaks: false });
  const esc = md.utils.escapeHtml;

  md.renderer.rules.image = (tokens, idx) => {
    const t = tokens[idx];
    const srcAttr = String(t.attrGet('src') || '');
    const alt = esc(t.content || '');
    const note = opts.notes ? srcAttr.match(NOTE_IMAGE) : null;
    if (note) return `<img src="/api/streams/${esc(encodeURIComponent(opts.streamId))}/notes/images/${note[1]}" alt="${alt}" decoding="async">`;
    if (isExternal(srcAttr)) return `<span class="md-remote-img" role="img" aria-label="Remote image not loaded">Remote image not loaded${alt ? `: ${alt}` : ''}</span>`;
    const rel = resolveRel(opts.docPath, srcAttr);
    if (!rel) return `<span class="md-remote-img">Image outside this folder${alt ? `: ${alt}` : ''}</span>`;
    const url = `/api/streams/${encodeURIComponent(opts.streamId)}/docs/asset?path=${encodeURIComponent(rel)}`;
    return `<img src="${esc(url)}" alt="${alt}" loading="lazy" decoding="async">`;
  };

  const linkOpen = md.renderer.rules.link_open || ((tokens, idx, o, _e, self) => self.renderToken(tokens, idx, o));
  md.renderer.rules.link_open = (tokens, idx, o, env, self) => {
    const t = tokens[idx];
    const href = String(t.attrGet('href') || '');
    if (href.startsWith('#')) {
      t.attrSet('data-anchor', href.slice(1));
    } else if (isExternal(href)) {
      if (!/^(https?|mailto):/i.test(href)) t.attrSet('href', '#');
      t.attrSet('target', '_blank');
      t.attrSet('rel', 'noopener noreferrer');
      t.attrSet('data-external', '1');
    } else {
      const rel = resolveRel(opts.docPath, href);
      t.attrSet('data-doc', rel || '');
      t.attrSet('href', '#');
    }
    return linkOpen(tokens, idx, o, env, self);
  };

  md.renderer.rules.table_open = () => '<div class="md-table" tabindex="0" role="region" aria-label="Table"><table>';
  md.renderer.rules.table_close = () => '</table></div>';

  // Headings get anchors so in-document links work.
  md.renderer.rules.heading_open = (tokens, idx) => {
    const inline = tokens[idx + 1];
    const slug = (inline?.content || '').toLowerCase().trim().replace(/[^\p{L}\p{N}\s-]/gu, '').replace(/\s+/g, '-');
    return `<${tokens[idx].tag} id="md-${esc(slug)}">`;
  };

  // Task lists: "- [ ] item" / "- [x] item" render as read-only checkboxes.
  md.core.ruler.after('inline', 'tasks', (state) => {
    const toks = state.tokens;
    for (let i = 2; i < toks.length; i++) {
      if (toks[i].type !== 'inline' || toks[i - 1].type !== 'paragraph_open' || toks[i - 2].type !== 'list_item_open') continue;
      const m = toks[i].content.match(/^\[([ xX])\]\s+/);
      if (!m) continue;
      const checked = m[1] !== ' ';
      const first = toks[i].children?.[0];
      if (first && first.type === 'text') first.content = first.content.replace(/^\[[ xX]\]\s+/, '');
      const box = new state.Token('html_inline', '', 0);
      box.content = `<input type="checkbox" disabled${checked ? ' checked' : ''} aria-label="${checked ? 'Done' : 'Not done'}"> `;
      toks[i].children?.unshift(box);
      toks[i - 2].attrJoin('class', 'task');
    }
  });

  return md.render(src);
}
