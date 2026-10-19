<script lang="ts">
  import type { Snippet } from 'svelte';

  let { id, title, children }: { id: string; title: string; children: Snippet } = $props();
</script>

<section class="set-section" id="set-{id}" aria-labelledby="set-{id}-h">
  <h2 id="set-{id}-h">{title}</h2>
  <div class="set-body">{@render children()}</div>
</section>
