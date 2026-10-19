// What the Changes surface shows: uncommitted work against HEAD, as the Mac reports it.
import { persisted } from '@briardr/sdk';

/** Files marked reviewed on this device, by stream, with the change they were reviewed at (`signature`). */
export const reviewed = persisted<Record<string, Record<string, string>>>('briardr.reviewed', {});

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
}

export const LETTER: Record<string, string> = { added: 'A', modified: 'M', deleted: 'D', renamed: 'R', copied: 'C', type: 'T', untracked: 'U', conflict: '!' };
