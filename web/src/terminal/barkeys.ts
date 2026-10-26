// The keys you can put in the key bar's top row, and which of them it shows: a default row, and
// optionally a row of its own for an app (the pane's foreground program; every shell is "shell"),
// used instead of the default while that app runs.
// Device-only, like the other terminal preferences.
import { persisted } from '../reactive.svelte.ts';

export interface BarKey {
  id: string;
  /** What the key shows. */
  label: string;
  /** Its accessible name. */
  name: string;
  /** Text and named keys take the bar's modifiers; chords and raw keys are sent as they are. */
  send: { text: string } | { named: string } | { keys: string[] } | { key: string };
}

const text = (c: string, name = c): BarKey => ({ id: c, label: c === '-' ? '−' : c, name, send: { text: c } });
const ctrl = (c: string): BarKey => ({ id: `ctrl+${c}`, label: `⌃${c.toUpperCase()}`, name: `Control-${c.toUpperCase()}`, send: { keys: [`ctrl+${c}`] } });
const raw = (k: string, label: string, name: string): BarKey => ({ id: k, label, name, send: { key: k } });
const fn = (n: number): BarKey => ({ id: `f${n}`, label: `F${n}`, name: `F${n}`, send: { named: `f${n}` } });

export const CATALOG: { title: string; keys: BarKey[] }[] = [
  { title: 'Symbols', keys: [...'|~/', '-', ...'\\_`:;\'"=+*&!?$#@%^<>()[]{}'].map((c) => (c === '-' ? text(c, 'Hyphen') : text(c))) },
  { title: 'Control', keys: [...'abcdegklorstuwxz'].map(ctrl) },
  {
    title: 'Keys',
    keys: [
      { id: 'shift+tab', label: '⇤', name: 'Shift-Tab', send: { keys: ['shift+tab'] } },
      { id: 'tab', label: '⇥', name: 'tab (Tab)', send: { named: 'tab' } },
      raw('home', 'Home', 'Home'),
      raw('end', 'End', 'End'),
      raw('pageup', 'PgUp', 'Page Up'),
      raw('pagedown', 'PgDn', 'Page Down'),
      raw('delete', 'Del', 'Forward Delete'),
      ...Array.from({ length: 12 }, (_, i) => fn(i + 1)),
    ],
  },
];
export const BAR_KEYS = new Map(CATALOG.flatMap((g) => g.keys).map((k) => [k.id, k]));

export const DEFAULT_KEYS = ['|', '~', '/', '-', 'ctrl+c', 'ctrl+d', 'ctrl+s', 'shift+tab', 'tab'];
/** More keys than this get too narrow to tap on a phone. */
export const MAX_KEYS = 10;

export interface BarKeyConfig {
  default: string[];
  apps: Record<string, string[]>;
}
export const barKeys = persisted<BarKeyConfig>('briardr.keybar', { default: DEFAULT_KEYS, apps: {} });

/** The app whose keys a pane shows. */
export function appOf(s: { kind: string; tool: string }): string {
  return s.kind === 'shell' ? 'shell' : s.tool;
}

const LABELS: Record<string, string> = { shell: 'Shell', claude: 'Claude', codex: 'Codex', nvim: 'Neovim', vim: 'Vim', hx: 'Helix', emacs: 'Emacs' };
export function appLabel(app: string): string {
  return LABELS[app] ?? app.charAt(0).toUpperCase() + app.slice(1);
}

/** The key IDs for an app (null: the default keys), falling back to the default keys. */
export function keyIds(app: string | null): string[] {
  const c = barKeys.value;
  const own = app ? c?.apps?.[app] : undefined;
  const ids = Array.isArray(own) ? own : Array.isArray(c?.default) ? c.default : DEFAULT_KEYS;
  return ids.filter((id) => BAR_KEYS.has(id)).slice(0, MAX_KEYS);
}

export function rowFor(app: string | null): BarKey[] {
  return keyIds(app).map((id) => BAR_KEYS.get(id)!);
}
