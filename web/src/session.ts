// Authenticated startup, shared by an existing session and pairing in the open app.
import { api } from './api.ts';
import { showAttentionBadge } from './badge.svelte.ts';
import { pruneScreens } from './idb.ts';
import { whenIdle } from './idle.ts';
import { liveConn } from './live.ts';
import { preloadBuiltins, setPlugins } from './plugins/registry.svelte.ts';
import type { PluginInfo } from './plugins/types.ts';
import { watchPush } from './push.ts';
import { preloadScreens } from './screens/lazy.svelte.ts';
import { host, mac, session } from './store.ts';
import type { HostCaps } from './types.ts';
import { rememberServer } from './updates.ts';

export interface SessionInfo {
  device: { id: string; name: string };
  instanceId: string;
  host: HostCaps;
  plugins: PluginInfo[];
}

export function acceptSession(info: SessionInfo) {
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
  api('GET', '/api/mac').then((r) => {
    mac.value = r.mac;
    if (r.host) host.value = r.host;
  }, () => {});
}
