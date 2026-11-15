// What a prompt put together in Files is made of, and the text it is pasted into the terminal as. The draft
// itself, kept on this device, is in prompt.ts.

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
