// Inserting text, images and files into a terminal. Nothing is ever submitted: the content is
// inserted as one paste and the user presses Enter when ready.
import type { Stream } from '../../types.ts';

export const MAX_PASTE_BYTES = 128 * 1024;

export function acceptsImages(s: Stream): boolean {
  return s.kind === 'agent' && (s.tool === 'claude' || s.tool === 'codex');
}
