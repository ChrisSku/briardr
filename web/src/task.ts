// What an agent says it's doing: its terminal's title, shown under the stream's name in lists.
import type { Stream } from './types.ts';

type Titled = Pick<Stream, 'kind' | 'name' | 'tool' | 'title' | 'folder' | 'cwd'>;

/**
 * An agent's terminal title, unless it tells nothing the row doesn't: the stream's name, its tool or
 * its folder again. Shells and other programs set theirs to the folder or the command, so only agents
 * count.
 */
export function taskOf(s: Titled): string | null {
  const title = s.title?.trim();
  if (!title || s.kind !== 'agent') return null;
  const t = title.toLowerCase();
  const repeats = [s.name, s.tool, s.folder, s.cwd].some((x) => x && x.trim().toLowerCase() === t);
  // A path (the folder, in full or from the home folder) is where it runs, not what it does.
  return repeats || /^[~/]/.test(title) ? null : title;
}
