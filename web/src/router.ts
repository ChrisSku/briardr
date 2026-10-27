// History-based routing. The last path is remembered on this device and restored after a relaunch.
import { state } from './reactive.svelte.ts';
import { lastPath } from './store.ts';

/** "terminal" or a plugin's stream tab ID. A tab that isn't on shows the terminal (StreamScreen). */
export type Surface = string;
export type Route =
  | { name: 'agents' }
  | { name: 'workspaces' }
  | { name: 'settings'; section?: string }
  | { name: 'stream'; id: string; surface: Surface }
  /** An installed plugin's app tab, at /<tab>. */
  | { name: 'app'; tab: string };

export function parse(pathname: string): Route {
  const parts = pathname.split('/').filter(Boolean).map(decodeURIComponent);
  switch (parts[0]) {
    case undefined:
    case 'agents':
      return { name: 'agents' };
    case 'workspaces':
      return { name: 'workspaces' };
    case 'settings':
      return { name: 'settings', section: parts[1] };
    case 'usage':
      // Usage was a tab until it moved into Settings; old links and saved places still open it.
      return { name: 'settings', section: 'usage' };
    case 'stream':
      if (parts[1]) return { name: 'stream', id: parts[1], surface: parts[2] || 'terminal' };
      return { name: 'agents' };
    default:
      return { name: 'app', tab: parts[0] };
  }
}

export function pathOf(r: Route): string {
  switch (r.name) {
    case 'stream':
      return `/stream/${encodeURIComponent(r.id)}${r.surface === 'terminal' ? '' : '/' + r.surface}`;
    case 'settings':
      return r.section ? `/settings/${r.section}` : '/settings';
    case 'app':
      return `/${encodeURIComponent(r.tab)}`;
    default:
      return `/${r.name}`;
  }
}

const initialUrl = new URL(location.href);
let initialPath = initialUrl.pathname;
const fromAlert = initialUrl.searchParams.get('from') === 'alert';
if (initialPath === '/' || initialPath === '') initialPath = lastPath.value || '/agents';

export const route = state<Route>(parse(initialPath));
/** Opened from a notification tap: an explicit request to open that exact stream. */
export const openedFromAlert = state<boolean>(fromAlert);

history.replaceState(null, '', pathOf(route.value));
lastPath.value = pathOf(route.value);

export function navigate(r: Route, opts: { replace?: boolean } = {}) {
  const p = pathOf(r);
  if (opts.replace) history.replaceState(null, '', p);
  else if (p !== location.pathname) history.pushState(null, '', p);
  route.value = r;
  lastPath.value = p;
}

export function navigatePath(p: string) {
  const u = new URL(p, location.origin);
  if (u.searchParams.get('from') === 'alert') openedFromAlert.value = true;
  navigate(parse(u.pathname));
}

window.addEventListener('popstate', () => {
  route.value = parse(location.pathname);
  lastPath.value = location.pathname;
});
