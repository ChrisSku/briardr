// Screens that bring the terminal (xterm.js, half of the app's code): loaded when first needed, right
// away when Briardr opens on a stream, and in idle time after launch, so the Agents list doesn't wait for
// them and opening a stream usually doesn't either. The service worker keeps them for offline use.
import type { Component } from 'svelte';
import { whenIdle } from '../idle.ts';
import { state } from '../reactive.svelte.ts';
import { codeMissing } from '../updates.ts';

function lazy<P extends Record<string, any>>(load: () => Promise<{ default: Component<P> }>) {
  const component = state<Component<P> | null>(null);
  const failed = state(false);
  let loading: Promise<void> | null = null;
  return {
    component,
    failed,
    load(): Promise<void> {
      failed.value = false;
      return (loading ??= load().then(
        (m) => void (component.value = m.default),
        () => {
          loading = null;
          failed.value = true;
          codeMissing();
        },
      ));
    },
  };
}

export const streamScreen = lazy(() => import('./stream/StreamScreen.svelte'));
export const peekSheet = lazy(() => import('./PeekSheet.svelte'));

/** After launch, once the browser is idle. */
export function preloadScreens() {
  whenIdle(() => {
    void streamScreen.load();
    void peekSheet.load();
  });
}
