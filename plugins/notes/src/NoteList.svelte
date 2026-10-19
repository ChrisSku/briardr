<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { ApiError, Empty, Icon, type Stream, type StreamContext } from '@briardr/sdk';
  import NoteCard, { type Item } from './NoteCard.svelte';
  import { cacheNote, copy, draftKey, drafts, inflight, listKey, load, localChange, newId, notesPath, sendToTerminal, store, type Note } from './notes.ts';

  let { stream, briardr, onOpen }: { stream: Stream; briardr: StreamContext; onOpen: (id: string) => void } = $props();
  const theHost = () => briardr.host().the;
  const toast = (text: string) => briardr.toast(text);
  // svelte-ignore state_referenced_locally
  const sid = stream.id; // a new list is made for each stream
  let listState = $state<'loading' | 'ready' | 'offline'>(load(listKey(sid)) ? 'ready' : 'loading');
  let conflicts = $state.raw<string[]>([]);

  // Save drafts left from editing while the Mac was unreachable. A draft based on an older revision
  // stays a draft; opening the note asks which version to keep.
  async function flush(server: Note[]) {
    const bad: string[] = [];
    for (const [id, d] of Object.entries(drafts(sid))) {
      if (inflight.has(id)) continue;
      const cur = server.find((n) => n.id === id);
      if ((cur && cur.text === d.text) || (!cur && d.base === 0 && !d.text.trim())) {
        store(draftKey(sid, id), null);
        continue;
      }
      try {
        const r = await briardr.core<{ note: Note }>('PUT', `${notesPath(sid)}/${id}`, { text: d.text, baseRevision: d.base });
        cacheNote(sid, id, r.note);
        store(draftKey(sid, id), null);
      } catch (e) {
        if (e instanceof ApiError && e.code === 'conflict') bad.push(id);
        else break;
      }
    }
    conflicts = bad;
  }

  async function refresh() {
    try {
      const r = await briardr.core<{ notes: Note[] }>('GET', notesPath(sid));
      store(listKey(sid), r.notes);
      listState = 'ready';
      await flush(r.notes);
    } catch {
      listState = 'offline';
    }
  }

  onMount(() => {
    refresh();
    return briardr.onCoreEvent('notes', (m) => m.streamId === sid && refresh());
  });
  // Back online after showing the last copy: load the current list.
  const connState = $derived(briardr.connection());
  $effect(() => {
    if (connState === 'online' && untrack(() => listState === 'offline')) refresh();
  });

  // This device's copy: the Mac's list with unsaved drafts on top, newest first.
  const server = $derived.by(() => {
    localChange.value;
    return load<Note[]>(listKey(sid)) ?? [];
  });
  const items = $derived.by((): Item[] => {
    localChange.value;
    const ds = drafts(sid);
    return [
      ...server.map((n) => (ds[n.id] ? { ...n, text: ds[n.id].text, pending: true } : { ...n, pending: false })),
      ...Object.entries(ds)
        .filter(([id, d]) => !server.some((n) => n.id === id) && d.text.trim())
        .map(([id, d]) => ({ id, text: d.text, createdAt: d.createdAt, updatedAt: null, pending: true })),
    ].sort((a, b) => b.createdAt - a.createdAt);
  });

  async function remove(it: Item) {
    if (!(await briardr.confirm({ title: 'Delete this note?', message: `It’s removed from ${theHost()} and every device.`, confirm: 'Delete', destructive: true }))) return;
    if (!server.some((n) => n.id === it.id)) return store(draftKey(sid, it.id), null); // never saved
    try {
      await briardr.core('DELETE', `${notesPath(sid)}/${it.id}`);
      store(draftKey(sid, it.id), null);
      cacheNote(sid, it.id, null);
      toast('Note deleted');
    } catch (e) {
      toast(e instanceof ApiError && e.category !== 'network' && e.category !== 'timeout' ? e.message : `Notes can be deleted when ${theHost()} is reachable.`);
    }
  }

  const canSend = $derived(briardr.terminal.canSend);
</script>

<div class="surface notes-list">
  <div class="surface-head">
    <div class="surface-title">
      <strong>Notes</strong>
      <span class="muted notes-status" role="status" aria-live="polite">{listState === 'loading' ? ' · Loading…' : listState === 'offline' ? ` · Can’t reach ${theHost()} — showing the last copy` : items.length ? ` · ${items.length}` : ''}</span>
    </div>
    <button class="btn btn-small btn-primary" onclick={() => onOpen(newId())}>
      <Icon name="plus" size={18} />
      New note
    </button>
  </div>
  {#if listState !== 'loading' && !items.length}
    <Empty title="No notes yet">Ideas, next prompts, things to check later. Saved on {theHost()}; Markdown works.</Empty>
  {/if}
  <ul class="note-cards" aria-label="Notes">
    {#each items as it}
      <NoteCard item={it} {sid} conflict={conflicts.includes(it.id)} {canSend} onOpen={() => onOpen(it.id)} onSend={() => sendToTerminal(briardr, sid, it.text)} onCopy={() => copy(briardr, it.text)} onDelete={() => remove(it)} />
    {/each}
  </ul>
  {#if !!items.length && !canSend}<p class="hint">Take control in Terminal to send a note to it.</p>{/if}
</div>
