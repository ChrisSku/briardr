// Link handling for terminal text: soft-wrapped URLs are joined, OSC 8 hyperlinks keep their real
// destination, localhost links open in Preview, other web links open in the system browser. Select Text
// picks a link, or the word, at the cell touched.
import type { Terminal } from '@xterm/xterm';

const URL_RE = /\bhttps?:\/\/[^\s<>"'`]+/g;
const OPENS: Record<string, string> = { ')': '(', ']': '[', '}': '{' };

/**
 * The web links in text, with their offsets. A link ends before trailing punctuation, and before a closing
 * bracket that closes none opened in it: `(see https://e.org/a)`, but `https://e.org/Rust_(language)`.
 */
function urlsIn(text: string): { index: number; url: string }[] {
  const out = [];
  for (const m of text.matchAll(URL_RE)) {
    let url = m[0];
    for (;;) {
      const c = url[url.length - 1];
      const open = OPENS[c];
      if (!'.,;:!?'.includes(c) && !(open && url.split(open).length < url.split(c).length)) break;
      url = url.slice(0, -1);
    }
    if (/:\/\/./.test(url)) out.push({ index: m.index!, url }); // not the scheme alone
  }
  return out;
}

export interface LogicalLine {
  text: string;
  /** Buffer row of each logical-line segment start. */
  rows: number[];
  cols: number;
  firstRow: number;
}

/**
 * Does row `r + 1` continue row `r`? Herdr sends rendered screens (rows placed with cursor moves), so
 * xterm's wrap flag is usually missing. Like terminals without wrap information, treat a row whose
 * last column is filled as continuing onto a next row that starts with text.
 */
export function continues(term: Terminal, r: number): boolean {
  const buf = term.buffer.active;
  const cur = buf.getLine(r);
  const next = buf.getLine(r + 1);
  if (!cur || !next) return false;
  if (next.isWrapped) return true;
  const last = cur.translateToString(false, term.cols - 1, term.cols);
  const first = next.translateToString(false, 0, 1);
  return last.trim() !== '' && first.trim() !== '';
}

/** Join a buffer row with the wrapped rows around it into one logical line. */
export function logicalLineAt(term: Terminal, row: number): LogicalLine {
  const buf = term.buffer.active;
  let first = row;
  while (first > 0 && continues(term, first - 1)) first--;
  const rows: number[] = [];
  let text = '';
  let r = first;
  for (;;) {
    const line = buf.getLine(r);
    if (!line) break;
    rows.push(r);
    text += line.translateToString(false, 0, term.cols);
    if (!continues(term, r)) break;
    r++;
  }
  return { text, rows, cols: term.cols, firstRow: first };
}

export interface FoundLink {
  url: string;
  /** Inclusive start / exclusive end as buffer [col,row] pairs. */
  start: { x: number; y: number };
  end: { x: number; y: number };
}

export function linksInLogicalLine(l: LogicalLine): FoundLink[] {
  const out: FoundLink[] = [];
  for (const { index: s, url } of urlsIn(l.text)) {
    const e = s + url.length;
    out.push({
      url,
      start: { x: s % l.cols, y: l.firstRow + Math.floor(s / l.cols) },
      end: { x: e % l.cols, y: l.firstRow + Math.floor(e / l.cols) },
    });
  }
  return out;
}

/** OSC 8 hyperlink at a cell, via xterm internals (guarded: returns null if unavailable). Only the cell
 *  says whether it has a link: its line's extended attributes keep one after new text overwrites it. */
export function osc8At(term: Terminal, col: number, row: number): string | null {
  try {
    const id = (term.buffer.active.getLine(row)?.getCell(col) as any)?.extended?.urlId;
    return id ? ((term as any)._core._oscLinkService.getLinkData(id)?.uri ?? null) : null;
  } catch {
    return null;
  }
}

export function linkAt(term: Terminal, col: number, row: number): string | null {
  const osc = osc8At(term, col, row);
  if (osc) return osc;
  const l = logicalLineAt(term, row);
  const offset = (row - l.firstRow) * l.cols + col;
  for (const link of linksInLogicalLine(l)) {
    const s = (link.start.y - l.firstRow) * l.cols + link.start.x;
    const e = (link.end.y - l.firstRow) * l.cols + link.end.x;
    if (offset >= s && offset < e) return link.url;
  }
  return null;
}

export function isLocalhostUrl(url: string): boolean {
  try {
    const u = new URL(url);
    return (u.protocol === 'http:' || u.protocol === 'https:') && ['localhost', '127.0.0.1', '0.0.0.0', '[::1]', '::1'].includes(u.hostname);
  } catch {
    return false;
  }
}

export function isWebUrl(url: string): boolean {
  try {
    const u = new URL(url);
    return u.protocol === 'http:' || u.protocol === 'https:' || u.protocol === 'mailto:';
  } catch {
    return false;
  }
}

/** The word (or URL) around a character offset in a string. */
export function wordAround(text: string, offset: number): { start: number; end: number } {
  for (const { index, url } of urlsIn(text)) {
    if (offset >= index && offset < index + url.length) return { start: index, end: index + url.length };
  }
  const isWord = (c: string) => /[^\s"'`()[\]{}<>|,;]/.test(c);
  let s = offset;
  let e = offset;
  while (s > 0 && isWord(text[s - 1])) s--;
  while (e < text.length && isWord(text[e])) e++;
  return { start: s, end: Math.max(e, s) };
}

/** A frozen copy of the screen as text, for Select Text, with the word (or URL) at a visible cell selected:
 *  later output doesn't move the selection. */
export function screenSelection(term: Pick<Terminal, 'rows' | 'buffer'>, row: number, col: number): { text: string; start: number; end: number } {
  const buf = term.buffer.active;
  const lines: string[] = [];
  let offset = 0;
  for (let r = 0; r < term.rows; r++) {
    const line = buf.getLine(buf.viewportY + r);
    const s = line ? line.translateToString(true) : '';
    if (r === row) offset = lines.join('\n').length + (r ? 1 : 0) + Math.min(col, s.length);
    lines.push(s);
  }
  const text = lines.join('\n');
  return { text, ...wordAround(text, offset) };
}

/** Register a link provider so wrapped URLs are underlined on hover and clickable with a pointer. */
export function registerLinkProvider(term: Terminal, activate: (url: string) => void) {
  return term.registerLinkProvider({
    provideLinks(y, callback) {
      const row = y - 1;
      const l = logicalLineAt(term, row);
      const links = linksInLogicalLine(l)
        .filter((k) => k.start.y <= row && (k.end.y > row || (k.end.y === row && k.end.x > 0)))
        .map((k) => ({
          range: { start: { x: k.start.x + 1, y: k.start.y + 1 }, end: { x: k.end.x === 0 ? l.cols : k.end.x, y: k.end.x === 0 ? k.end.y : k.end.y + 1 } },
          text: k.url,
          activate: (_e: MouseEvent, text: string) => activate(text),
        }));
      callback(links.length ? links : undefined);
    },
  });
}
