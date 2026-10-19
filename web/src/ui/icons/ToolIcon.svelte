<!-- Small mark for the program running in a stream. -->
<script lang="ts">
  import Icon from './Icon.svelte';
  import type { IconName } from './shapes.ts';

  let { tool, kind, size = 22 }: { tool: string; kind: string; size?: number } = $props();
  const mark = $derived.by((): [string, IconName] => {
    const t = tool.toLowerCase();
    if (t === 'claude' || t === 'codex') return [t, t];
    if (kind === 'editor' || kind === 'program') return [kind, kind];
    return kind === 'agent' ? ['agent', 'agents'] : ['shell', 'terminal'];
  });
</script>

<span class="tool tool-{mark[0]}" aria-hidden="true">
  <Icon name={mark[1]} {size} />
</span>
