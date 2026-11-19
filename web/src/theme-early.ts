// Light or dark, decided before the page first paints: this device's choice (Settings → Appearance, kept in
// briardr.prefs), else the system's. The build puts this script into the page itself, ahead of the styles
// (scripts/build.ts): the app's modules run only after the first paint. The service worker's page for a
// launch without a saved copy of the app has it too (web/src/sw.ts). The styles follow data-theme on
// <html>, and so does whatever draws its own colours (web/src/theme.ts).
import type { ThemeChoice } from './theme.ts';

/** The status bar and the browser's own bars take the page's canvas, --bg-canvas in web/src/styles.css. */
const CANVAS = { light: '#f6f5f3', dark: '#0d0c0b' };
const system = matchMedia('(prefers-color-scheme: dark)');
const stored = (): ThemeChoice | undefined => {
  try {
    return JSON.parse(localStorage.getItem('briardr.prefs') || 'null')?.theme;
  } catch {}
};
let choice = stored();

function apply() {
  const theme = choice === 'dark' || (choice !== 'light' && system.matches) ? 'dark' : 'light';
  const root = document.documentElement;
  if (root.dataset.theme === theme) return;
  root.dataset.theme = theme;
  document.querySelector('meta[name="theme-color"]')?.setAttribute('content', CANVAS[theme]);
}

apply();
system.addEventListener('change', apply);
// The choice changed: in another window, or in this one, which says so (its storage may not have taken it).
addEventListener('storage', (e: StorageEvent) => {
  if (e.key !== 'briardr.prefs' && e.key !== null) return;
  choice = stored();
  apply();
});
addEventListener('briardr:theme', (e) => {
  choice = (e as CustomEvent<ThemeChoice>).detail;
  apply();
});
