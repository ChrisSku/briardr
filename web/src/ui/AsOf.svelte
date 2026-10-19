<!-- "as of" label for server-time stamps. -->
<script lang="ts">
  import { clockSkew, now } from '../store.ts';
  import { ago } from './time.ts';

  let { at, prefix = 'as of', stale = false }: { at: number | null | undefined; prefix?: string; stale?: boolean } = $props();
</script>

{#if !at}
  <span class="asof">{stale ? 'no data yet' : ''}</span>
{:else}
  <time class="asof {stale ? 'is-stale' : ''}" datetime={new Date(at).toISOString()} title={new Date(at).toLocaleString()}>
    {stale ? 'Stale · ' : ''}{prefix} {ago(now.value + clockSkew.value - at)}
  </time>
{/if}
