import './install.ts'; // first: the browser's install prompt fires early
import '@xterm/xterm/css/xterm.css';
import './styles.css';
import { mount } from 'svelte';
import { api, ApiError } from './api.ts';
import App from './App.svelte';
import { record } from './diag.ts';
import { trackKeyboard } from './keyboard.svelte.ts';
import { registerShell } from './offline.ts';
import { streamScreen } from './screens/lazy.svelte.ts';
import { navigatePath, route } from './router.ts';
import { acceptSession, startSession } from './session.ts';
import { host, session, wasSignedIn } from './store.ts';
import { shellActivated } from './updates.ts';

// Opening on a stream (a relaunch restores the last screen): fetch the terminal alongside the session.
if (route.value.name === 'stream') void streamScreen.load();

async function boot() {
  record('app_start', { standalone: matchMedia('(display-mode: standalone)').matches });
  // A device that was signed in opens on its cached state (labeled stale) without waiting for the host,
  // which may be slow or unreachable; the check below still signs it out if it was revoked meanwhile.
  if (wasSignedIn) session.value = { status: 'signed_in', device: { id: '', name: '' } };
  try {
    const r = await api('GET', '/api/session', undefined, { timeout: 8000 });
    acceptSession(r);
  } catch (e) {
    if (e instanceof ApiError && e.data?.host) host.value = { ...host.value, ...e.data.host };
    if (e instanceof ApiError && e.category === 'auth') session.value = { status: 'signed_out' };
    // Offline at launch: keep showing cached state, labeled stale; the live connection keeps retrying.
    else session.value = { status: 'signed_in', device: { id: '', name: '' } };
  }
  if (session.value.status === 'signed_in') startSession();
}

if ('serviceWorker' in navigator) {
  void registerShell();
  navigator.serviceWorker.addEventListener('message', (e) => {
    if (e.data?.t === 'open' && typeof e.data.path === 'string') navigatePath(e.data.path);
    else if (e.data?.t === 'shell') shellActivated(e.data.version);
  });
}

// Every screen keeps the focused field, and the sheet it's in, above the on-screen keyboard.
trackKeyboard();
mount(App, { target: document.getElementById('app')! });
boot();
