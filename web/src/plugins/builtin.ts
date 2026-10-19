// The built-in plugins, plugins/<id>/ in this repository: made like any installed plugin (the same
// manifest, a client that uses only @briardr/sdk and Svelte), but bundled with the app and on unless
// turned off in Settings → Plugins. Their manifests are here from the start (tab labels, order); their
// code is in chunks of its own, loaded when a stream, an app tab or Settings needs it, or in idle time
// after launch (registry.svelte.ts).
import changesManifest from '../../../plugins/changes/briardr-plugin.json';
import docsManifest from '../../../plugins/docs/briardr-plugin.json';
import filesManifest from '../../../plugins/files/briardr-plugin.json';
import notesManifest from '../../../plugins/notes/briardr-plugin.json';
import previewManifest from '../../../plugins/preview/briardr-plugin.json';
import type { Contributes, PluginModule } from './types.ts';
import usageManifest from '../../../plugins/usage/briardr-plugin.json';

export interface BuiltinManifest {
  id: string;
  name: string;
  description: string;
  contributes: Partial<Contributes>;
}

/** In tab order, the same as the server's list (server/src/plugins/mod.rs). */
export const BUILTIN_MANIFESTS: BuiltinManifest[] = [filesManifest, changesManifest, previewManifest, docsManifest, notesManifest, usageManifest];

export const BUILTIN_LOADERS: Record<string, () => Promise<{ default: PluginModule }>> = {
  files: () => import('../../../plugins/files/src/index.ts'),
  changes: () => import('../../../plugins/changes/src/index.ts'),
  preview: () => import('../../../plugins/preview/src/index.ts'),
  docs: () => import('../../../plugins/docs/src/index.ts'),
  notes: () => import('../../../plugins/notes/src/index.ts'),
  usage: () => import('../../../plugins/usage/src/index.ts'),
};
