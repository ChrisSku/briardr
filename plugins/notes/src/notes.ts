// Notes on this device: the cached list from the Mac, unsaved drafts, and the note open in the editor.
// - Each save says which revision of the note it was based on; if another device saved (or deleted the
//   note) in between, nothing is overwritten and the user chooses which version to keep.
// - While the Mac is unreachable, edits are kept as drafts on this device and saved when it's back.
import { ApiError, state, type PluginContext, type StreamContext } from '@briardr/sdk';

export interface Note {
  id: string;
  text: string;
  revision: number;
  createdAt: number;
  updatedAt: number;
}
/** An edit not yet saved on the Mac. `base` is the revision it was based on (0: a new note). */
export interface Draft {
  text: string;
  base: number;
  createdAt: number;
}

export const MAX_BYTES = 256 * 1024;
export const enc = new TextEncoder();

export const listKey = (sid: string) => `briardr.notes.list.${sid}`;
const draftPrefix = (sid: string) => `briardr.notes.draft.${sid}/`;
export const draftKey = (sid: string, nid: string) => draftPrefix(sid) + nid;
const openKey = (sid: string) => `briardr.notes.open.${sid}`;
export const notesPath = (sid: string) => `/api/streams/${encodeURIComponent(sid)}/notes`;

/** Bumped whenever this device changes its copy of the notes, so the list re-reads it. */
export const localChange = state(0);
/** Notes being saved right now; the list doesn't also send their drafts. */
export const inflight = new Set<string>();

export function load<T>(k: string): T | null {
  try {
    const v = localStorage.getItem(k);
    return v ? (JSON.parse(v) as T) : null;
  } catch {
    return null;
  }
}
/** Writes (or with null, removes) a value. False if this device couldn't store it (full, or not allowed). */
export function store(k: string, v: unknown | null): boolean {
  let ok = true;
  try {
    if (v === null) localStorage.removeItem(k);
    else localStorage.setItem(k, JSON.stringify(v));
  } catch {
    ok = false;
  }
  localChange.value++;
  return ok;
}
export function drafts(sid: string): Record<string, Draft> {
  const out: Record<string, Draft> = {};
  try {
    for (let i = 0; i < localStorage.length; i++) {
      const k = localStorage.key(i);
      if (k?.startsWith(draftPrefix(sid))) {
        const d = load<Draft>(k);
        if (d) out[k.slice(draftPrefix(sid).length)] = d;
      }
    }
  } catch {}
  return out;
}
/** The note open in the editor survives a reload of this tab. */
export function openNote(sid: string): string | null {
  try {
    return sessionStorage.getItem(openKey(sid));
  } catch {
    return null;
  }
}
export function setOpenNote(sid: string, id: string | null) {
  try {
    if (id) sessionStorage.setItem(openKey(sid), id);
    else sessionStorage.removeItem(openKey(sid));
  } catch {}
}

/** Update this device's copy of the list: replace or add a note, or remove it (null). */
export function cacheNote(sid: string, id: string, n: Note | null) {
  const list = (load<Note[]>(listKey(sid)) ?? []).filter((x) => x.id !== id);
  if (n) list.push(n);
  store(listKey(sid), list.sort((a, b) => b.createdAt - a.createdAt));
}

export const newId = () => Array.from(crypto.getRandomValues(new Uint8Array(8)), (b) => b.toString(16).padStart(2, '0')).join('');

export function copy(briardr: PluginContext, text: string) {
  navigator.clipboard.writeText(text).then(
    () => briardr.toast('Copied'),
    () => briardr.toast('Copying isn’t allowed here.'),
  );
}
const HAS_IMAGE = /note:[a-f0-9]{24}\.png/;

/** Paste a note (or part of it) as one paste, never pressing Enter. Images go as the paths of copies on
 *  the Mac, which Claude Code and Codex attach as images; `note:` references mean nothing to them. */
export async function sendToTerminal(briardr: StreamContext, sid: string, text: string) {
  const toast = (t: string) => briardr.toast(t);
  if (!text.trim()) return toast('The note is empty.');
  // If the connection changes while the Mac copies the images, nothing is pasted.
  const gen = briardr.terminal.generation;
  let missing = 0;
  if (HAS_IMAGE.test(text)) {
    try {
      ({ text, missing } = await briardr.core<{ text: string; missing: number }>('POST', `${notesPath(sid)}/paste`, { text }));
    } catch (e) {
      return toast(e instanceof ApiError ? e.message : 'Couldn’t prepare the note’s images.');
    }
  }
  if (!briardr.terminal.send(text, { generation: gen })) return;
  toast(missing ? `Sent, but ${missing === 1 ? 'an image wasn’t' : `${missing} images weren’t`} found on ${briardr.host().the}. Press Enter there when ready.` : 'Sent to the terminal. Press Enter there when ready.');
}
