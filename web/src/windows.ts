// Several windows on one computer: each holds its own live connection and controls its own stream. Windows
// share this device's stored state (reactive.svelte.ts keeps it in step), and a pane has one controller, so
// a window opening a stream another one controls watches it instead.
import { platform } from './install.ts';
import { pathOf, type Route } from './router.ts';
import { toast } from './store.ts';

/** Phones and tablets run one window of an installed app, and window.open only adds a tab. */
export const canOpenWindows = !['iphone', 'ipad', 'android'].includes(platform());

export function openWindow(r: Route = { name: 'agents' }) {
  const w = window.open(pathOf(r), '_blank', `popup,width=${window.outerWidth},height=${window.outerHeight}`);
  if (!w) toast('The browser blocked the new window. Allow pop-ups for Briardr, or use its New Window command.', 5000);
}
