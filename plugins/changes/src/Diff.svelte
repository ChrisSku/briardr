<!--
  One changed file's diff. Lines are picked for a review comment by tapping their numbers (a second tap
  makes a range; Shift-click extends one); lines that already have a comment are marked while the file
  stays as it was when the comment was written.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, Icon, Sheet, type PluginContext, type Stream } from '@briardr/sdk';
  import type { ChangedFile, Since } from './changes.ts';
  import { lineId, parseDiff, whereOf } from './diff.ts';
  import { addComment, reviewOf, type Comment } from './review.ts';

  let { stream, briardr, file, since }: { stream: Stream; briardr: PluginContext; file: ChangedFile; since: Since } = $props();
  /** The diff, and the change of the file it is of (`signature`). */
  let d = $state.raw<{ text: string; truncated: boolean; lines: number; binary: boolean; signature: string } | null>(null);
  let err = $state<string | null>(null);
  /** Lines picked for a comment, as positions in `lines` (inclusive). */
  let sel = $state<{ anchor: number; from: number; to: number } | null>(null);

  // Load when opened, and again when the file changes or is compared with something else.
  const key = $derived(`${file.path}\n${file.signature}\n${since}`);
  $effect(() => {
    void key;
    untrack(() => {
      sel = null;
      const signature = file.signature;
      briardr.core('GET', `/api/streams/${encodeURIComponent(stream.id)}/changes/diff?path=${encodeURIComponent(file.path)}${since === 'base' ? '&since=base' : ''}`, undefined, { timeout: 20_000 }).then(
        (r) => (d = { ...r, signature }),
        (e) => (err = e instanceof ApiError ? e.message : 'Couldn’t load the diff.'),
      );
    });
  });

  const lines = $derived(d ? parseDiff(d.text) : []);

  // ---------- picking lines for a comment ----------
  const picked = $derived(sel ? lines.slice(sel.from, sel.to + 1).filter((l) => l.k !== 'dl-meta') : []);
  const where = $derived(whereOf(picked));
  const noted = $derived(new Set(reviewOf(stream.id).comments.filter((c) => c.path === file.path && c.signature === d?.signature).flatMap((c) => c.ids)));
  /** The lines a comment is being written on, taken when Comment is chosen: the diff reloading meanwhile
   *  (the agent changing the file) doesn't change them. */
  let draft = $state.raw<Omit<Comment, 'text'> | null>(null);
  let text = $state('');

  function comment() {
    if (!d || !where) return;
    draft = { path: file.path, signature: d.signature, where, lines: picked.map((l) => l.text).join('\n'), ids: picked.map(lineId).filter((id) => id !== null) };
  }

  // A click on a line's number.
  function onclick(e: MouseEvent) {
    const line = (e.target as Element).closest<HTMLElement>('pre > span');
    const num = line?.querySelector('.dl-n');
    if (!line || !num || e.clientX > num.getBoundingClientRect().right + 8) return;
    const i = Number(line.dataset.i);
    if (lines[i].n === null) return;
    if (sel && (e.shiftKey || sel.from === sel.to) && i !== sel.anchor) sel = { anchor: sel.anchor, from: Math.min(sel.anchor, i), to: Math.max(sel.anchor, i) };
    else if (sel && sel.from === sel.to && i === sel.from) sel = null;
    else sel = { anchor: i, from: i, to: i };
  }

  function add(e: Event) {
    e.preventDefault();
    if (!draft || !text.trim()) return;
    addComment(stream.id, { ...draft, text: text.trim() });
    briardr.record('review_comment_added', { lines: draft.lines.split('\n').length });
    draft = null;
    text = '';
    sel = null;
  }
</script>

{#if err}
  <p class="error padded">{err}</p>
{:else if !d}
  <p class="hint padded">Loading diff…</p>
{:else if d.binary || file.binary}
  <p class="hint padded">Binary file — no text diff.</p>
{:else}
  <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
  <div class="diff" role="region" aria-label="Diff of {file.path}" tabindex={0} onkeydown={(e) => e.key === 'Escape' && sel && (e.stopPropagation(), (sel = null))}>
    <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
    <pre {onclick}>{#each lines as l, i}<span class={[l.k, sel && i >= sel.from && i <= sel.to && 'is-sel', noted.has(lineId(l) ?? '') && 'is-noted']} data-i={i}>{#if l.k !== 'dl-hunk' && l.k !== 'dl-meta'}<span class="dl-n" aria-hidden="true">{l.n ?? ''}</span>{/if}{l.text + '\n'}</span>{/each}</pre>
    {#if d.truncated}<p class="note">Diff truncated at 2,000 of {d.lines.toLocaleString()} lines.</p>{/if}
  </div>
  {#if sel && where}
    <div class="diff-selbar" role="toolbar" aria-label="Selected lines">
      <span class="diff-sel-label">
        {where.includes('-') ? 'Lines' : 'Line'} {where.replace('-', '–')}
        {#if sel.from === sel.to}<small>Tap another number for a range</small>{/if}
      </span>
      <button class="btn btn-small btn-primary" onclick={comment}>Comment…</button>
      <button class="icon-btn" aria-label="Clear selection" title="Clear selection" onclick={() => (sel = null)}><Icon name="close" /></button>
    </div>
  {/if}
  {#if draft}
    <Sheet title="Comment on {draft.path.split('/').pop()} {draft.where}" onClose={() => (draft = null)} initialFocus=".prompt-message">
      <form class="form" onsubmit={add}>
        <div class="prompt-snippet"><pre class="prompt-snippet-code comment-lines">{draft.lines}</pre></div>
        <textarea class="field prompt-message" bind:value={text} aria-label="Comment" placeholder="What should the agent do about these lines?" rows="3"></textarea>
        <p class="hint">Comments are collected for this stream. Send them from Review when you’re done.</p>
        <div class="form-actions">
          <button type="button" class="btn" onclick={() => (draft = null)}>Cancel</button>
          <button class="btn btn-primary" disabled={!text.trim()}>Add comment</button>
        </div>
      </form>
    </Sheet>
  {/if}
{/if}
