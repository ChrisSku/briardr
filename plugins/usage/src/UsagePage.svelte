<!--
  Usage (Settings → Usage): Claude and Codex subscription limits. Refreshes every 3 minutes while this
  page is visible; the Mac combines overlapping requests and backs off after failures. Reports keep their
  original capture time; a window whose reset time has passed shows "Reset since last report". Codex
  limit reset credits are listed, and one can be used after an explicit confirmation. Sources opens
  where the reports come from.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { ago, ApiError, Icon, Sheet, type SettingsProps } from '@briardr/sdk';
  import Provider from './Provider.svelte';
  import Sources from './Sources.svelte';
  import { cached, REFRESH_MS, type Usage } from './usage.ts';
  import UseResetSheet from './UseResetSheet.svelte';

  let { briardr, s, save }: SettingsProps = $props();
  let busy = $state(false);
  let error = $state<string | null>(null);
  let resetting = $state<number | null>(null); // available resets, while confirming
  let sources = $state(false);

  const t = $derived(briardr.now());
  const reports = $derived([cached.value?.claude, cached.value?.codex].flatMap((x) => [x?.report, x?.tapReport]).filter((r) => !!r));
  const newest = $derived(reports.length ? Math.max(...reports.map((r) => r.capturedAt)) : null);
  const on = $derived([(s?.usage.claudeKeychainOptIn || cached.value?.claude.tapInstalled) && 'Claude', s?.usage.codexEnabled && 'Codex'].filter(Boolean).join(', ') || 'None');

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

<div class="usage">
  <div class="usage-top">
    <span>Plan limits{newest ? ` · updated ${ago(t - newest)}` : ''}</span>
    <button class="icon-btn round" aria-label="Refresh usage now" onclick={() => load(true)} disabled={busy}>
      <Icon name="refresh" size={18} class={busy ? 'spin' : ''} />
    </button>
  </div>
  {#if error}<p class="note" role="status">{error} Showing the last reports.</p>{/if}
  <Provider {briardr} name="Claude" s={cached.value?.claude} />
  <Provider {briardr} name="Codex" s={cached.value?.codex} onReset={(n) => (resetting = n)} />
  <div class="card">
    <button class="set-row" onclick={() => (sources = true)}>
      <span class="set-tile"><Icon name="plug" size={17} /></span>
      <span class="set-label">Sources</span>
      <span class="set-value">{on}</span>
      <Icon name="chevron" size={15} class="set-chev" />
    </button>
  </div>
  <p class="hint">Reading usage never starts an agent turn.</p>
</div>
{#if sources}
  <Sheet title="Usage sources" onClose={() => (sources = false)}>
    <Sources {briardr} {s} {save} />
  </Sheet>
{/if}
{#if resetting !== null}
  <UseResetSheet {briardr} name="Codex" available={resetting} onClose={() => (resetting = null)} onUsed={(usage) => (cached.value = usage)} />
{/if}
