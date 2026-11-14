// What the Changes surface shows, as the host reports it: uncommitted work against HEAD, or everything
// since the branch the work started from (its commits and the uncommitted work).
import { persisted } from '@briardr/sdk';
import type { Comment, Review } from './review.ts';

/** Files marked reviewed on this device, by stream, with the change they were reviewed at (`signature`). */
export const reviewed = persisted<Record<string, Record<string, string>>>('briardr.reviewed', {});
/** A file's key in `reviewed`: a mark made against the base is its own, beside the one for uncommitted work. */
export const markKey = (path: string, since: Since) => (since === 'base' ? `base:${path}` : path);

export interface ChangedFile {
  path: string;
  origPath: string | null;
  staged: boolean;
  unstaged: boolean;
  untracked: boolean;
  conflicted: boolean;
  change: string;
  modeChange: { from: string; to: string } | null;
  binary: boolean;
  additions: number | null;
  deletions: number | null;
  signature: string;
}
export interface Report {
  state: 'ok' | 'clean' | 'not_repo' | 'git_missing' | 'error';
  message?: string;
  branch: string | null;
  detached: boolean;
  headShort: string | null;
  noCommits: boolean;
  files: ChangedFile[];
  totalFiles: number;
  truncated: boolean;
  asOf: number;
  /** The branch the work started from, when HEAD has commits of its own since then. */
  base: { name: string; short: string; ahead: number } | null;
  /** What the files are compared with: HEAD (uncommitted work), or `base`. */
  since: Since;
}
export type Since = 'head' | 'base';

export const LETTER: Record<string, string> = { added: 'A', modified: 'M', deleted: 'D', renamed: 'R', copied: 'C', type: 'T', untracked: 'U', conflict: '!' };

// ---------- review comments, kept on this device per stream until they are sent or cleared ----------
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
