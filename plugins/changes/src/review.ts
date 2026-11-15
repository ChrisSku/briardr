// Review comments on the changes: lines picked in a diff, each with what to do about them, sent to the
// terminal as one paste with an optional note in front. Kept on this device per stream, so they survive
// switching surfaces and relaunches until they are sent or cleared. Each comment keeps the lines it was
// written on, as they were then.
import { codeBlock, persisted } from '@briardr/sdk';

export interface Comment {
  path: string;
  /** The file's change when the comment was written (`ChangedFile.signature`): `ids` name lines of it. */
  signature: string;
  /** Where the lines are, as the diff numbers them: "42", "42-44", or "40 (removed)". */
  where: string;
  /** The picked lines as the diff shows them, with their +, - or space. */
  lines: string;
  /** Which lines they were (`lineId`), to mark them in the diff while the file is unchanged. */
  ids: string[];
  text: string;
}

export interface Review {
  note: string;
  comments: Comment[];
}

const reviews = persisted<Record<string, Review>>('briardr.changes.review', {});
const EMPTY: Review = { note: '', comments: [] };

export const reviewOf = (sid: string): Review => reviews.value[sid] ?? EMPTY;

function put(sid: string, r: Review) {
  const next = { ...reviews.value };
  if (r.comments.length || r.note) next[sid] = r;
  else delete next[sid];
  reviews.value = next;
}
export const addComment = (sid: string, c: Comment) => put(sid, { ...reviewOf(sid), comments: [...reviewOf(sid).comments, c] });
export function removeComment(sid: string, i: number) {
  const r = reviewOf(sid);
  put(sid, { ...r, comments: r.comments.filter((_, j) => j !== i) });
}
export const setNote = (sid: string, note: string) => put(sid, { ...reviewOf(sid), note });
export const clearReview = (sid: string) => put(sid, EMPTY);

/**
 * The text that is pasted: the note, then each comment as its file and lines, the diff lines in a code
 * block and the comment under it.
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
  for (const c of r.comments) parts.push(`${c.path} ${c.where}\n${codeBlock(c.lines, 'diff')}\n${c.text.trim()}`);
  return parts.join('\n\n');
}
