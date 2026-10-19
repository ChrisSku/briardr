// Link handling for terminal text: soft-wrapped URLs are joined, OSC 8 hyperlinks keep their real
// destination, localhost links open in Preview, other web links open in the system browser.
import type { IBufferLine, Terminal } from '@xterm/xterm';

const URL_RE = /\bhttps?:\/\/[^\s<>"'`]+[^\s<>"'`.,;:!?)\]}]/g;

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
  for (const m of l.text.matchAll(URL_RE)) {
    const s = m.index!;
    const e = s + m[0].length;
    out.push({
      url: m[0],
      start: { x: s % l.cols, y: l.firstRow + Math.floor(s / l.cols) },
      end: { x: e % l.cols, y: l.firstRow + Math.floor(e / l.cols) },
    });
  }
  return out;
}

/** OSC 8 hyperlink at a cell, via xterm internals (guarded: returns null if unavailable). */
export function osc8At(term: Terminal, col: number, row: number): string | null {
  try {
    const core = (term as any)._core;
    const line: IBufferLine | undefined = term.buffer.active.getLine(row);
    const cell: any = line?.getCell(col);
    const id = cell?.extended?.urlId ?? (cell as any)?._cell?.extended?.urlId;
    if (!id) {
      const bl = core.buffer.lines.get(row);
      const ext = bl?._extendedAttrs?.[col];
      if (ext?.urlId) return core._oscLinkService.getLinkData(ext.urlId)?.uri ?? null;
      return null;
    }
    return core._oscLinkService.getLinkData(id)?.uri ?? null;
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
  for (const m of text.matchAll(URL_RE)) {
    if (offset >= m.index! && offset < m.index! + m[0].length) return { start: m.index!, end: m.index! + m[0].length };
  }
  const isWord = (c: string) => /[^\s"'`()[\]{}<>|,;]/.test(c);
  let s = offset;
  let e = offset;
  while (s > 0 && isWord(text[s - 1])) s--;
  while (e < text.length && isWord(text[e])) e++;
  return { start: s, end: Math.max(e, s) };
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
