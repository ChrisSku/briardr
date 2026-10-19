// Syntax highlighting for Files (highlight.js with a curated set of languages). Loaded on
// demand with import(), so it stays out of the main bundle. Output is split into one HTML string per
// line, with spans that cross a line break closed and reopened, so each line renders on its own.
import hljs from 'highlight.js/lib/core';
import bash from 'highlight.js/lib/languages/bash';
import c from 'highlight.js/lib/languages/c';
import cpp from 'highlight.js/lib/languages/cpp';
import csharp from 'highlight.js/lib/languages/csharp';
import css from 'highlight.js/lib/languages/css';
import dart from 'highlight.js/lib/languages/dart';
import diff from 'highlight.js/lib/languages/diff';
import dockerfile from 'highlight.js/lib/languages/dockerfile';
import elixir from 'highlight.js/lib/languages/elixir';
import go from 'highlight.js/lib/languages/go';
import graphql from 'highlight.js/lib/languages/graphql';
import ini from 'highlight.js/lib/languages/ini';
import java from 'highlight.js/lib/languages/java';
import javascript from 'highlight.js/lib/languages/javascript';
import json from 'highlight.js/lib/languages/json';
import kotlin from 'highlight.js/lib/languages/kotlin';
import lua from 'highlight.js/lib/languages/lua';
import makefile from 'highlight.js/lib/languages/makefile';
import markdown from 'highlight.js/lib/languages/markdown';
import objectivec from 'highlight.js/lib/languages/objectivec';
import php from 'highlight.js/lib/languages/php';
import plaintext from 'highlight.js/lib/languages/plaintext';
import protobuf from 'highlight.js/lib/languages/protobuf';
import python from 'highlight.js/lib/languages/python';
import ruby from 'highlight.js/lib/languages/ruby';
import rust from 'highlight.js/lib/languages/rust';
import scss from 'highlight.js/lib/languages/scss';
import sql from 'highlight.js/lib/languages/sql';
import swift from 'highlight.js/lib/languages/swift';
import typescript from 'highlight.js/lib/languages/typescript';
import xml from 'highlight.js/lib/languages/xml';
import yaml from 'highlight.js/lib/languages/yaml';

const LANGUAGES = { bash, c, cpp, csharp, css, dart, diff, dockerfile, elixir, go, graphql, ini, java, javascript, json, kotlin, lua, makefile, markdown, objectivec, php, plaintext, protobuf, python, ruby, rust, scss, sql, swift, typescript, xml, yaml };
for (const [name, lang] of Object.entries(LANGUAGES)) hljs.registerLanguage(name, lang);

/** Beyond this, a file is shown as plain text: highlighting it would stall a phone. */
export const MAX_HIGHLIGHT_BYTES = 300_000;

const escapeHtml = (s: string) => s.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');

/** Highlighted HTML, one entry per line; plain (escaped) for an unknown language or a big file. */
export function highlightLines(text: string, language: string | null): { lines: string[]; highlighted: boolean } {
  // A final newline ends the last line rather than starting another.
  const src = text.replace(/\r\n?/g, '\n').replace(/\n$/, '');
  if (!language || language === 'plaintext' || !hljs.getLanguage(language) || src.length > MAX_HIGHLIGHT_BYTES) return { lines: src.split('\n').map(escapeHtml), highlighted: false };
  return { lines: splitLines(hljs.highlight(src, { language, ignoreIllegals: true }).value), highlighted: true };
}

/** highlight.js output only holds `<span class=…>`, `</span>` and escaped text. */
export function splitLines(html: string): string[] {
  const lines: string[] = [];
  const open: string[] = [];
  let line = '';
  let last = 0;
  for (const m of html.matchAll(/<span[^>]*>|<\/span>|\n/g)) {
    line += html.slice(last, m.index);
    last = m.index + m[0].length;
    if (m[0] === '\n') {
      lines.push(line + '</span>'.repeat(open.length));
      line = open.join('');
    } else {
      if (m[0] === '</span>') open.pop();
      else open.push(m[0]);
      line += m[0];
    }
  }
  lines.push(line + html.slice(last));
  return lines;
}
