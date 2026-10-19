<!-- Agents: what needs you, what's working, what finished. Ready and Unknown start collapsed. -->
<script module lang="ts">
  import { GROUP_LABEL, groupOf, type Group, type Stream } from '../types.ts';
  import { isSeen } from './streams.ts';

  const ORDER: Group[] = ['attention', 'working', 'finished', 'ready', 'unknown'];
  const SHORT: Record<Group, string> = { attention: 'Needs you', working: 'Working', finished: 'Finished', ready: 'Ready', unknown: 'Unknown' };

  export function groupStreams(streams: Stream[]): Record<Group, Stream[]> {
    const g: Record<Group, Stream[]> = { attention: [], working: [], finished: [], ready: [], unknown: [] };
    for (const s of streams) {
      const k = groupOf(s);
      if (k) g[k].push(s);
    }
    // Unseen finished work first, then most recent state change.
    for (const k of ORDER)
      g[k].sort((a, b) => {
        if (k === 'finished') {
          const sa = isSeen(a) ? 1 : 0;
          const sb = isSeen(b) ? 1 : 0;
          if (sa !== sb) return sa - sb;
        }
        return (b.stateSince || 0) - (a.stateSince || 0);
      });
    return g;
  }
</script>

<script lang="ts">
  import { theHost } from '../host.ts';
  import { agentFilter, collapsed, model, stale } from '../store.ts';
  import { Icon } from '../ui/icons/index.ts';
  import { Empty } from '../ui/kit.ts';
  import MacSummary from './MacSummary.svelte';
  import { peekSheet } from './lazy.svelte.ts';
  import StreamRow from './StreamRow.svelte';

  let { compact = false }: { compact?: boolean } = $props();
  let peek = $state.raw<Stream | null>(null);
  const groups = $derived(groupStreams(model.value?.streams || []));
  const filter = $derived(agentFilter.value);
  const total = $derived(ORDER.reduce((n, k) => n + groups[k].length, 0));
  const shown = $derived(ORDER.filter((k) => (!filter || filter === k) && groups[k].length));
  const onPeek = (s: Stream) => {
    void peekSheet.load();
    peek = s;
  };
</script>

<div class="agents">
  {#if !compact}<MacSummary />{/if}
  <div class="count-strip" role="group" aria-label="Filter by state">
    {#each ORDER as k}
      <button class={['count', `st-${k}`, filter === k && 'is-on']} aria-pressed={filter === k} onclick={() => (agentFilter.value = filter === k ? null : k)} disabled={!groups[k].length && filter !== k}>
        <span class="count-n">{groups[k].length}</span>
        <span class="count-l">{SHORT[k]}</span>
      </button>
    {/each}
  </div>
  {#if !model.value}
    <Empty title="Waiting for {theHost()}">Briardr shows your agents as soon as it reaches {theHost()}.</Empty>
  {:else if total === 0}
    <Empty title="No agents running">Shells and editors are in Workspaces. Start Claude or Codex in a project from there.</Empty>
  {/if}
  {#each shown as k}
    {@const isCollapsed = !filter && collapsed.value[k]}
    <section class="group grp-{k}" aria-labelledby="grp-{k}">
      <h2 class="group-head">
        <button id="grp-{k}" class="group-toggle" aria-expanded={!isCollapsed} onclick={() => (collapsed.value = { ...collapsed.value, [k]: !collapsed.value[k] })}>
          <Icon name="chevron" class={['chev', !isCollapsed && 'open']} size={16} />
          <span>{GROUP_LABEL[k]}</span>
          <span class="group-n">{groups[k].length}</span>
        </button>
      </h2>
      {#if !isCollapsed}
        <ul class="rows">
          {#each groups[k] as s (s.id)}
            <StreamRow {s} {onPeek} dim={stale.value} />
          {/each}
        </ul>
      {/if}
    </section>
  {/each}
  {#if peek && peekSheet.component.value}{@const PeekSheet = peekSheet.component.value}<PeekSheet stream={peek} onClose={() => (peek = null)} />{/if}
</div>
