<!--
  Usage: Claude and Codex subscription limits. Refreshes every 3 minutes while this screen is
  visible; the Mac combines overlapping requests and backs off after failures. Reports keep their
  original capture time; a window whose reset time has passed shows "Reset since last report".
  Codex limit reset credits are listed, and one can be used after an explicit confirmation.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError, Icon, type AppTabProps } from '@briardr/sdk';
  import Provider from './Provider.svelte';
  import { cached, REFRESH_MS, type Usage } from './usage.ts';
  import UseResetSheet from './UseResetSheet.svelte';

  let { briardr }: AppTabProps = $props();
  let busy = $state(false);
  let error = $state<string | null>(null);
  let resetting = $state<number | null>(null); // available resets, while confirming

  async function load(force = false) {
    busy = true;
    error = null;
    try {
      const r = await briardr.core<{ usage: Usage }>('GET', `/api/usage${force ? '?force=1' : ''}`, undefined, { timeout: 45_000 });
      cached.value = r.usage;
    } catch (e) {
      error = e instanceof ApiError ? e.message : `Couldn’t reach ${briardr.host().the}.`;
    } finally {
      busy = false;
    }
  }

  onMount(() => {
    const tick = () => {
      if (document.visibilityState === 'visible') load(false);
    };
    tick();
    const timer = window.setInterval(tick, REFRESH_MS);
    const vis = () => document.visibilityState === 'visible' && tick();
    document.addEventListener('visibilitychange', vis);
    return () => {
      clearInterval(timer);
      document.removeEventListener('visibilitychange', vis);
    };
  });
</script>

<div class="screen">
  <header class="screen-head">
    <h1>Usage</h1>
    <button class="icon-btn" aria-label="Refresh usage now" onclick={() => load(true)} disabled={busy}>
      <Icon name="refresh" class={busy ? 'spin' : ''} />
    </button>
  </header>
  <div class="screen-body usage">
    {#if error}<p class="note" role="status">{error} Showing the last reports.</p>{/if}
    <Provider {briardr} name="Claude" s={cached.value?.claude} />
    <Provider {briardr} name="Codex" s={cached.value?.codex} onReset={(n) => (resetting = n)} />
    <p class="hint">
      Reading usage never starts an agent turn. Set up sources in <a href="/settings/usage" onclick={(e) => (e.preventDefault(), briardr.navigate({ settings: 'usage' }))}>Settings</a>.
    </p>
  </div>
  {#if resetting !== null}
    <UseResetSheet {briardr} name="Codex" available={resetting} onClose={() => (resetting = null)} onUsed={(usage) => (cached.value = usage)} />
  {/if}
</div>
