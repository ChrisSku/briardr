<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, type PluginContext, type Stream } from '@briardr/sdk';
  import type { ChangedFile } from './changes.ts';

  let { stream, briardr, file }: { stream: Stream; briardr: PluginContext; file: ChangedFile } = $props();
  let d = $state.raw<{ text: string; truncated: boolean; lines: number; binary: boolean } | null>(null);
  let err = $state<string | null>(null);

  // Load when opened, and again when the file changes.
  const key = $derived(`${file.path}\n${file.signature}`);
  $effect(() => {
    void key;
    untrack(() =>
      briardr.core('GET', `/api/streams/${encodeURIComponent(stream.id)}/changes/diff?path=${encodeURIComponent(file.path)}`, undefined, { timeout: 20_000 }).then(
        (r) => (d = r),
        (e) => (err = e instanceof ApiError ? e.message : 'Couldn’t load the diff.'),
      ),
    );
  });

  const kind = (l: string) => (l.startsWith('+++') || l.startsWith('---') || l.startsWith('diff ') || l.startsWith('index ') ? 'dl-meta' : l.startsWith('@@') ? 'dl-hunk' : l.startsWith('+') ? 'dl-add' : l.startsWith('-') ? 'dl-del' : 'dl-ctx');
  /** Each line with its number: the old file's for removed lines, the new file's otherwise (from the hunk headers). */
  const lines = $derived.by(() => {
    let a = 0;
    let b = 0;
    return (d ? d.text.split('\n').filter((l, i) => i > 0 || l) : []).map((text) => {
      const k = kind(text);
      const h = k === 'dl-hunk' ? /^@@ -(\d+)(?:,\d+)? \+(\d+)/.exec(text) : null;
      if (h) [a, b] = [Number(h[1]), Number(h[2])];
      const n = k === 'dl-del' ? a++ : k === 'dl-add' ? b++ : k === 'dl-ctx' && !text.startsWith('\\') ? (a++, b++) : null;
      return { text, k, n };
    });
  });
</script>

{#if err}
  <p class="error padded">{err}</p>
{:else if !d}
  <p class="hint padded">Loading diff…</p>
{:else if d.binary || file.binary}
  <p class="hint padded">Binary file — no text diff.</p>
{:else}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
  <div class="diff" role="region" aria-label="Diff of {file.path}" tabindex={0}>
    <pre>{#each lines as l}<span class={l.k}>{#if l.k !== 'dl-hunk' && l.k !== 'dl-meta'}<span class="dl-n" aria-hidden="true">{l.n ?? ''}</span>{/if}{l.text + '\n'}</span>{/each}</pre>
    {#if d.truncated}<p class="note">Diff truncated at 2,000 of {d.lines.toLocaleString()} lines.</p>{/if}
  </div>
{/if}
