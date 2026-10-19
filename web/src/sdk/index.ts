// @briardr/sdk: what every plugin imports, built-in ones (plugins/<id>/ in this repository) included.
// Briardr serves it at /sdk/v1/briardr.js, and plugins built with `briardr plugin build` share the app's
// instance of it (and of Svelte), so they look and behave like the built-in plugins. Built-in plugins
// get nothing else: when one needs something new, it goes here, for every plugin. Keep this list
// deliberate: everything here is an API plugins rely on.
export { definePlugin } from '../plugins/types.ts';
export type {
  AppTabProps,
  Contributes,
  Navigation,
  PluginContext,
  PluginModule,
  SectionInfo,
  SettingsProps,
  StreamContext,
  StreamTabProps,
  TabInfo,
  TerminalAccess,
} from '../plugins/types.ts';
export type { AgentState, HostCaps, Stream, StreamKind } from '../types.ts';
export type { Save, Settings } from '../screens/settings/settings.ts';
export { ApiError, type FailureCategory } from '../api.ts';
export { derived, persisted, state, type Derived, type Persisted, type State } from '../reactive.svelte.ts';
export { prepareImage, MAX_IMAGE_BYTES, MAX_IMAGE_SIDE, type PreparedImage } from '../media.ts';
export { keyboardInset } from '../screens/stream/keyboard.svelte.ts';
export { isLocalhostUrl } from '../terminal/links.ts';
export { AsOf, Empty, Sheet, StateBadge, ago, duration, longPress, type MenuItem } from '../ui/kit.ts';
export { Icon, iconName, type IconName } from '../ui/icons/index.ts';
export { PullToRefresh } from '../ui/pull.svelte.ts';

/** The plugin API version (`"briardr"` in briardr-plugin.json). */
export const API_VERSION = 1;
