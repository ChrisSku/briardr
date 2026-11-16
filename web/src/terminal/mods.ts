// The key bar's modifiers, shared by the key bar and the terminal: each is off, on for the next key only,
// locked, or held. keys.ts applies them to what is sent.
import { state } from '../reactive.svelte.ts';
import type { Mod } from './keys.ts';

export type ModMode = 'off' | 'once' | 'locked' | 'held';
export const mods = state<Record<Mod, ModMode>>({ ctrl: 'off', alt: 'off', shift: 'off', super: 'off' });

export function activeMods(): Mod[] {
  return (Object.keys(mods.value) as Mod[]).filter((m) => mods.value[m] !== 'off');
}

/** After a key is sent, one-shot modifiers turn off; locked and held ones stay. */
export function consumeMods() {
  const m = { ...mods.value };
  let changed = false;
  for (const k of Object.keys(m) as Mod[])
    if (m[k] === 'once') {
      m[k] = 'off';
      changed = true;
    }
  if (changed) mods.value = m;
}
