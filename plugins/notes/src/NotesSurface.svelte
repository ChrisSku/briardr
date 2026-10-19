<!--
  Notes: any number of free-form notes per stream, saved on the Mac so they're the same on every device.
  - The list shows each note as a card (cut off at a fixed height) with Edit, Copy, Send to terminal
    and Delete. "New note" opens an empty editor; the note appears as a card once it's saved.
  - The editor saves automatically and never overwrites another device's newer version.
  - "Send to terminal" pastes the note (or the selection) as one paste and never presses Enter.
  - Images (pasted, dropped or picked) are stored on the Mac and referenced in the Markdown as
    `![name](note:<id>.png)`; Preview and the cards show them. Sent to a terminal, each image is the
    path of a copy on the Mac instead, which Claude Code and Codex attach as an image. Copy keeps the
    Markdown.
-->
<script lang="ts">
  import type { StreamTabProps } from '@briardr/sdk';
  import NoteEditor from './NoteEditor.svelte';
  import NoteList from './NoteList.svelte';
  import { openNote, setOpenNote } from './notes.ts';

  let { stream, visible, briardr }: StreamTabProps = $props();
  // The stream screen is keyed by stream, so the ID is fixed for this instance.
  // svelte-ignore state_referenced_locally
  const sid = stream.id;
  let open = $state<string | null>(openNote(sid));
  function edit(id: string | null) {
    setOpenNote(sid, id);
    open = id;
  }
</script>

{#if visible}
  {#if open}
    {#key open}
      <NoteEditor {stream} noteId={open} {briardr} onDone={() => edit(null)} />
    {/key}
  {:else}
    <NoteList {stream} {briardr} onOpen={edit} />
  {/if}
{/if}
