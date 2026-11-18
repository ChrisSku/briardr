// What the Changes surface shows, as the host reports it: uncommitted work against HEAD, or everything
// since the branch the work started from (its commits and the uncommitted work).
import { persisted } from '@briardr/sdk';

/** Files marked reviewed on this device, by stream, with the change they were reviewed at (`signature`). */
export const reviewed = persisted<Record<string, Record<string, string>>>('briardr.reviewed', {});
/** A file's key in `reviewed`: a mark made against the base is its own, beside the one for uncommitted work
 *  (no path has a NUL in it, so the two never meet). */
export const markKey = (path: string, since: Since) => (since === 'base' ? `base\0${path}` : path);

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
