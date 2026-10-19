import './install.ts'; // first: the browser's install prompt fires early
import '@xterm/xterm/css/xterm.css';
import './styles.css';
import { mount } from 'svelte';
import { api, ApiError } from './api.ts';
import App from './App.svelte';
import { record } from './diag.ts';
import { showAttentionBadge } from './badge.svelte.ts';
import { pruneScreens } from './idb.ts';
import { whenIdle } from './idle.ts';
import { liveConn } from './live.ts';
import { registerShell } from './offline.ts';
import { preloadBuiltins, setPlugins } from './plugins/registry.svelte.ts';
import { watchPush } from './push.ts';
import { preloadScreens, streamScreen } from './screens/lazy.svelte.ts';
import { navigatePath, route } from './router.ts';
import { host, mac, session, wasSignedIn } from './store.ts';
import { rememberServer, shellActivated } from './updates.ts';

// Opening on a stream (a relaunch restores the last screen): fetch the terminal alongside the session.
if (route.value.name === 'stream') void streamScreen.load();

async function boot() {
  record('app_start', { standalone: matchMedia('(display-mode: standalone)').matches });
  // A device that was signed in opens on its cached state (labeled stale) without waiting for the host,
  // which may be slow or unreachable; the check below still signs it out if it was revoked meanwhile.
  if (wasSignedIn) session.value = { status: 'signed_in', device: { id: '', name: '' } };
  try {
    const r = await api('GET', '/api/session', undefined, { timeout: 8000 });
    rememberServer(r.instanceId);
    if (r.host) host.value = r.host;
    setPlugins(r.plugins);
    session.value = { status: 'signed_in', device: r.device };
  } catch (e) {
    if (e instanceof ApiError && e.data?.host) host.value = { ...host.value, ...e.data.host };
    if (e instanceof ApiError && e.category === 'auth') session.value = { status: 'signed_out' };
    // Offline at launch: keep showing cached state, labeled stale; the live connection keeps retrying.
    else session.value = { status: 'signed_in', device: { id: '', name: '' } };
  }
  if (session.value.status === 'signed_in') {
    // Plugins turned on or off on another device.
    liveConn.on((msg) => msg.t === 'plugins' && setPlugins(msg.plugins));
    liveConn.start();
    // Once the first screen is up: the terminal and the built-in plugins' code, in idle time.
    preloadBuiltins();
    preloadScreens();
    // Notifications' health on this device (repaired quietly when it can be), and the icon badge.
    watchPush();
    showAttentionBadge();
    whenIdle(() => void pruneScreens());
    api('GET', '/api/mac').then((r) => {
      mac.value = r.mac;
      if (r.host) host.value = r.host;
    }, () => {});
  }
}

if ('serviceWorker' in navigator) {
  void registerShell();
  navigator.serviceWorker.addEventListener('message', (e) => {
    if (e.data?.t === 'open' && typeof e.data.path === 'string') navigatePath(e.data.path);
    else if (e.data?.t === 'shell') shellActivated(e.data.version);
  });
}

mount(App, { target: document.getElementById('app')! });
boot();
