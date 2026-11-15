// A changed file's unified diff as Changes shows it: each line with its kind and number.

export interface DiffLine {
  text: string;
  k: 'dl-meta' | 'dl-hunk' | 'dl-add' | 'dl-del' | 'dl-ctx';
  /** The line's number: the old file's for a removed line, the new file's otherwise (from the hunk headers). */
  n: number | null;
}

const kind = (l: string): DiffLine['k'] =>
  l.startsWith('+++') || l.startsWith('---') || l.startsWith('diff ') || l.startsWith('index ') ? 'dl-meta' : l.startsWith('@@') ? 'dl-hunk' : l.startsWith('+') ? 'dl-add' : l.startsWith('-') ? 'dl-del' : 'dl-ctx';

/** A unified diff's lines, each with its kind and number. */
export function parseDiff(text: string): DiffLine[] {
  let a = 0;
  let b = 0;
  const lines = text.split('\n');
  // The diff ends with a line break, which is not one more line of it (an empty context line is a space).
  if (lines.at(-1) === '') lines.pop();
  return lines
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
