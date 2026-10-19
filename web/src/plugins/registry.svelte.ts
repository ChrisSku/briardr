// Which plugins are on (as the server says, cached for offline launches) and what they add: stream
// tabs, app tabs, settings sections and terminal link handlers, in plugin order (built-ins first).
import { whenIdle } from '../idle.ts';
import { derived, persisted, state } from '../reactive.svelte.ts';
import { BUILTIN_LOADERS, BUILTIN_MANIFESTS } from './builtin.ts';
import type { Contributes, PluginInfo, PluginModule, SectionInfo, TabInfo } from './types.ts';

/** Until the server has answered: the built-ins, all on. */
const DEFAULT: PluginInfo[] = BUILTIN_MANIFESTS.map((m) => ({
  id: m.id,
  name: m.name,
  description: m.description,
  version: '',
  builtin: true,
  enabled: true,
  contributes: m.contributes,
  backend: null,
  problem: null,
}));

export const plugins = persisted<PluginInfo[]>('briardr.cache.plugins', DEFAULT);

/** Client code of the plugins loaded so far: built-ins when needed, installed ones when they're on. */
const modules = state<Record<string, PluginModule>>({});

export function registerModule(id: string, m: PluginModule) {
  modules.value = { ...modules.value, [id]: m };
}

const builtinLoads = new Map<string, Promise<void>>();

/** Loads a built-in plugin's code (once; a failed load is tried again next time). */
export function loadBuiltin(id: string): Promise<void> {
  const load = BUILTIN_LOADERS[id];
  if (modules.value[id] || !load) return Promise.resolve();
  let p = builtinLoads.get(id);
  if (!p) {
    p = load().then(
      (m) => registerModule(id, m.default),
      (e) => {
        builtinLoads.delete(id);
        loadErrors.value = { ...loadErrors.value, [id]: e instanceof Error ? e.message : String(e) };
      },
    );
    builtinLoads.set(id, p);
  }
  return p;
}

/** Loads the built-ins that are on and add `kind` (all of them without one). */
export function loadBuiltins(kind?: keyof Contributes): Promise<void> {
  const ids = plugins.value.filter((p) => p.builtin && p.enabled && (!kind || p.contributes?.[kind]?.length)).map((p) => p.id);
  return Promise.all(ids.map(loadBuiltin)).then(() => {});
}

/** The client URL each installed plugin was loaded from, and why one couldn't load. */
const loaded = new Map<string, string>();
export const loadErrors = state<Record<string, string>>({});

/** Loads the client of each installed plugin that's on, again when it was rebuilt (a new URL). */
async function loadClients(list: PluginInfo[]) {
  for (const p of list) {
    if (p.builtin || !p.enabled || !p.client || loaded.get(p.id) === p.client) continue;
    const url = p.client;
    loaded.set(p.id, url);
    try {
      const mod = await import(/* @vite-ignore */ url);
      if (loaded.get(p.id) !== url) continue; // a newer build arrived meanwhile
      registerModule(p.id, (mod.default || {}) as PluginModule);
      const { [p.id]: _, ...rest } = loadErrors.value;
      loadErrors.value = rest;
    } catch (e) {
      loaded.delete(p.id);
      loadErrors.value = { ...loadErrors.value, [p.id]: e instanceof Error ? e.message : String(e) };
    }
  }
}

/** A tab or section a plugin adds; `component` is null while a built-in's code is still loading. */
export type Contribution<C> = TabInfo & { plugin: string; component: C | null };

function collect<C>(list: (c: Partial<Contributes>) => (TabInfo | SectionInfo)[] | undefined, component: (m: PluginModule, id: string) => C | undefined) {
  return derived(() =>
    plugins.value.flatMap((p) => {
      const m = modules.value[p.id];
      // Installed plugins show once their code has loaded; built-ins right away, from their manifests.
      if (!p.enabled || !p.contributes || (!m && !p.builtin)) return [];
      return (list(p.contributes) || []).flatMap((t): Contribution<C>[] => {
        const c = m ? component(m, t.id) : null;
        return c || !m ? [{ ...t, plugin: p.id, component: c ?? null }] : [];
      });
    }),
  );
}

export const streamTabs = collect((c) => c.streamTabs, (m, id) => m.streamTabs?.[id]);
export const appTabs = collect((c) => c.appTabs, (m, id) => m.appTabs?.[id]);
export const settingsSections = collect((c) => c.settings, (m, id) => m.settings?.[id]);

/** Enabled plugins that want to handle terminal links, in order. */
export const linkHandlers = derived(() =>
  plugins.value.flatMap((p) => {
    const fn = p.enabled ? modules.value[p.id]?.onTerminalLink : undefined;
    return fn ? [{ plugin: p.id, fn }] : [];
  }),
);

export function isOn(id: string): boolean {
  return plugins.value.some((p) => p.id === id && p.enabled);
}

export function setPlugins(list: PluginInfo[] | undefined) {
  if (!Array.isArray(list)) return;
  plugins.value = list;
  void loadClients(list);
}

// The cached list, so installed plugins show while offline (their files are in the service worker's cache).
void loadClients(plugins.value);

/** After launch, once the browser is idle: the built-ins' code, so opening a stream doesn't wait for it. */
export function preloadBuiltins() {
  whenIdle(() => void loadBuiltins());
}
