// The `briardr` object plugin components get: requests, toasts, navigation, backend events and, in a
// stream, the terminal (insert only, through the same controller as every other insert).
import { api, upload } from '../api.ts';
import { record } from '../diag.ts';
import { caps, theHost } from '../host.ts';
import { liveConn } from '../live.ts';
import { load, save } from '../reactive.svelte.ts';
import { navigate } from '../router.ts';
import { clockSkew, conn, now, prefs, toast } from '../store.ts';
import { MAX_PASTE_BYTES } from '../screens/stream/insert.ts';
import type { TerminalController } from '../terminal/controller.ts';
import { confirmDialog, openMenu } from '../ui/overlays.ts';
import type { Navigation, PluginContext, StreamContext } from './types.ts';

const contexts = new Map<string, PluginContext>();
const enc = new TextEncoder();

function go(to: Navigation) {
  if ('stream' in to) navigate({ name: 'stream', id: to.stream, surface: to.tab || 'terminal' });
  else if ('app' in to) navigate({ name: 'app', tab: to.app });
  else if ('settings' in to) navigate({ name: 'settings', section: to.settings });
  else navigate({ name: 'agents' });
}

export function pluginContext(id: string): PluginContext {
  let c = contexts.get(id);
  if (c) return c;
  const base = `/api/plugins/${encodeURIComponent(id)}`;
  const prefix = `briardr.plugin.${id}.`;
  c = {
    plugin: { id },
    api: (method, path, body, opts) => api(method, base + (path.startsWith('/') ? path : `/${path}`), body, opts),
    core: (method, path, body, opts) => api(method, path, body, opts),
    upload: (path, file, opts = {}) => upload(path, file, opts.onProgress || (() => {}), opts.signal || new AbortController().signal),
    toast,
    confirm: (o) => confirmDialog(o.title, o.message || '', o.confirm, o.destructive),
    menu: (items, anchor, title) => openMenu(items, anchor, title),
    navigate: go,
    onEvent: (topic, fn) =>
      liveConn.on((msg) => {
        if (msg.t === 'plugin_event' && msg.plugin === id && msg.topic === topic) fn(msg.data);
      }),
    onCoreEvent: (type, fn) =>
      liveConn.on((msg) => {
        if (msg.t === type && type !== 'plugin_event') fn(msg);
      }),
    connection: () => conn.value.state,
    now: () => now.value + clockSkew.value,
    clockSkew: () => clockSkew.value,
    fontSize: () => prefs.value.fontSize,
    record: (event, fields = {}) => record(event, { ...fields, plugin: id }),
    prefs: { get: (key, fallback) => load(prefix + key, fallback), set: (key, value) => save(prefix + key, value) },
    host: () => ({ ...caps(), the: theHost() }),
  };
  contexts.set(id, c);
  return c;
}

/** The context for a plugin's tab in one stream; `ctl` is that stream's terminal. */
export function streamContext(id: string, streamId: string, ctl: TerminalController): StreamContext {
  const base = pluginContext(id);
  const k: StreamContext = Object.freeze({
    ...base,
    terminal: {
      maxBytes: MAX_PASTE_BYTES,
      get canSend() {
        return ctl.status.value.ownership === 'controlling';
      },
      get generation() {
        return ctl.generation.value;
      },
      send(text: string, opts: { generation?: number } = {}) {
        if (!text || enc.encode(text).length > MAX_PASTE_BYTES) return false;
        return ctl.paste(text, prefs.value.bracketedPaste, opts.generation ?? ctl.generation.value);
      },
    },
    openTab: (tab: string) => navigate({ name: 'stream', id: streamId, surface: tab }, { replace: true }),
  });
  return k;
}
