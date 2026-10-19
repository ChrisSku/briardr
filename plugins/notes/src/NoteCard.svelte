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
  import { renderMarkdown } from '@briardr/sdk/markdown';

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
