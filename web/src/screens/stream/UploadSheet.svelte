<!--
  Sending images and files to a stream: what was picked, pasted or dropped is reviewed (an image as the
  PNG it becomes), uploaded to a private folder on the host, and the quoted paths are inserted as one
  paste. Like every insert, it's never submitted.
-->
<script lang="ts">
  import { theHost } from '../../host.ts';
  import { onMount } from 'svelte';
  import { ApiError, upload } from '../../api.ts';
  import { record } from '../../diag.ts';
  import { prepareImage, type PreparedImage } from '../../media.ts';
  import type { Uploaded } from '../../protocol.ts';
  import { toast } from '../../store.ts';
  import type { TerminalController } from '../../terminal/controller.ts';
  import type { Stream } from '../../types.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { holdUpdates } from '../../updates.ts';
  import { acceptsImages, joinPaths, type UploadItem } from './insert.ts';

  let { ctl, stream, items, onClose }: { ctl: TerminalController; stream: Stream; items: UploadItem[]; onClose: () => void } = $props();

  const MAX_FILE_BYTES = 100 * 1024 * 1024;
  type Phase = { k: 'review' } | { k: 'uploading'; sent: number; total: number } | { k: 'error'; message: string } | { k: 'not_inserted'; why: string; paths: string[] };
  interface Entry extends UploadItem {
    name: string;
    /** An image as it will be sent, once it's ready. */
    img: PreparedImage | null;
    prepError: boolean;
  }
  // svelte-ignore state_referenced_locally
  let entries = $state.raw<Entry[]>(items.map((it) => ({ ...it, name: (it.file as File).name || (it.kind === 'image' ? 'image.png' : 'file'), img: null, prepError: false })));
  let phase = $state.raw<Phase>({ k: 'review' });
  // The connection generation when the items were picked. If it changes, the uploads aren't inserted.
  // svelte-ignore state_referenced_locally
  const gen = ctl.generation.value;
  let abort: AbortController | null = null;

  const one = $derived(entries.length === 1 ? entries[0] : null);
  const images = $derived(entries.filter((e) => e.kind === 'image'));
  const noun = $derived(images.length === entries.length ? 'image' : 'file');
  const tooLarge = (e: Entry) => e.kind === 'file' && e.file.size > MAX_FILE_BYTES;
  const ready = $derived(entries.length > 0 && entries.every((e) => (e.kind === 'image' ? e.img : !tooLarge(e))));
  const size = (bytes: number) => `${(bytes / 1024).toFixed(bytes > 1024 * 1024 ? 0 : 1)} KB`;

  onMount(() => {
    // A reload would drop the picked files, so an update waits until the sheet closes.
    const release = holdUpdates('you send or close the file');
    let closed = false;
    // One image at a time: each is decoded at full size, and a phone has memory for only a few.
    (async () => {
      for (const e of entries.filter((x) => x.kind === 'image')) {
        if (!entries.includes(e)) continue; // removed before its turn
        const img = await prepareImage(e.file, e.name).catch(() => null);
        // Closed or removed meanwhile: nothing will show it.
        if (closed || !entries.includes(e)) {
          if (img) URL.revokeObjectURL(img.url);
        } else entries = entries.map((x) => (x === e ? { ...x, img, prepError: !img } : x));
        if (closed) return;
      }
    })();
    return () => {
      closed = true;
      abort?.abort();
      release();
      for (const e of entries) if (e.img) URL.revokeObjectURL(e.img.url);
    };
  });

  function remove(e: Entry) {
    if (e.img) URL.revokeObjectURL(e.img.url);
    entries = entries.filter((x) => x !== e);
  }

  async function start() {
    const parts = entries.map((e) => ({ kind: e.kind, blob: e.kind === 'image' ? e.img!.blob : e.file, name: e.kind === 'image' ? e.img!.name : e.name }));
    abort = new AbortController();
    const total = parts.reduce((n, p) => n + p.blob.size, 0);
    let done = 0;
    phase = { k: 'uploading', sent: 0, total };
    const t0 = performance.now();
    try {
      // One after the other: the paths are inserted in the order the files were picked.
      const stored: Uploaded[] = [];
      for (const p of parts) {
        stored.push(await upload<Uploaded>(`/api/streams/${encodeURIComponent(stream.id)}/uploads?kind=${p.kind}&name=${encodeURIComponent(p.name)}`, p.blob, (sent) => (phase = { k: 'uploading', sent: done + sent, total }), abort.signal));
        done += p.blob.size;
      }
      record('upload_done', { kind: noun, count: parts.length, ms: Math.round(performance.now() - t0) });
      if (gen !== ctl.generation.value || !ctl.canInput) {
        const why = gen !== ctl.generation.value ? `the connection to ${theHost()} changed since you picked this` : 'you aren’t controlling this stream';
        phase = { k: 'not_inserted', why, paths: stored.map((r) => r.path) };
        return;
      }
      ctl.paste(joinPaths(stored.map((r) => r.quoted), stream), true, gen);
      toast(one ? (one.kind === 'image' && acceptsImages(stream) ? 'Image path inserted. Press Enter when ready.' : 'Path inserted. Press Enter when ready.') : `${parts.length} paths inserted. Press Enter when ready.`);
      onClose();
    } catch (e) {
      if (e instanceof ApiError && e.code === 'cancelled') return (phase = { k: 'review' });
      phase = { k: 'error', message: e instanceof ApiError ? e.message : 'Upload failed.' };
    }
  }

  const plainShell = $derived(images.length > 0 && !acceptsImages(stream));
</script>

{#snippet imageInfo(img: PreparedImage)}
  {img.width} × {img.height} px PNG · {(img.blob.size / 1024 / 1024).toFixed(1)} MB{img.scaled ? ' · scaled to fit 4096 px' : ''}
{/snippet}

<Sheet
  title={one ? (one.kind === 'image' ? 'Send image' : 'Send file') : `Send ${entries.length} ${noun}s`}
  {onClose}
  wide
>
  {#if one?.kind === 'image'}
    <div class="upload-preview">
      {#if one.img}
        <img src={one.img.url} alt="Preview of what will be sent" />
      {:else if one.prepError}
        <p class="error">This image couldn’t be read. Try a different one.</p>
      {:else}
        <p class="hint">Preparing image…</p>
      {/if}
      {#if one.img}
        <p class="hint">{@render imageInfo(one.img)}. Orientation corrected; camera and location details removed.</p>
      {/if}
    </div>
  {:else if one}
    <p class="file-line"><strong>{one.name}</strong> · {size(one.file.size)}</p>
    {#if tooLarge(one)}<p class="error">Files are limited to 100 MB.</p>{/if}
  {:else}
    <ul class="upload-list" aria-label="What will be sent">
      {#each entries as e (e.file)}
        <li class="upload-item">
          {#if e.img}<img src={e.img.url} alt="" />{:else}<span class="upload-thumb"><Icon name={e.kind === 'image' ? 'image' : 'file'} size={20} /></span>{/if}
          <span class="upload-text">
            <strong>{e.name}</strong>
            <span class={e.prepError || tooLarge(e) ? 'error' : 'hint'}>
              {#if e.kind === 'file'}{size(e.file.size)}{tooLarge(e) ? ' · over the 100 MB limit' : ''}{:else if e.img}{@render imageInfo(e.img)}{:else if e.prepError}This image couldn’t be read.{:else}Preparing image…{/if}
            </span>
          </span>
          {#if phase.k === 'review' || phase.k === 'error'}
            <button class="icon-btn" aria-label="Remove {e.name}" title="Remove" onclick={() => remove(e)}><Icon name="close" size={18} /></button>
          {/if}
        </li>
      {/each}
    </ul>
    {#if images.length}<p class="hint">Images are sent as PNG: orientation corrected; camera and location details removed.</p>{/if}
  {/if}
  {#if plainShell}
    <p class="note" role="note">
      This stream is {stream.kind === 'agent' ? `${stream.tool}, which may not read images` : `a ${stream.kind === 'shell' ? 'plain shell' : stream.kind}`}. Briardr will insert the uploaded {one ? 'file’s path' : 'files’ paths'} as text; the program won’t see an image attachment.
    </p>
  {/if}
  <p class="hint">
    {#if one}The file is uploaded to a private folder on {theHost()} and its quoted path is inserted as one paste.{:else}The files are uploaded to a private folder on {theHost()} and their quoted paths are inserted as one paste.{/if}
    Briardr never presses Enter.
  </p>
  {#if phase.k === 'error'}<p class="error" role="alert">{phase.message}</p>{/if}
  {#if phase.k === 'not_inserted'}
    <p class="note" role="alert">
      Uploaded, but not inserted: {phase.why}, so Briardr didn’t type into the terminal.
      {#if phase.paths.length === 1}The file is at <code>{phase.paths[0]}</code>.{:else}The files are at:{#each phase.paths as p}<br /><code>{p}</code>{/each}{/if}
    </p>
  {/if}
  {#snippet footer()}
    {#if phase.k === 'uploading'}
      <progress max={phase.total} value={phase.sent} aria-label="Upload progress"></progress>
      <button class="btn" onclick={() => abort?.abort()}>Cancel upload</button>
    {:else if phase.k === 'not_inserted'}
      {@const paths = phase.paths}
      <button class="btn" onclick={() => navigator.clipboard.writeText(paths.join('\n')).then(() => toast(paths.length === 1 ? 'Path copied' : 'Paths copied'))}>{paths.length === 1 ? 'Copy path' : 'Copy paths'}</button>
      <button class="btn btn-primary" onclick={onClose}>Done</button>
    {:else}
      <button class="btn" onclick={onClose}>Cancel</button>
      <button class="btn btn-primary" disabled={!ready} onclick={start}>{one ? 'Upload and insert path' : `Upload and insert ${entries.length} paths`}</button>
    {/if}
  {/snippet}
</Sheet>
