<script lang="ts">
  import type { Snippet } from 'svelte';

  let { title, children }: { title: string; children?: Snippet } = $props();
</script>

<div class="empty" role="status">
  <p class="empty-title">{title}</p>
  {#if children}
    <div class="empty-body">{@render children()}</div>
  {/if}
</div>
