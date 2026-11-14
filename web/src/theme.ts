// The page's theme, light or dark: web/public/theme.js sets it on <html> (data-theme) from this
// device's choice or the system's. Styles follow the attribute; code that draws its own colours (the
// terminal, diagrams) asks here.
import { prefs } from './store.ts';

export type ThemeChoice = 'system' | 'light' | 'dark';

const root = document.documentElement;
export const isDark = () => root.dataset.theme === 'dark';

const listeners = new Set<(dark: boolean) => void>();
new MutationObserver(() => listeners.forEach((fn) => fn(isDark()))).observe(root, { attributes: true, attributeFilter: ['data-theme'] });

/** Calls `fn` when the theme changes; returns a function that stops it. */
export function onThemeChange(fn: (dark: boolean) => void): () => void {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

/** This device's choice; before it was ever made, the system's. */
export const themeChoice = (): ThemeChoice => prefs.value.theme ?? 'system';

export function setTheme(theme: ThemeChoice) {
  prefs.value = { ...prefs.value, theme };
  dispatchEvent(new Event('briardr:theme'));
}
