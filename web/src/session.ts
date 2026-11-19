// Authenticated startup, shared by an existing session and pairing in the open app.
import { api } from './api.ts';
import { showAttentionBadge } from './badge.svelte.ts';
import { pruneScreens } from './idb.ts';
import { whenIdle } from './idle.ts';
import { liveConn } from './live.ts';
import { preloadBuiltins, setPlugins } from './plugins/registry.svelte.ts';
import type { MacState, Session } from './protocol.ts';
import { watchPush } from './push.ts';
import { preloadScreens } from './screens/lazy.svelte.ts';
import { host, mac, session } from './store.ts';
import { rememberServer } from './updates.ts';

export function acceptSession(info: Session) {
  rememberServer(info.instanceId);
  host.value = info.host;
  setPlugins(info.plugins);
  session.value = { status: 'signed_in', device: info.device };
}

let started = false;
export function startSession() {
  if (!started) {
    started = true;
    liveConn.on((msg) => msg.t === 'plugins' && setPlugins(msg.plugins));
    preloadBuiltins();
    preloadScreens();
    watchPush();
    showAttentionBadge();
    whenIdle(() => void pruneScreens());
  }
  liveConn.start();
  api<MacState>('GET', '/api/mac').then((r) => {
    mac.value = r.mac;
    if (r.host) host.value = r.host;
  }, () => {});
}
