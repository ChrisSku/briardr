// Review comments on the changes: lines picked in a diff, each with what to do about them, sent to the
// terminal as one paste with an optional note in front. Each comment keeps the lines it was written on.

export interface DiffLine {
  text: string;
  k: 'dl-meta' | 'dl-hunk' | 'dl-add' | 'dl-del' | 'dl-ctx';
  /** The line's number: the old file's for a removed line, the new file's otherwise (from the hunk headers). */
  n: number | null;
}

export interface Comment {
  path: string;
  /** Where the lines are, as the diff numbers them: "42", "42-44", or "40 (removed)". */
  where: string;
  /** The picked lines as the diff shows them, with their +, - or space. */
  lines: string;
  /** Which lines they were (`lineId`), to mark them in the diff. */
  ids: string[];
  text: string;
}

export interface Review {
  note: string;
  comments: Comment[];
}

const kind = (l: string): DiffLine['k'] =>
  l.startsWith('+++') || l.startsWith('---') || l.startsWith('diff ') || l.startsWith('index ') ? 'dl-meta' : l.startsWith('@@') ? 'dl-hunk' : l.startsWith('+') ? 'dl-add' : l.startsWith('-') ? 'dl-del' : 'dl-ctx';

/** A unified diff's lines, each with its kind and number. */
export function parseDiff(text: string): DiffLine[] {
  let a = 0;
  let b = 0;
  return text
    .split('\n')
    .filter((l, i) => i > 0 || l)
    .map((text) => {
      const k = kind(text);
      const h = k === 'dl-hunk' ? /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(text) : null;
      if (h) [a, b] = [Number(h[1]), Number(h[2])];
      const n = k === 'dl-del' ? a++ : k === 'dl-add' ? b++ : k === 'dl-ctx' && !text.startsWith('\\') ? (a++, b++) : null;
      return { text, k, n };
    });
}

/** A numbered line's name within its file's diff: removed lines count in the old file, the others in the new one. */
export const lineId = (l: DiffLine): string | null => (l.n === null ? null : `${l.k === 'dl-del' ? '-' : '+'}${l.n}`);

const range = (ns: number[]) => (ns[0] === ns.at(-1) ? `${ns[0]}` : `${ns[0]}-${ns.at(-1)}`);

/** Where picked lines are: their numbers in the new file, or in the old one when all of them were removed. */
export function whereOf(picked: DiffLine[]): string {
  const now = picked.filter((l) => l.n !== null && l.k !== 'dl-del').map((l) => l.n!);
  if (now.length) return range(now);
  const old = picked.filter((l) => l.n !== null).map((l) => l.n!);
  return old.length ? `${range(old)} (removed)` : '';
}

/**
 * The text that is pasted: the note, then each comment as its file and lines, the diff lines in a fence
 * (longer than any run of backticks inside them) and the comment under it.
 *
 *   Please keep the public API as it is.
 *
 *   server/src/auth.rs 42-43
 *   ```diff
 *   -    let token = read()?;
 *   +    let token = cached()?;
 *   ```
 *   The cache can be stale here.
 */
export function formatReview(r: Review): string {
  const parts: string[] = [];
  if (r.note.trim()) parts.push(r.note.trim());
  for (const c of r.comments) {
    let longest = 2;
    for (const match of c.lines.matchAll(/`+/g)) longest = Math.max(longest, match[0].length);
    const fence = '`'.repeat(longest + 1);
    parts.push(`${c.path} ${c.where}\n${fence}diff\n${c.lines}\n${fence}\n${c.text.trim()}`);
  }
  return parts.join('\n\n');
}
