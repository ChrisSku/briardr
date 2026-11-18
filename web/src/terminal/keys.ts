// Mapping keyboard events and key-bar presses to Herdr input.
// Herdr encodes named keys for the program's current mode (e.g. application cursor keys), so
// arrows and chords are sent by name; Home/End/PageUp/PageDown/Delete/Insert use standard sequences.
// Command is sent as Herdr's `super` modifier (the kitty keyboard protocol, e.g. ⌘A = CSI 97;9u).
// Herdr 0.9.4 and newer drop it, like terminals do, for a program that turned on no keyboard protocol.
export type Mod = 'ctrl' | 'alt' | 'shift' | 'super';
export type Outgoing = { keys: string[] } | { text: string } | { key: string };

const NAMED: Record<string, string> = {
  ArrowUp: 'up', ArrowDown: 'down', ArrowLeft: 'left', ArrowRight: 'right',
  Escape: 'esc', Tab: 'tab', Backspace: 'backspace', Enter: 'enter',
  F1: 'f1', F2: 'f2', F3: 'f3', F4: 'f4', F5: 'f5', F6: 'f6', F7: 'f7', F8: 'f8', F9: 'f9', F10: 'f10', F11: 'f11', F12: 'f12',
};
const RAW: Record<string, string> = { Home: 'home', End: 'end', PageUp: 'pageup', PageDown: 'pagedown', Delete: 'delete', Insert: 'insert' };

export function chord(base: string, m: Mod[]): string {
  const order: Mod[] = ['ctrl', 'alt', 'shift', 'super'];
  return [...order.filter((x) => m.includes(x)), base].join('+');
}

/**
 * Translate a hardware keydown into Herdr input, or null to let xterm produce text.
 * Hardware Command combinations are left to the browser (⌘C, ⌘V, ⌘K); the key bar's ⌘ is sent.
 */
export function fromKeyEvent(e: KeyboardEvent, extra: Mod[] = []): Outgoing | null {
  if (e.metaKey) return null;
  const m: Mod[] = [...new Set<Mod>([...(e.ctrlKey ? ['ctrl' as Mod] : []), ...(e.altKey ? ['alt' as Mod] : []), ...(e.shiftKey ? ['shift' as Mod] : []), ...extra])];
  if (RAW[e.key] && m.length === 0) return { key: RAW[e.key] };
  if (NAMED[e.key]) {
    const name = NAMED[e.key];
    // Return on a hardware keyboard is a real Enter (carriage return); Shift+Return inserts a line feed.
    if (name === 'enter' && m.length === 1 && m[0] === 'shift') return { text: '\n' };
    if (name === 'tab' && m.length === 1 && m[0] === 'shift') return { keys: ['shift+tab'] };
    return { keys: [chord(name, m)] };
  }
  if (e.key.length === 1 && (e.ctrlKey || (e.altKey && !isOptionComposed(e)))) {
    const base = e.code.startsWith('Key') ? e.code.slice(3).toLowerCase() : e.code.startsWith('Digit') ? e.code.slice(5) : e.key.toLowerCase();
    if (/^[a-z0-9]$/.test(base) || base === '@' || base === '[' || base === ']' || base === '\\') return { keys: [charChord(base, m)] };
  }
  return null;
}

/** On Mac keyboards Option often types characters (e.g. Option+2 = ™); keep those as text. */
function isOptionComposed(e: KeyboardEvent): boolean {
  return e.altKey && !e.ctrlKey && e.key.length === 1 && !/[a-z0-9]/i.test(e.key);
}

/**
 * A character's chord. Without Command it leaves Shift out, as legacy Control characters do (⇧⌃C is ⌃C).
 * Command reaches only a program with a keyboard protocol, which tells ⇧⌘Z from ⌘Z, so it keeps Shift.
 */
function charChord(c: string, m: Mod[]): string {
  return chord(c, m.includes('super') ? m : m.filter((x) => x !== 'shift'));
}

/** Apply key-bar modifiers to text typed on the on-screen keyboard. */
export function applyModsToText(text: string, m: Mod[]): Outgoing {
  const c = text.toLowerCase();
  // Command reaches the program as a kitty-protocol chord, so punctuation can carry it too.
  const chordable = m.includes('super') ? /^[a-z0-9@\[\]\\\/.,;'`=]$/ : /^[a-z0-9@\[\]\\]$/;
  if (!m.some((x) => x !== 'shift') || !chordable.test(c)) return { text: m.includes('shift') ? text.toUpperCase() : text };
  // A capital is its letter with Shift, as a keyboard reports it.
  return { keys: [charChord(c, c === text ? m : [...m, 'shift'])] };
}
