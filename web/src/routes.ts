// The app's places and their addresses. The router (router.ts) keeps the current one in the window's history.

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
  let parts: string[];
  try {
    parts = pathname.split('/').filter(Boolean).map(decodeURIComponent);
  } catch {
    return { name: 'agents' }; // a stray % in a typed address
  }
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
