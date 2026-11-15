// History-based routing between the places in routes.ts. The last path is remembered on this device and
// restored after a relaunch.
import { state } from './reactive.svelte.ts';
import { parse, pathOf, type Route } from './routes.ts';
import { lastPath } from './store.ts';

const STREAM_LOCK = 'briardr.stream.';

const initialUrl = new URL(location.href);
let initialPath = initialUrl.pathname;
const fromAlert = initialUrl.searchParams.get('from') === 'alert';
/** The one-time code a pairing link brought in its fragment (the QR code in Settings → Devices and from
 *  `briardr pair`). The address loses it right below: a code has no place in the history. */
export const linkedCode = /^#pair=([A-Za-z0-9]{4}-?[A-Za-z0-9]{4})$/.exec(initialUrl.hash)?.[1].toUpperCase() ?? '';
const restoring = initialPath === '/' || initialPath === '';
if (restoring) initialPath = lastPath.value || '/agents';

export const route = state<Route>(parse(initialPath));
/** Opened from a notification tap: an explicit request to open that exact stream. */
export const openedFromAlert = state<boolean>(fromAlert);

history.replaceState(null, '', pathOf(route.value));
lastPath.value = pathOf(route.value);

/** Marks a stream as open in this window until the returned function runs; the lock ends with the window. */
export function holdStream(id: string): () => void {
  let release = () => {};
  void navigator.locks?.request(STREAM_LOCK + id, { mode: 'shared' }, () => new Promise<void>((done) => (release = done)));
  return () => release();
}
// A second window opens at / too (New Window, the Dock): it starts on Agents rather than on the stream
// another window is using, and leaves that window's place to be restored after a relaunch.
if (restoring && route.value.name === 'stream') {
  const lock = STREAM_LOCK + route.value.id;
  void navigator.locks?.query().then(({ held }) => {
    if (!held?.some((l) => l.name === lock)) return;
    history.replaceState(null, '', '/agents');
    route.value = { name: 'agents' };
  });
}

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
