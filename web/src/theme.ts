// The page's theme, light or dark: web/src/theme-early.ts (in the page itself) sets it on <html>
// (data-theme) from this device's choice or the system's. Styles follow the attribute; code that draws its
// own colours (the terminal, diagrams) reads them from the styles again when it changes.
import { prefs } from './store.ts';

export type ThemeChoice = 'system' | 'light' | 'dark';
export const THEMES: Record<ThemeChoice, string> = { system: 'System', light: 'Light', dark: 'Dark' };

const root = document.documentElement;
export const isDark = () => root.dataset.theme === 'dark';

const listeners = new Set<() => void>();
new MutationObserver(() => listeners.forEach((fn) => fn())).observe(root, { attributes: true, attributeFilter: ['data-theme'] });

/** Calls `fn` when the theme changes; returns a function that stops it. */
export function onThemeChange(fn: () => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** This device's choice; before it was ever made, the system's. */
export const themeChoice = (): ThemeChoice => prefs.value.theme ?? 'system';

export function setTheme(theme: ThemeChoice) {
  prefs.value = { ...prefs.value, theme };
  dispatchEvent(new CustomEvent('briardr:theme', { detail: theme }));
}
