<!-- Agents: what needs you, what's working, what finished. Needs you shows every row; the other groups
     show their first few until "Show all". Beside an open stream (compact), plain rows without cards. -->
<script module lang="ts">
  import { GROUP_LABEL, groupOf, type Group, type Stream } from '../types.ts';
  import { isSeen } from './streams.ts';

  const ORDER: Group[] = ['attention', 'working', 'finished', 'ready', 'unknown'];
  /** Rows a group shows before "Show all". */
  const PREVIEW = 3;

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
  import { agentFilter, expanded, model, stale } from '../store.ts';
  import { Empty } from '../ui/kit.ts';
  import { peekSheet } from './lazy.svelte.ts';
  import StreamRow from './StreamRow.svelte';

  let { compact = false }: { compact?: boolean } = $props();
  let peek = $state.raw<Stream | null>(null);
  const groups = $derived(groupStreams(model.value?.streams || []));
  const filter = $derived(agentFilter.value);
  const total = $derived(ORDER.reduce((n, k) => n + groups[k].length, 0));
  const shown = $derived(ORDER.filter((k) => (!filter || filter === k) && groups[k].length));
  // Needs you, a filtered group and an expanded one show every row.
  const showsAll = (k: Group) => k === 'attention' || filter === k || !!expanded.value[k];
  const onPeek = (s: Stream) => {
    void peekSheet.load();
    peek = s;
  };
</script>

<div class={['agents', compact && 'is-list']}>
  <div class="count-strip" role="group" aria-label="Filter by state">
    {#each ORDER as k}
      <button class={['count', `st-${k}`, filter === k && 'is-on']} aria-pressed={filter === k} onclick={() => (agentFilter.value = filter === k ? null : k)} disabled={!groups[k].length && filter !== k}>
        <span class="count-n">{groups[k].length}</span>
        <span class="count-l">{GROUP_LABEL[k]}</span>
      </button>
    {/each}
  </div>
  {#if !model.value}
    <Empty title="Waiting for {theHost()}">Briardr shows your agents as soon as it reaches {theHost()}.</Empty>
  {:else if total === 0}
    <Empty title="No agents running">Shells and editors are in Workspaces. Start Claude or Codex in a project from there.</Empty>
  {/if}
  {#each shown as k}
    {@const all = showsAll(k)}
    {@const rows = all ? groups[k] : groups[k].slice(0, PREVIEW)}
    <section class="group grp-{k}" aria-labelledby="grp-{k}">
      <h2 class="section-head">
        <span id="grp-{k}" class={['overline', k === 'attention' && 'st-blocked']}>{GROUP_LABEL[k]}</span>
        <span class="section-n">{groups[k].length}</span>
        {#if k !== 'attention' && filter !== k && groups[k].length > PREVIEW}
          <button class="section-action" aria-expanded={all} aria-label="{all ? 'Show fewer' : 'Show all'} {GROUP_LABEL[k]}" onclick={() => (expanded.value = { ...expanded.value, [k]: !all })}>
            {all ? 'Show less' : 'Show all'}
          </button>
        {/if}
      </h2>
      <ul class={['rows', !compact && 'card', !compact && k === 'attention' && 'tone-attention']}>
        {#each rows as s (s.id)}
          <StreamRow {s} {onPeek} dim={stale.value} badge={false} />
        {/each}
      </ul>
    </section>
  {/each}
  {#if peek && peekSheet.component.value}{@const PeekSheet = peekSheet.component.value}<PeekSheet stream={peek} onClose={() => (peek = null)} />{/if}
</div>
