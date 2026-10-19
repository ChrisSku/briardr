<script lang="ts">
  import { untrack } from 'svelte';
  import { api, ApiError } from '../../api.ts';
  import type { Stream } from '../../types.ts';
  import { AsOf, Sheet } from '../../ui/kit.ts';

  let { stream, onClose }: { stream: Stream; onClose: () => void } = $props();
  let lines = $state(1000);
  let data = $state.raw<{ text: string; truncated: boolean; asOf: number } | null>(null);
  let err = $state<string | null>(null);
  let q = $state('');
  let hit = $state(0);
  let pre: HTMLPreElement | undefined = $state();

  // Load (more) history whenever the number of lines asked for changes.
  $effect(() => {
    const n = lines;
    const sid = untrack(() => stream.id);
    err = null;
    api('GET', `/api/streams/${encodeURIComponent(sid)}/history?lines=${n}`, undefined, { timeout: 20_000 }).then(
      (r) => (data = r),
      (e) => (err = e instanceof ApiError ? e.message : 'Couldn’t load history.'),
    );
  });

  const parts = $derived.by(() => {
    if (!data) return [];
    if (!q) return [data.text];
    const out: (string | { m: string; i: number })[] = [];
    const lower = data.text.toLowerCase();
    const ql = q.toLowerCase();
    let i = 0;
    let n = 0;
    for (;;) {
      const j = lower.indexOf(ql, i);
      if (j < 0) break;
      out.push(data.text.slice(i, j), { m: data.text.slice(j, j + q.length), i: n++ });
      i = j + q.length;
    }
    out.push(data.text.slice(i));
    return out;
  });
  const count = $derived(parts.filter((p) => typeof p !== 'string').length);

  // Bring the current match into view.
  $effect(() => {
    void [hit, q, data];
    pre?.querySelector(`mark[data-i="${hit}"]`)?.scrollIntoView({ block: 'center' });
  });
  // New history without a search: show the most recent lines.
  $effect(() => {
    if (data && pre && !untrack(() => q)) pre.scrollTop = pre.scrollHeight;
  });
</script>

<Sheet title="History" {onClose} wide class="history">
  <div class="history-bar">
    <input
      class="field"
      type="search"
      placeholder="Search history"
      aria-label="Search history"
      bind:value={q}
      oninput={() => (hit = 0)}
      onkeydown={(e) => e.key === 'Enter' && count && (hit = (hit + (e.shiftKey ? count - 1 : 1)) % count)}
    />
    <span class="muted" aria-live="polite">{q ? `${count ? hit + 1 : 0} of ${count}` : ''}</span>
    <button class="btn btn-small" disabled={!count} onclick={() => (hit = (hit + count - 1) % count)} aria-label="Previous match">↑</button>
    <button class="btn btn-small" disabled={!count} onclick={() => (hit = (hit + 1) % count)} aria-label="Next match">↓</button>
  </div>
  {#if err}<p class="error" role="alert">{err}</p>{/if}
  {#if data}
    <button class="btn btn-block" onclick={() => (lines += 2000)} disabled={!data.truncated && data.text.split('\n').length < lines - 5}>Load more</button>
    <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
    <pre class="history-text mono" bind:this={pre} tabindex={0}>{#each parts as p}{#if typeof p === 'string'}{p}{:else}<mark data-i={p.i} class={p.i === hit ? 'cur' : ''}>{p.m}</mark>{/if}{/each}</pre>
    <p class="hint">
      Last {lines.toLocaleString()} lines Herdr can provide · <AsOf at={data.asOf} />
    </p>
  {/if}
</Sheet>
