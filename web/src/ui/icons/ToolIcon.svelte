<!-- The avatar for the program running in a stream: a neutral tile with its glyph (agents aren't
     colour-coded) and, given a state, a dot on the corner in that state's colour. -->
<script lang="ts">
  import type { AgentState } from '../../types.ts';
  import Icon from './Icon.svelte';
  import type { IconName } from './shapes.ts';

  let { tool, kind, size, state }: { tool: string; kind: string; size?: number; state?: AgentState | null } = $props();
  const mark = $derived.by((): [string, IconName] => {
    const t = tool.toLowerCase();
    if (t === 'claude' || t === 'codex') return [t, t];
    if (kind === 'editor' || kind === 'program') return [kind, kind];
    return kind === 'agent' ? ['agent', 'agents'] : ['shell', 'terminal'];
  });
</script>

<span class="tool tool-{mark[0]}" aria-hidden="true">
  <Icon name={mark[1]} size={size ?? (mark[0] === 'claude' ? 16 : 18)} />
  {#if state}<span class="tool-dot st-{state}"></span>{/if}
</span>
