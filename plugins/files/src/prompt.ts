// A prompt put together in Files: line ranges picked from one or more files, plus a question, sent to
// the terminal as one paste. Kept on this device per stream, so it survives switching files, surfaces
// and relaunches until it is sent or cleared. Each snippet keeps the text it had when it was added.
import { persisted } from '@briardr/sdk';

export interface Snippet {
  path: string;
  /** 1-based, inclusive. */
  from: number;
  to: number;
  text: string;
  /** Legacy whole-file drafts are migrated to references below. */
  whole?: boolean;
}
export interface FileReference { path: string; dir: boolean }
export interface Prompt {
  question: string;
  snippets: Snippet[];
  references?: FileReference[];
}

const prompts = persisted<Record<string, Prompt>>('briardr.files.prompt', {});
const EMPTY: Prompt = { question: '', snippets: [] };

// Older drafts embedded whole files. Keep the selected paths, dropping their cached contents.
const migrated = { ...prompts.value };
let needsMigration = false;
for (const [sid, prompt] of Object.entries(migrated)) {
  const whole = prompt.snippets.filter((s) => s.whole);
  if (!whole.length) continue;
  const references = new Map((prompt.references ?? []).map((r) => [r.path, r]));
  for (const s of whole) references.set(s.path, { path: s.path, dir: false });
  migrated[sid] = { ...prompt, snippets: prompt.snippets.filter((s) => !s.whole), references: [...references.values()] };
  needsMigration = true;
}
if (needsMigration) prompts.value = migrated;

export const promptOf = (sid: string): Prompt => prompts.value[sid] ?? EMPTY;

function put(sid: string, p: Prompt) {
  const next = { ...prompts.value };
  if (p.snippets.length || p.references?.length || p.question) next[sid] = p;
  else delete next[sid];
  prompts.value = next;
}

/** Add lines from..to of `text` (a whole file); the same range of the same file is replaced. */
export function addSnippet(sid: string, path: string, from: number, to: number, text: string) {
  const lines = text.replace(/\r\n?/g, '\n').split('\n').slice(from - 1, to);
  const p = promptOf(sid);
  const snippets = p.snippets.filter((s) => !(s.path === path && !s.whole && s.from === from && s.to === to));
  put(sid, { ...p, snippets: [...snippets, { path, from, to, text: lines.join('\n') }] });
}
export function addFileReference(sid: string, path: string, dir: boolean) {
  const p = promptOf(sid);
  put(sid, { ...p, references: [...(p.references ?? []).filter((r) => r.path !== path), { path, dir }] });
}
export function removeFileReference(sid: string, i: number) {
  const p = promptOf(sid);
  put(sid, { ...p, references: (p.references ?? []).filter((_, j) => j !== i) });
}
export function removeSnippet(sid: string, i: number) {
  const p = promptOf(sid);
  put(sid, { ...p, snippets: p.snippets.filter((_, j) => j !== i) });
}
export const setQuestion = (sid: string, question: string) => put(sid, { ...promptOf(sid), question });
export const clearPrompt = (sid: string) => put(sid, EMPTY);

export const rangeLabel = (from: number, to: number) => (from === to ? `${from}` : `${from}-${to}`);
export const fileMention = (r: FileReference) => `@${r.path}${r.dir ? '/' : ''}`;

/**
 * The text that is pasted: the question, then each snippet as its path and lines on one line and the
 * code in a fence (longer than any run of backticks inside it, so code containing fences stays whole).
 *
 *   Why does this retry forever?
 *
 *   server/src/files.rs 40-52
 *   ```rs
 *   …
 *   ```
 */
export function formatPrompt(p: Prompt): string {
  const parts: string[] = [];
  if (p.question.trim()) parts.push(p.question.trim());
  for (const s of p.snippets) {
    if (s.whole) {
      parts.push(fileMention({ path: s.path, dir: false }));
      continue;
    }
    let longest = 2;
    for (const match of s.text.matchAll(/`+/g)) longest = Math.max(longest, match[0].length);
    const fence = '`'.repeat(longest + 1);
    const name = s.path.split('/').pop()!;
    const ext = name.lastIndexOf('.') > 0 ? name.slice(name.lastIndexOf('.') + 1).toLowerCase() : '';
    parts.push(`${s.path} ${rangeLabel(s.from, s.to)}\n${fence}${ext}\n${s.text}\n${fence}`);
  }
  for (const r of p.references ?? []) parts.push(fileMention(r));
  return parts.join('\n\n');
}
