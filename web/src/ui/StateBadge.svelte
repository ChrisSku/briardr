<script lang="ts">
  import { STATE_LABEL, type AgentState } from '../types.ts';

  let { state, seen = false }: { state: AgentState | null; seen?: boolean } = $props();
</script>

{#if state}
  <span class="badge st-{state} {seen ? 'is-seen' : ''}">
    <span class="dot" aria-hidden="true"></span>
    {STATE_LABEL[state]}
  </span>
{/if}
