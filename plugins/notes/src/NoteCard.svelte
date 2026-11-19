<script module lang="ts">
  export interface Item {
    id: string;
    text: string;
    createdAt: number;
    updatedAt: number | null;
    pending: boolean;
  }
</script>

<script lang="ts">
  import { AsOf, Icon } from '@briardr/sdk';
  import { renderDiagrams, renderMarkdown } from '@briardr/sdk/markdown';

  interface Props {
    item: Item;
    sid: string;
    conflict: boolean;
    canSend: boolean;
    onOpen: () => void;
    onSend: () => void;
    onCopy: () => void;
    onDelete: () => void;
  }
  let { item, sid, conflict, canSend, onOpen, onSend, onCopy, onDelete }: Props = $props();
  let body: HTMLDivElement;
  let clipped = $state(false);
  const measure = () => (clipped = body.scrollHeight > body.clientHeight + 1);
  const html = $derived(renderMarkdown(item.text, { docPath: '', streamId: sid, notes: true }));
  // Measure once the text is in place (and again when images load and change the height).
  $effect(() => {
    void html;
    measure();
    void renderDiagrams(body).then(measure);
  });
  const title = $derived(item.text.trim().split('\n')[0].replace(/^#+\s*/, '').slice(0, 60));
</script>

<li class="note-card" aria-label={title || 'Note'}>
  <div class="note-card-meta muted">
    {#if conflict}
      <span class="notes-status st-conflict">Changed on another device — open to choose</span>
    {:else if item.pending}
      <span class="notes-status st-offline">Not saved yet</span>
    {:else}
      <AsOf at={item.updatedAt} prefix="edited" />
    {/if}
  </div>
  <!-- Tapping the text opens the note, a pointer shortcut for the Edit button; links in it still work. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div bind:this={body} class={['md note-card-body', clipped && 'is-clipped']} onclick={(e) => !(e.target as Element).closest('a') && onOpen()} onloadcapture={measure}>{@html html}</div>
  <div class="note-card-actions">
    <button class="btn btn-small" onclick={onOpen}>Edit</button>
    <button class="btn btn-small" onclick={onCopy}>Copy</button>
    <button class="btn btn-small" disabled={!canSend} title={canSend ? 'Paste the note into the terminal' : 'Take control of the terminal first'} onclick={onSend}>Send to terminal</button>
    <button class="icon-btn note-delete" aria-label="Delete note" title="Delete note" onclick={onDelete}>
      <Icon name="trash" size={20} />
    </button>
  </div>
</li>

<style>
  /* A note in the list: a card cut off at a fixed height, fading out when there's more. */
  .note-card { border: 1px solid var(--line-default); border-radius: 14px; background: var(--bg-sunken); padding: 10px 12px 8px; display: flex; flex-direction: column; gap: 4px; min-width: 0; }
  .note-card-meta { font-size: 0.78rem; }
  .notes-status { color: var(--state-attention); font-weight: 600; }
  .note-card-body { max-height: 168px; overflow: hidden; padding: 0; cursor: pointer; }
  .note-card-body.is-clipped { -webkit-mask-image: linear-gradient(to bottom, #000 calc(100% - 44px), transparent); mask-image: linear-gradient(to bottom, #000 calc(100% - 44px), transparent); }
  .note-card-body > :global(:first-child) { margin-top: 0; }
  .note-card-body > :global(:last-child) { margin-bottom: 0; }
  .note-card-body :global(:is(h1, h2, h3)) { font-size: 1.05rem; margin: 8px 0 4px; padding: 0; border: 0; }
  .note-card-body :global(img) { display: block; max-height: 120px; object-fit: contain; margin: 6px 0; }
  .note-card-body :global(:is(pre, code)) { background: var(--bg-surface); }
  .note-card-actions { display: flex; gap: 6px; align-items: center; flex-wrap: wrap; padding-top: 4px; }
  .note-card-actions .btn { background: var(--bg-surface); }
  .note-delete { margin-left: auto; width: 38px; height: 34px; }
  .note-delete:hover { color: var(--state-danger); }
</style>
