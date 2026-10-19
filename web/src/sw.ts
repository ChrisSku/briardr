/// <reference lib="webworker" />
// Briardr service worker: offline app shell, push alerts, and notification taps.
// It never caches API responses or terminal data.
//
// Shell caches: one per build (briardr-shell-<version>), written only at install, never from page loads,
// so a cached page always comes with the assets it was built with. Activation keeps this build's cache
// and the one before it: pages still running the previous build load their screens' code from it until
// they reload (web/src/updates.ts). Older shells are deleted.
import { alertPath, pickWindow } from './alert-window.ts';

declare const __BRIARDR_VERSION__: string;
declare const __BRIARDR_ASSETS__: string[];
declare const __BRIARDR_ICON__: string;
const sw = self as unknown as ServiceWorkerGlobalScope;
const SHELL = 'briardr-shell-';
const CACHE = SHELL + __BRIARDR_VERSION__;
const PLUGINS = 'briardr-plugins';
/** Caches from when Briardr was called Kelpie: its last shell counts as the previous build, the rest goes. */
const LEGACY_SHELL = 'kelpie-shell-';
/** How long a launch waits for the network before opening the cached app instead. */
const NAVIGATION_TIMEOUT = 3000;
/** Answers from a proxy (or a host that's restarting) that mean "try again later": open the cached app. */
const TRANSIENT = new Set([502, 503, 504]);

const versionOf = (html: string) => html.match(/<meta name="briardr-version" content="([^"]*)"/)?.[1] ?? null;
/** Content-addressed files: the same URL is the same bytes in every build, so any shell's copy will do. */
const hashed = (path: string) => path.startsWith('/assets/') || /\.[0-9a-f]{10}\.png$/.test(path);

/** Saves this build's shell. Fails, saving nothing, unless every file arrives and the page is this build's. */
async function saveShell() {
  const cache = await caches.open(CACHE);
  try {
    // Unhashed files (the page, the plugin runtime's stable names, the manifest) are revalidated.
    await cache.addAll(__BRIARDR_ASSETS__.map((u) => (hashed(u) ? u : new Request(u, { cache: 'no-cache' }))));
    // A deployment between fetching sw.js and the page would pair another build's page with these assets.
    const page = await cache.match('/');
    const version = page ? versionOf(await page.text()) : null;
    if (version !== __BRIARDR_VERSION__) throw new Error(`the page is build ${version}, not ${__BRIARDR_VERSION__}`);
  } catch (err) {
    await caches.delete(CACHE);
    throw err;
  }
}

// If installing fails, the previous worker and its shell stay in charge; the next update check tries again.
sw.addEventListener('install', (e) => e.waitUntil(saveShell().then(() => sw.skipWaiting())));

sw.addEventListener('activate', (e) => {
  e.waitUntil(
    (async () => {
      // caches.keys() lists caches in the order they were made: the last other shell is the previous build.
      const keys = await caches.keys();
      const older = keys.filter((k) => (k.startsWith(SHELL) || k.startsWith(LEGACY_SHELL)) && k !== CACHE);
      await Promise.all(older.slice(0, -1).map((k) => caches.delete(k)));
      if (keys.includes('kelpie-plugins')) await caches.delete('kelpie-plugins');
      await sw.clients.claim();
      // Pages on another build reload into this one when they're idle (web/src/updates.ts).
      for (const c of await sw.clients.matchAll({ type: 'window' })) c.postMessage({ t: 'shell', version: __BRIARDR_VERSION__ });
    })(),
  );
});

sw.addEventListener('fetch', (e) => {
  const url = new URL(e.request.url);
  if (url.origin !== sw.location.origin || e.request.method !== 'GET') return;
  if (url.pathname.startsWith('/api/') || url.pathname.startsWith('/ws/')) return;
  if (e.request.mode === 'navigate') return e.respondWith(navigation(e));
  if (url.pathname.startsWith('/assets/') || url.pathname.startsWith('/icons/') || url.pathname.startsWith('/sdk/')) {
    // A stable name (the plugin runtime) must come from this build's shell, never a previous one.
    const cached = hashed(url.pathname) ? caches.match(e.request) : caches.open(CACHE).then((c) => c.match(e.request));
    e.respondWith(cached.then((r) => r || fetch(e.request)));
  } else if (url.pathname.startsWith('/plugins/')) {
    // Installed plugins' files have hashed names, so a copy is good for as long as it's asked for.
    e.respondWith(
      caches.match(e.request).then(
        (r) =>
          r ||
          fetch(e.request).then((res) => {
            if (res.ok) e.waitUntil(keepPluginFile(e.request, res.clone()));
            return res;
          }),
      ),
    );
  }
});

/**
 * Network first, so a launch gets the current build. The cached shell answers when the network fails,
 * is slower than NAVIGATION_TIMEOUT, or a proxy says the host is unavailable. Other answers (sign-in
 * pages, redirects, errors) pass through. Without a cached shell, the launch waits for the network.
 */
async function navigation(e: FetchEvent): Promise<Response> {
  const shell = await caches.open(CACHE).then((c) => c.match('/'));
  const network = fetch(e.request);
  if (!shell) {
    // The shell was cleared (storage pressure, or Settings → Diagnostics): save it again while online.
    return network.then(
      (r) => {
        if (r.ok) e.waitUntil(saveShell().catch(() => {}));
        return r;
      },
      () => offlinePage(),
    );
  }
  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<null>((resolve) => (timer = setTimeout(() => resolve(null), NAVIGATION_TIMEOUT)));
  try {
    const r = await Promise.race([network, timeout]);
    return r && !TRANSIENT.has(r.status) ? r : shell;
  } catch {
    return shell;
  } finally {
    clearTimeout(timer);
  }
}

/** Shown when the app was never saved for offline use (or its copy was cleared) and the host can't be reached. */
function offlinePage(): Response {
  const html = `<!doctype html>
<html lang="en"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1, viewport-fit=cover">
<meta name="color-scheme" content="light dark"><meta name="briardr-offline" content="1"><title>Briardr</title>
<style>
:root{--bg:#fff;--fg:#1c1c1e;--muted:#6c6c70;--accent:#0a66d8}
@media (prefers-color-scheme:dark){:root{--bg:#000;--fg:#f2f2f7;--muted:#98989f;--accent:#4a9eff}}
body{margin:0;min-height:100vh;display:grid;place-items:center;background:var(--bg);color:var(--fg);font:17px/1.4 -apple-system,system-ui,sans-serif;padding:16px;box-sizing:border-box}
main{max-width:26rem}h1{font-size:22px;margin:0 0 8px}p{margin:0 0 16px;color:var(--muted)}
button{font:inherit;font-weight:600;color:#fff;background:var(--accent);border:0;border-radius:10px;padding:10px 18px;min-height:44px}
</style></head><body><main>
<h1>Briardr can’t open right now</h1>
<p id="why">Briardr can’t reach its host, and this device doesn’t have a saved copy of the app to open offline yet.</p>
<p>It opens again as soon as the host is reachable.</p>
<button id="retry" type="button">Try again</button>
</main><script>
if (!navigator.onLine) document.getElementById('why').textContent = 'This device is offline, and it doesn’t have a saved copy of Briardr to open offline yet.';
document.getElementById('retry').onclick = () => location.reload();
addEventListener('online', () => location.reload(), { once: true });
</script></body></html>`;
  return new Response(html, { status: 503, headers: { 'content-type': 'text/html; charset=utf-8', 'cache-control': 'no-store' } });
}

/** Caches a plugin file, keeping the most recent ones (every rebuild of a plugin has new names). */
async function keepPluginFile(req: Request, res: Response) {
  const cache = await caches.open(PLUGINS);
  await cache.put(req, res);
  const keys = await cache.keys();
  for (const k of keys.slice(0, Math.max(0, keys.length - 200))) await cache.delete(k);
}

/** Whether this build's shell is complete, for Settings (web/src/offline.ts). */
async function shellStatus() {
  const cache = await caches.open(CACHE);
  let missing = 0;
  for (const u of __BRIARDR_ASSETS__) if (!(await cache.match(u))) missing++;
  return { t: 'status', version: __BRIARDR_VERSION__, missing, total: __BRIARDR_ASSETS__.length };
}

sw.addEventListener('message', (e) => {
  if (e.data?.t === 'status') e.waitUntil(shellStatus().then((s) => e.ports[0]?.postMessage(s)));
});

interface Payload {
  id: string;
  kind: string;
  title: string;
  body: string;
  streamId?: string;
  tag: string;
  ts: number;
  /** Agents that need attention when it was sent (alerts from the monitor only). */
  attention?: number;
}

sw.addEventListener('push', (e) => {
  let p: Payload;
  try {
    p = e.data!.json();
  } catch {
    p = { id: '', kind: 'unknown', title: 'Briardr', body: '', tag: 'briardr', ts: Date.now() };
  }
  e.waitUntil(
    (async () => {
      await sw.registration.showNotification(p.title, {
        body: p.body,
        tag: p.tag,
        data: { streamId: p.streamId || null, kind: p.kind, id: p.id },
        icon: __BRIARDR_ICON__,
        badge: __BRIARDR_ICON__,
        timestamp: p.ts,
      } as NotificationOptions);
      // The icon badge, from the host's count when the alert was sent; other pushes leave it alone.
      if (typeof p.attention === 'number' && 'setAppBadge' in navigator) await (p.attention > 0 ? navigator.setAppBadge(p.attention) : navigator.clearAppBadge()).catch(() => {});
      // Tell the Mac this device really received it; only then does Briardr report "delivered".
      if (p.id) {
        try {
          await fetch('/api/notifications/ack', { method: 'POST', credentials: 'same-origin', headers: { 'content-type': 'application/json' }, body: JSON.stringify({ id: p.id }) });
        } catch {}
      }
      const clients = await sw.clients.matchAll({ type: 'window' });
      for (const c of clients) c.postMessage({ t: 'push', payload: p });
    })(),
  );
});

sw.addEventListener('notificationclick', (e) => {
  e.notification.close();
  const streamId: string | null = e.notification.data?.streamId || null;
  const target = alertPath(streamId);
  e.waitUntil(
    (async () => {
      // An open window is brought forward and told where to go (main.ts); without one, a window opens there.
      const win = pickWindow((await sw.clients.matchAll({ type: 'window', includeUncontrolled: true })) as WindowClient[], streamId);
      if (!win) return void (await sw.clients.openWindow(target));
      await win.focus().catch(() => {});
      win.postMessage({ t: 'open', path: target });
    })(),
  );
});
