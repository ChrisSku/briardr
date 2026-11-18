// Links in terminal text: found in a logical line (rows joined where text runs on past the last column, as Herdr's
// rendered screens rarely carry xterm's wrap flag), and told apart as Preview, web or other links. The terminal
// here has only the public buffer API that links.ts reads, with no OSC 8 hyperlinks in its cells.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import type { ILink, ILinkProvider, Terminal } from '@xterm/xterm';
import { continues, isLocalhostUrl, isWebUrl, linkAt, linksInLogicalLine, logicalLineAt, registerLinkProvider, screenSelection, wordAround } from '../web/src/terminal/links.ts';

/** A terminal `cols` wide showing `rows`; the rows in `wrapped` have xterm's wrap flag. */
function screen(cols: number, rows: string[], wrapped: number[] = []) {
  const lines = rows.map((text, r) => ({
    isWrapped: wrapped.includes(r),
    translateToString: (_trimRight: boolean, start = 0, end = cols) => text.padEnd(cols).slice(start, end),
    getCell: () => undefined,
  }));
  let provider: ILinkProvider | undefined;
  const term = {
    cols,
    buffer: { active: { getLine: (r: number) => lines[r] } },
    registerLinkProvider(p: ILinkProvider) {
      provider = p;
      return { dispose() {} };
    },
  } as unknown as Terminal;
  return { term, provider: () => provider! };
}

// One URL over three full rows, ten columns wide, between two rows of other text.
const URL = 'https://example.com/a/b';
const wrappedUrl = () => screen(10, ['$ ls', 'go ' + URL.slice(0, 7), URL.slice(7, 17), URL.slice(17) + ' ok', '$']);

test('a row filled to its last column continues onto a next row that starts with text', () => {
  const { term } = wrappedUrl();
  assert.equal(continues(term, 0), false, 'ends before the last column');
  assert.equal(continues(term, 1), true);
  assert.equal(continues(term, 2), true);
  assert.equal(continues(term, 3), false);
  assert.equal(continues(term, 4), false, 'the last row');
  assert.equal(continues(screen(4, ['abcd', ' efg']).term, 0), false, 'the next row starts with a space');
  assert.equal(continues(screen(4, ['ab', 'cd'], [1]).term, 0), true, 'xterm’s wrap flag');
});

test('a logical line joins the rows around any of its rows', () => {
  const { term } = wrappedUrl();
  for (const row of [1, 2, 3]) {
    const l = logicalLineAt(term, row);
    assert.deepEqual([l.firstRow, l.rows, l.cols], [1, [1, 2, 3], 10]);
    assert.equal(l.text.trimEnd(), 'go ' + URL + ' ok');
  }
  assert.deepEqual(logicalLineAt(term, 0).rows, [0]);
});

test('links in a logical line, with their cells; trailing punctuation and closing brackets stay out', () => {
  const l = { text: 'see https://example.com/a. (http://localhost:5173/x) "https://e.org/q?a=1"', rows: [0], cols: 200, firstRow: 0 };
  assert.deepEqual(linksInLogicalLine(l).map((k) => k.url), ['https://example.com/a', 'http://localhost:5173/x', 'https://e.org/q?a=1']);
  assert.deepEqual(linksInLogicalLine({ ...l, text: 'ftp://example.com mailto:a@b.c example.com' }), []);
  const [k] = linksInLogicalLine(logicalLineAt(wrappedUrl().term, 2));
  assert.deepEqual(k, { url: URL, start: { x: 3, y: 1 }, end: { x: 6, y: 3 } });
});

test('a link is found from any of its cells, on any of its rows', () => {
  const { term } = wrappedUrl();
  assert.equal(linkAt(term, 3, 1), URL, 'its first cell');
  assert.equal(linkAt(term, 0, 2), URL);
  assert.equal(linkAt(term, 5, 3), URL, 'its last cell');
  assert.equal(linkAt(term, 6, 3), null, 'just after it');
  assert.equal(linkAt(term, 1, 1), null);
  assert.equal(linkAt(term, 0, 0), null);
});

test('the link provider gives xterm each row’s part of a link, in its 1-based cells', () => {
  const { term, provider } = wrappedUrl();
  const opened: string[] = [];
  registerLinkProvider(term, (url) => opened.push(url));
  const linksOn = (y: number) => {
    let found: ILink[] | undefined;
    provider().provideLinks(y, (links) => (found = links));
    return found;
  };
  for (const y of [2, 3, 4]) assert.deepEqual(linksOn(y)?.map((k) => [k.range, k.text]), [[{ start: { x: 4, y: 2 }, end: { x: 6, y: 4 } }, URL]]);
  assert.equal(linksOn(1), undefined);
  assert.equal(linksOn(5), undefined);
  linksOn(3)![0].activate({} as MouseEvent, URL);
  assert.deepEqual(opened, [URL]);
});

test('a link that ends at the last column ends there, not at the start of the next row', () => {
  const { term, provider } = screen(10, ['x https://', 'e.com/abcd', '"ok"']);
  registerLinkProvider(term, () => {});
  const rangesOn = (y: number) => {
    let found: ILink[] | undefined;
    provider().provideLinks(y, (links) => (found = links));
    return found?.map((k) => k.range);
  };
  for (const y of [1, 2]) assert.deepEqual(rangesOn(y), [{ start: { x: 3, y: 1 }, end: { x: 10, y: 2 } }]);
  assert.equal(rangesOn(3), undefined, 'the row after it, in the same logical line');
});

test('localhost links are Preview’s', () => {
  for (const url of ['http://localhost:3000/', 'https://localhost', 'http://127.0.0.1:8080/x', 'http://0.0.0.0:5173', 'http://[::1]:5173/'])
    assert.equal(isLocalhostUrl(url), true, url);
  for (const url of ['https://example.com', 'http://localhost.example.com', 'http://127.0.0.2', 'ftp://localhost/', 'localhost:3000', 'not a url'])
    assert.equal(isLocalhostUrl(url), false, url);
});

test('web and mail links open in the browser; other schemes don’t', () => {
  for (const url of ['https://example.com', 'http://localhost:3000', 'mailto:someone@example.com']) assert.equal(isWebUrl(url), true, url);
  for (const url of ['javascript:alert(1)', 'file:///etc/hosts', 'data:text/html,x', 'ssh://host', 'example.com']) assert.equal(isWebUrl(url), false, url);
});

test('the word around a character: a whole URL, or text between spaces, quotes and brackets', () => {
  const pick = (text: string, offset: number) => {
    const { start, end } = wordAround(text, offset);
    return text.slice(start, end);
  };
  assert.equal(pick('see https://example.com/a?b=1. ok', 10), 'https://example.com/a?b=1');
  assert.equal(pick('git commit -m "fix it"', 6), 'commit');
  assert.equal(pick('git commit -m "fix it"', 16), 'fix');
  assert.equal(pick('open(src/main.ts:12)', 9), 'src/main.ts:12');
  assert.equal(pick('a  b', 2), '', 'between two spaces');
});

test('Select Text copies the visible rows, without trailing spaces, and selects the word at the cell touched', () => {
  // Scrolled back by one row: the history's first row isn't on the screen.
  const history = ['old', '$ echo alpha bravo   ', 'alpha bravo', '$ open https://example.com/x   '];
  const term = {
    rows: 3,
    buffer: { active: { viewportY: 1, getLine: (r: number) => history[r] !== undefined && { translateToString: (trimRight: boolean) => (trimRight ? history[r].trimEnd() : history[r]) } } },
  } as unknown as Terminal;
  const pick = (row: number, col: number) => {
    const { text, start, end } = screenSelection(term, row, col);
    assert.equal(text, '$ echo alpha bravo\nalpha bravo\n$ open https://example.com/x');
    return text.slice(start, end);
  };
  assert.equal(pick(0, 8), 'alpha', 'on the first row');
  assert.equal(pick(1, 7), 'bravo', 'on a later row');
  assert.equal(pick(2, 12), 'https://example.com/x', 'a link whole');
  assert.equal(pick(1, 30), 'bravo', 'past the end of a row, its last word');
});
