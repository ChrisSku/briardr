<!-- A state with its label: Dot beside text in rows and headers, Filled (a tinted pill) when it stands alone. -->
<script lang="ts">
  import { STATE_LABEL, type AgentState } from '../types.ts';

  let { state, seen = false, filled = false }: { state: AgentState | null; seen?: boolean; filled?: boolean } = $props();
</script>

{#if state}
  <span class={['badge', `st-${state}`, filled && 'is-filled', seen && 'is-seen']}>
    <span class="dot" aria-hidden="true"></span>
    {STATE_LABEL[state]}
  </span>
{/if}
