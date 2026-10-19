// The app icon's badge: how many agents need you (the Agents list's "Needs you"), where the platform
// has app badges. Set only from live state: while the host is unreachable or Herdr isn't answering, the
// last count stays. Alerts update it while Briardr is closed (sw.ts, from the count the host sends).
import { model, stale } from './store.ts';
import { groupOf } from './types.ts';

let started = false;
export function showAttentionBadge() {
  if (started || !('setAppBadge' in navigator)) return;
  started = true;
  $effect.root(() => {
    $effect(() => {
      if (stale.value) return;
      const ids = new Set((model.value?.streams ?? []).filter((s) => groupOf(s) === 'attention').map((s) => s.id));
      (ids.size ? navigator.setAppBadge(ids.size) : navigator.clearAppBadge()).catch(() => {});
    });
  });
}
