<!--
  The note editor. It saves automatically: each save says which revision of the note it was based on;
  if another device saved (or deleted the note) in between, nothing is overwritten and the user
  chooses which version to keep. While the Mac is unreachable, edits are kept as a draft on this
  device and saved when it's back.
  Saving is driven by input events and timers only; no effect writes the text, so edits can't loop
  back into saves.
-->
<script lang="ts">
  import { onMount, untrack } from 'svelte';
  import { ApiError, AsOf, Icon, keyboardInset, prepareImage, type Stream, type StreamContext } from '@briardr/sdk';
  import { renderMarkdown } from '@briardr/sdk/markdown';
  import { cacheNote, copy, draftKey, enc, inflight, listKey, load, MAX_BYTES, notesPath, sendToTerminal, store, type Draft, type Note } from './notes.ts';

  type Status = 'new' | 'loading' | 'saved' | 'saving' | 'unsaved' | 'offline' | 'conflict' | 'error';
  const SAVE_DELAY = 800;

  let { stream, noteId, briardr, onDone }: { stream: Stream; noteId: string; briardr: StreamContext; onDone: () => void } = $props();
  const theHost = () => briardr.host().the;
  const toast = (text: string) => briardr.toast(text);
  // One editor per note (NotesSurface keys it by note ID), so the IDs are fixed here.
  // svelte-ignore state_referenced_locally
  const sid = stream.id;
  // svelte-ignore state_referenced_locally
  const nid = noteId;
  const path = `${notesPath(sid)}/${nid}`;
  const cached = (load<Note[]>(listKey(sid)) ?? []).find((n) => n.id === nid) ?? null;
  const draft = load<Draft>(draftKey(sid, nid));

  let server = $state.raw<Note | null>(cached);
  let text = $state(draft?.text ?? cached?.text ?? '');
  let status = $state<Status>(draft ? 'unsaved' : cached ? 'saved' : 'new');
  let theirs = $state.raw<Note | null>(null); // with status 'conflict'; null: deleted elsewhere
  let preview = $state(false);
  let adding = $state(0);
  let dropping = $state(false);
  // Not shown, so not reactive: the revision edits are based on, and the save in progress.
  let base = draft?.base ?? cached?.revision ?? 0;
  const createdAt = draft?.createdAt ?? cached?.createdAt ?? Date.now();
  let saving = false;
  // A read belongs to the local state it started with. Typing, saving, and newer reads supersede it.
  let refreshGeneration = 0;
  let timer: number | null = null;
  let area: HTMLTextAreaElement | undefined = $state();
  let picker: HTMLInputElement;
  const kb = keyboardInset(); // keep the editor above the on-screen keyboard

  function accept(n: Note) {
    refreshGeneration++;
    server = n;
    cacheNote(sid, nid, n);
    base = n.revision;
  }
  // Whether this device holds a copy of the unsaved text. If it can't (storage full or not allowed), the
  // editor says so and offers to copy the text, and Briardr doesn't reload for an update meanwhile.
  let draftKept = $state(true);
  const keepDraft = (v: string) => {
    const kept = store(draftKey(sid, nid), !server && !v.trim() ? null : { text: v, base, createdAt });
    if (kept !== draftKept) briardr.record('note_draft_stored', { stored: kept });
    draftKept = kept;
  };
  const dropDraft = () => {
    store(draftKey(sid, nid), null);
    draftKept = true;
  };

  async function save() {
    if (timer) clearTimeout(timer);
    timer = null;
    if (saving) return;
    const sent = text;
    const srv = server;
    if (srv ? sent === srv.text && base === srv.revision : !sent.trim()) {
      dropDraft();
      status = srv ? 'saved' : 'new'; // nothing to save; an empty new note isn't kept
      return;
    }
    if (enc.encode(sent).length > MAX_BYTES) return (status = 'error');
    saving = true;
    inflight.add(nid);
    status = 'saving';
    try {
      const r = await briardr.core<{ note: Note }>('PUT', path, { text: sent, baseRevision: base });
      accept(r.note);
      if (text === sent) {
        dropDraft();
        status = 'saved';
      } else schedule(); // typed more while saving
    } catch (e) {
      if (e instanceof ApiError && e.code === 'conflict') {
        const res = await briardr.core<{ note: Note }>('GET', path).catch(() => null);
        theirs = res?.note ?? null;
        status = 'conflict';
      } else if (e instanceof ApiError && (e.category === 'network' || e.category === 'timeout')) status = 'offline';
      else {
        if (e instanceof ApiError && e.code === 'too_many') toast(e.message);
        status = 'error';
      }
    } finally {
      saving = false;
      inflight.delete(nid);
    }
  }

  function schedule() {
    if (timer) clearTimeout(timer);
    timer = window.setTimeout(save, SAVE_DELAY);
  }

  function onInput(v: string) {
    refreshGeneration++;
    text = v;
    keepDraft(v);
    if (status !== 'conflict') {
      status = 'unsaved';
      schedule();
    }
  }

  // Load (or refresh) from the Mac. Never replaces unsaved local edits.
  async function refresh() {
    const generation = ++refreshGeneration;
    const hasDraft = !!load(draftKey(sid, nid));
    try {
      const r = await briardr.core<{ note: Note }>('GET', path);
      if (generation !== refreshGeneration || saving) return;
      if (!hasDraft) {
        accept(r.note);
        text = r.note.text;
        status = 'saved';
      } else if (r.note.revision !== base) {
        theirs = r.note;
        status = 'conflict';
      } else {
        server = r.note;
        save();
      }
    } catch (e) {
      if (generation !== refreshGeneration || saving) return;
      if (e instanceof ApiError && e.code === 'note_gone') {
        if (base === 0) {
          if (hasDraft) save(); // a new note written while offline
          return;
        }
        if (hasDraft) {
          theirs = null; // deleted on another device while edited here
          status = 'conflict';
          return;
        }
        cacheNote(sid, nid, null);
        toast('This note was deleted on another device.');
        return onDone();
      }
      if (!server && status !== 'new') status = 'offline';
    }
  }

  onMount(() => {
    if (status === 'new') area?.focus(); // a new note is ready to type
    refresh();
    // Another device saved: pick it up if nothing is pending here.
    const off = briardr.onCoreEvent('notes', (m) => {
      if (m.streamId === sid && m.noteId === nid && m.revision !== base) refresh();
    });
    // Save when leaving the editor or the app; retry a pending draft when the network returns.
    const onHide = () => document.visibilityState === 'hidden' && timer && save();
    const onOnline = () => load(draftKey(sid, nid)) && refresh();
    document.addEventListener('visibilitychange', onHide);
    window.addEventListener('online', onOnline);
    return () => {
      refreshGeneration++;
      off();
      document.removeEventListener('visibilitychange', onHide);
      window.removeEventListener('online', onOnline);
      if (timer) save();
    };
  });

  // Back online: save a pending draft.
  const connState = $derived(briardr.connection());
  $effect(() => {
    if (connState === 'online' && untrack(() => status === 'offline' || status === 'unsaved')) schedule();
  });

  function resolve(keep: 'mine' | 'theirs') {
    refreshGeneration++;
    if (keep === 'theirs') {
      dropDraft();
      if (!theirs) {
        cacheNote(sid, nid, null);
        return onDone(); // it was deleted; let it go
      }
      accept(theirs);
      text = theirs.text;
      status = 'saved';
    } else {
      base = theirs?.revision ?? 0; // save mine on top of the newer version, or as a new note
      server = theirs;
      keepDraft(text);
      status = 'unsaved';
      setTimeout(save, 0);
    }
    theirs = null;
  }

  // Back to the list. A note emptied here is deleted rather than kept as a blank card.
  async function done() {
    if (timer) await save();
    if (!text.trim() && server && status !== 'conflict') {
      try {
        await briardr.core('DELETE', path);
        dropDraft();
        cacheNote(sid, nid, null);
        toast('Empty note deleted');
      } catch {}
    }
    onDone();
  }

  // Upload images and insert a reference to each where the cursor is (or at the end in Preview).
  async function addImages(files: File[]) {
    const images = files.filter((f) => f.type.startsWith('image/'));
    if (!images.length) return;
    if (briardr.connection() !== 'online') return toast(`Images can be added when ${theHost()} is reachable.`);
    let at = area && !preview ? area.selectionEnd : text.length;
    adding += images.length;
    for (const f of images) {
      try {
        const img = await prepareImage(f, f.name);
        URL.revokeObjectURL(img.url);
        const r = await briardr.upload(`${notesPath(sid)}/images`, img.blob);
        const alt = (f.name || '').replace(/\.[^.]+$/, '').replace(/[[\]]/g, '').trim() || 'image';
        const cur = text;
        at = Math.min(at, cur.length);
        const before = cur.slice(0, at);
        const after = cur.slice(at);
        const md = `${before && !before.endsWith('\n') ? '\n' : ''}![${alt}](note:${r.name})${after.startsWith('\n') ? '' : '\n'}`;
        onInput(before + md + after);
        at += md.length;
        const caret = at;
        requestAnimationFrame(() => document.activeElement === area && area?.setSelectionRange(caret, caret));
        if (!preview) toast('Image added. Preview shows it.');
      } catch (e) {
        toast(e instanceof ApiError ? e.message : 'Couldn’t add that image.');
      } finally {
        adding--;
      }
    }
  }

  // Read an image from the clipboard (iOS asks with its own Paste button first).
  async function pasteImage() {
    if (!navigator.clipboard?.read) return toast('This browser can’t read images from the clipboard. Use Photo or file.');
    try {
      const files: File[] = [];
      for (const item of await navigator.clipboard.read()) {
        const type = item.types.find((t) => t.startsWith('image/'));
        if (type) files.push(new File([await item.getType(type)], 'pasted image', { type }));
      }
      if (!files.length) return toast('There’s no image on the clipboard.');
      addImages(files);
    } catch {
      toast('Couldn’t read the clipboard.');
    }
  }

  const filesOf = (dt: DataTransfer | null) => Array.from(dt?.files ?? []).filter((f) => f.type.startsWith('image/'));

  function selection() {
    if (!area || preview) return text;
    const s = area.value.slice(area.selectionStart, area.selectionEnd);
    return s || area.value;
  }

  const STATUS_TEXT: Record<Exclude<Status, 'conflict' | 'error'>, string> = {
    new: 'New note',
    loading: 'Loading…',
    saved: `Saved on ${theHost()}`,
    saving: 'Saving…',
    unsaved: 'Editing…',
    offline: `Not saved yet — kept on this device until ${theHost()} is reachable`,
  };
  // The note's size in bytes only matters for an error, so it isn't measured on every keystroke.
  const statusText = $derived(
    !draftKept && (status === 'offline' || status === 'unsaved' || status === 'saving' || status === 'error')
      ? 'Not saved, and this device couldn’t keep a copy'
      : status === 'conflict'
      ? theirs
        ? 'Changed on another device'
        : 'Deleted on another device'
      : status === 'error'
        ? enc.encode(text).length > MAX_BYTES
          ? 'Too long — notes are limited to 256 KB'
          : 'Couldn’t save'
        : STATUS_TEXT[status],
  );
  const canInsert = $derived(briardr.terminal.canSend);
</script>

<!-- Images can be dropped anywhere on the editor. -->
<!-- svelte-ignore a11y_no_static_element_interactions -->
<div
  class={['surface notes', dropping && 'notes-drop']}
  style:padding-bottom={kb.current ? `${kb.current + 8}px` : undefined}
  ondragover={(e) => {
    if (!e.dataTransfer?.types.includes('Files')) return;
    e.preventDefault();
    dropping = true;
  }}
  ondragleave={(e) => e.currentTarget === e.target && (dropping = false)}
  ondrop={(e) => {
    dropping = false;
    const files = filesOf(e.dataTransfer);
    if (!files.length) return;
    e.preventDefault();
    addImages(files);
  }}
>
  <div class="surface-head">
    <button class="icon-btn notes-back" aria-label="All notes" title="All notes" onclick={done}>
      <Icon name="back" />
    </button>
    <div class="surface-title">
      <strong>Note</strong>
      <span class="muted notes-status st-{status}" role="status" aria-live="polite">{' · '}{statusText}</span>
      {#if status === 'saved' && server?.updatedAt}<AsOf at={server.updatedAt} prefix="saved" />{/if}
    </div>
    <div class="segmented small" role="group" aria-label="View">
      <button aria-pressed={!preview} class={!preview ? 'is-on' : ''} onclick={() => (preview = false)}>Edit</button>
      <button aria-pressed={preview} class={preview ? 'is-on' : ''} onclick={() => (preview = true)}>Preview</button>
    </div>
  </div>
  {#if !draftKept && status !== 'saved'}
    <div class="note tone-error" role="alert">
      <p>This device couldn’t keep a copy of your changes (its storage is full or not available), and they aren’t saved on {theHost()} yet. Copy the text to keep it somewhere safe.</p>
      <div class="form-actions">
        <button class="btn btn-small btn-primary" onclick={() => copy(briardr, text)}>Copy text</button>
      </div>
    </div>
  {/if}
  {#if status === 'conflict'}
    <div class="note conflict" role="alert">
      {#if theirs}
        <p>This note was changed on another device{theirs.updatedAt ? ' ' : ''}{#if theirs.updatedAt}<AsOf at={theirs.updatedAt} prefix="" />{/if}. Which version do you want to keep?</p>
      {:else}
        <p>This note was deleted on another device while you were editing it here. Keep your version?</p>
      {/if}
      <div class="form-actions">
        <button class="btn btn-small" onclick={() => resolve('theirs')}>{theirs ? 'Use the other version' : 'Discard it'}</button>
        <button class="btn btn-small btn-primary" onclick={() => resolve('mine')}>Keep mine</button>
      </div>
    </div>
  {/if}
  {#if preview}
    <article class="md notes-preview">{@html renderMarkdown(text || '_Nothing written yet._', { docPath: '', streamId: sid, notes: true })}</article>
  {:else}
    <textarea
      bind:this={area}
      class="field notes-area"
      data-keeps-draft={draftKept || undefined}
      bind:value={() => text, onInput}
      onblur={() => timer && save()}
      onpaste={(e) => {
        const files = filesOf(e.clipboardData);
        if (!files.length) return;
        e.preventDefault();
        addImages(files);
      }}
      placeholder="Ideas, next prompts, things to check later. Saved on {theHost()}; Markdown works."
      aria-label="Note for {stream.name}"
      spellcheck="true"
    ></textarea>
  {/if}
  <div class="notes-actions">
    <button class="btn btn-small" onclick={() => copy(briardr, selection())}>Copy</button>
    <button
      class="btn btn-small"
      disabled={adding > 0}
      aria-haspopup="menu"
      onclick={(e) =>
        briardr.menu(
          [
            { label: 'Paste image', hint: 'From the clipboard', onSelect: pasteImage },
            { label: 'Photo or file', onSelect: () => picker?.click() },
          ],
          e.currentTarget,
          'Add image',
        )}
    >
      {adding > 0 ? 'Adding image…' : 'Add image'}
    </button>
    <input
      bind:this={picker}
      type="file"
      accept="image/*"
      multiple
      hidden
      onchange={(e) => {
        const input = e.currentTarget;
        addImages(Array.from(input.files ?? []));
        input.value = '';
      }}
    />
    <button class="btn btn-small" disabled={!canInsert} title={canInsert ? 'Paste the note (or the selected part) into the terminal' : 'Take control of the terminal first'} onclick={() => sendToTerminal(briardr, sid, selection())}>Send to terminal</button>
    {#if !canInsert && !kb.current}<span class="hint">Take control in Terminal to send.</span>{/if}
  </div>
</div>
