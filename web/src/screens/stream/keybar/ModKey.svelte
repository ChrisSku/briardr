<!-- A modifier: tap = next key only, double tap = lock, hold = chord while held. -->
<script lang="ts">
  import { mods, type Mod, type ModMode } from '../../../terminal/keys.ts';
  import { capture } from './capture.ts';

  let { m, label, name }: { m: Mod; label: string; name: string } = $props();
  let lastTap = 0;
  let holdTimer: number | null = null;
  const state = $derived(mods.value[m]);
  const set = (v: ModMode) => (mods.value = { ...mods.value, [m]: v });
</script>

<button
  class="key mod mod-{state}"
  aria-pressed={state !== 'off'}
  aria-label="{name}{state === 'locked' ? ', locked' : state === 'once' ? ', next key' : state === 'held' ? ', held' : ''}"
  onpointerdown={(e) => {
    capture(e);
    holdTimer = window.setTimeout(() => {
      holdTimer = null;
      set('held');
    }, 280);
  }}
  onpointerup={() => {
    if (holdTimer === null && mods.value[m] === 'held') {
      set('off'); // end of a chord
      return;
    }
    if (holdTimer) clearTimeout(holdTimer);
    holdTimer = null;
    const t = Date.now();
    const cur = mods.value[m];
    if (cur === 'once' && t - lastTap < 320) set('locked');
    else if (cur === 'off') set('once');
    else set('off');
    lastTap = t;
  }}
  onpointercancel={() => {
    if (holdTimer) clearTimeout(holdTimer);
    holdTimer = null;
    if (mods.value[m] === 'held') set('off');
  }}
  onkeydown={(e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      set(state === 'off' ? 'once' : state === 'once' ? 'locked' : 'off');
    }
  }}
>
  {label}
</button>
