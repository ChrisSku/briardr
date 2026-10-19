import fs from 'node:fs';
import path from 'node:path';

// The modules Briardr shares with installed plugins (built-in ones are bundled with the app and use the same), by the name a plugin imports and the stable file
// Briardr serves under /sdk/v1/. Plugins get the app's own instances: Svelte's runtime keeps its state
// in module scope, so a second copy would break reactivity between a plugin and the app.
export const SDK_MODULES: Record<string, string> = {
  '@briardr/sdk': 'briardr',
  '@briardr/sdk/markdown': 'briardr-markdown',
  svelte: 'svelte',
  'svelte/internal/client': 'svelte-internal-client',
  'svelte/animate': 'svelte-animate',
  'svelte/attachments': 'svelte-attachments',
  'svelte/easing': 'svelte-easing',
  'svelte/events': 'svelte-events',
  'svelte/motion': 'svelte-motion',
  'svelte/reactivity': 'svelte-reactivity',
  'svelte/reactivity/window': 'svelte-reactivity-window',
  'svelte/store': 'svelte-store',
  'svelte/transition': 'svelte-transition',
};

export const SDK_PATH = '/sdk/v1';

/** The source of each @briardr/sdk module, relative to web/src/sdk. */
export const SDK_SOURCES: Record<string, string> = {
  '@briardr/sdk': 'index.ts',
  '@briardr/sdk/markdown': 'markdown.ts',
};

/** What a plugin build must match to run (scripts/plugin-build.ts): the Svelte compiler and the plugin API. */
export function runtimeKey(): string {
  const pkg = path.resolve(import.meta.dir, '..', 'node_modules', 'svelte', 'package.json');
  return `svelte@${JSON.parse(fs.readFileSync(pkg, 'utf8')).version}+sdk1`;
}
