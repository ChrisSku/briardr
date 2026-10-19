<!-- Connection state with a visible recovery action, shown on every screen. -->
<script lang="ts">
  import { theHost } from '../host.ts';
  import { liveConn } from '../live.ts';
  import { navigate } from '../router.ts';
  import { conn, herdr, now } from '../store.ts';
  import { Icon } from '../ui/icons/index.ts';
  import { ago } from '../ui/kit.ts';
  import { herdrProblem } from './streams.ts';

  const c = $derived(conn.value);
  const h = $derived(herdr.value);
  const kind = $derived(
    c.state === 'stopped' && c.error ? 'stopped' : c.state === 'connecting' ? 'connecting' : c.state === 'offline' ? 'offline' : h && !h.ok ? 'herdr' : 'other',
  );
  const secs = $derived(c.retryAt ? Math.max(0, Math.round((c.retryAt - now.value) / 1000)) : 0);
</script>

{#if c.state !== 'online' || (h && !h.ok)}
  {@const tone = kind === 'stopped' || kind === 'herdr' ? 'error' : 'warn'}
  <div class="connbar tone-{tone}" role="status" aria-live="polite">
    {#if tone === 'error'}<Icon name="warning" size={16} />{:else}<Icon name="offline" size={16} />{/if}
    {#if kind === 'stopped'}
      <span class="connbar-text">{c.error?.message}</span>
      {#if c.error?.category === 'auth'}
        <button class="btn btn-small" onclick={() => location.assign('/')}>Pair this device</button>
      {:else}
        <button class="btn btn-small" onclick={() => location.reload()}>Reload</button>
      {/if}
    {:else if kind === 'connecting'}
      <span class="connbar-text">Connecting to {theHost()}…</span>
    {:else if kind === 'offline'}
      <span class="connbar-text">{c.error?.message || `Can’t reach ${theHost()}.`} {c.lastOnlineAt ? `Last connected ${ago(now.value - c.lastOnlineAt)}.` : ''} {secs ? `Retrying in ${secs}s.` : 'Retrying…'}</span>
      <button class="btn btn-small" onclick={() => liveConn.retryNow()}>Retry now</button>
    {:else if kind === 'herdr'}
      <span class="connbar-text">{herdrProblem(h!.problem)}. Showing the last known state.</span>
      <button class="btn btn-small" onclick={() => navigate({ name: 'settings', section: 'herdr' })}>Herdr settings</button>
    {:else}
      <span class="connbar-text"></span>
    {/if}
  </div>
{/if}
