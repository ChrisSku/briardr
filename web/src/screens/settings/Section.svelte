<!-- A settings section: the body of its page (the page has the title). -->
<script lang="ts">
  import type { Snippet } from 'svelte';

  let { id, title, children }: { id: string; title: string; children: Snippet } = $props();
</script>

<section class="set-section" id="set-{id}" aria-label={title}>
  <div class="set-body">{@render children()}</div>
</section>
