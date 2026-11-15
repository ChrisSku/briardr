// Inserting text, images and files into a terminal. Nothing is ever submitted: the content is
// inserted as one paste and the user presses Enter when ready.
import type { Stream } from '../../types.ts';

export const MAX_PASTE_BYTES = 128 * 1024;

export function acceptsImages(s: Stream): boolean {
  return s.kind === 'agent' && (s.tool === 'claude' || s.tool === 'codex');
}

/** `code` as a Markdown code block for a prompt, its fence longer than any run of backticks inside, so
 *  code that holds fences stays whole. `info` is the language (`ts`, `diff`), or nothing. */
export function codeBlock(code: string, info = ''): string {
  let longest = 2;
  for (const match of code.matchAll(/`+/g)) longest = Math.max(longest, match[0].length);
  const fence = '`'.repeat(longest + 1);
  return `${fence}${info}\n${code}\n${fence}`;
}

// ---------- Compose: this device's drafts and the messages inserted last ----------

export const MAX_RECENT = 20;
/** A longer message isn't offered again: twenty of them would fill this browser's storage. */
const MAX_RECENT_LENGTH = 4000;
const MAX_DRAFTS = 30;

/** The messages inserted last with `text` in front: each once, the newest first. */
export function remembered(recent: string[], text: string): string[] {
  const t = text.trim();
  if (!t || t.length > MAX_RECENT_LENGTH) return recent;
  return [t, ...recent.filter((r) => r !== t)].slice(0, MAX_RECENT);
}

/** The drafts with `text` as the stream's, or without one when it's empty; the oldest go first. */
export function withDraft(drafts: Record<string, string>, streamId: string, text: string): Record<string, string> {
  const rest = Object.entries(drafts).filter(([id]) => id !== streamId);
  if (text.trim()) rest.push([streamId, text]);
  return Object.fromEntries(rest.slice(-MAX_DRAFTS));
}
