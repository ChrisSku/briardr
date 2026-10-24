// How `changeset version` writes CHANGELOG.md entries (.changeset/config.json points here): each
// changeset's summary as written, as one bullet, followed by a link to the pull request that added it.
// main only takes squash merges, so the commit that added the changeset is the merge, and its subject
// ends in "(#123)".
import { execFileSync } from 'node:child_process';

const REPO = 'https://github.com/ChrisSku/briardr';

function pullRequest(commit: string | undefined): string | null {
  if (!commit) return null;
  const subject = execFileSync('git', ['log', '-1', '--format=%s', commit], { encoding: 'utf8' }).trim();
  return subject.match(/\(#(\d+)\)$/)?.[1] ?? null;
}

export function getReleaseLine(changeset: { summary: string; commit?: string }): string {
  const pr = pullRequest(changeset.commit);
  const [first, ...rest] = changeset.summary.trim().split('\n').map((line) => line.trimEnd());
  const lines = [`- ${first}`, ...rest.map((line) => (line ? `  ${line}` : line))];
  if (pr) lines[lines.length - 1] += ` ([#${pr}](${REPO}/pull/${pr}))`;
  return lines.join('\n');
}

/** Briardr is one package, so no dependency of it is ever released alongside. */
export function getDependencyReleaseLine(): string {
  return '';
}
