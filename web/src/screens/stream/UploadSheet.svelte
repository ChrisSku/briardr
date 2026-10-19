<script lang="ts">
  import { theHost } from '../../host.ts';
  import { onMount } from 'svelte';
  import { ApiError, upload } from '../../api.ts';
  import { record } from '../../diag.ts';
  import { prepareImage, type PreparedImage } from '../../media.ts';
  import { toast } from '../../store.ts';
  import type { TerminalController } from '../../terminal/controller.ts';
  import type { Stream } from '../../types.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { holdUpdates } from '../../updates.ts';
  import { acceptsImages } from './insert.ts';

  let { ctl, stream, file, kind, onClose }: { ctl: TerminalController; stream: Stream; file: File | Blob; kind: 'image' | 'file'; onClose: () => void } = $props();

  type Phase = { k: 'review' } | { k: 'uploading'; sent: number; total: number } | { k: 'error'; message: string } | { k: 'not_inserted'; path: string; quoted: string };
  let img = $state.raw<PreparedImage | null>(null);
  let prepError = $state<string | null>(null);
  let phase = $state.raw<Phase>({ k: 'review' });
  // The connection generation when the item was picked. If it changes, the upload isn't inserted.
  // svelte-ignore state_referenced_locally
  const gen = ctl.generation.value;
  let abort: AbortController | null = null;
  // svelte-ignore state_referenced_locally
  const name = (file as File).name || (kind === 'image' ? 'image.png' : 'file');

  onMount(() => {
    // A reload would drop the picked file, so an update waits until the sheet closes.
    const release = holdUpdates('you send or close the file');
    if (kind === 'image')
      prepareImage(file, name).then(
        (r) => (img = r),
        () => (prepError = 'This image couldn’t be read. Try a different one.'),
      );
    return () => {
      release();
      if (img) URL.revokeObjectURL(img.url);
    };
  });

  async function start() {
    const blob = kind === 'image' ? img!.blob : file;
    const nm = kind === 'image' ? img!.name : name;
    if (kind === 'file' && blob.size > 100 * 1024 * 1024) return (phase = { k: 'error', message: 'Files are limited to 100 MB.' });
    abort = new AbortController();
    phase = { k: 'uploading', sent: 0, total: blob.size };
    const t0 = performance.now();
    try {
      const r = await upload(`/api/streams/${encodeURIComponent(stream.id)}/uploads?kind=${kind}&name=${encodeURIComponent(nm)}`, blob, (sent, total) => (phase = { k: 'uploading', sent, total }), abort.signal);
      record('upload_done', { kind, ms: Math.round(performance.now() - t0) });
      if (gen !== ctl.generation.value || !ctl.canInput) {
        phase = { k: 'not_inserted', path: r.path, quoted: r.quoted };
        return;
      }
      ctl.paste(r.quoted, true, gen);
      toast(kind === 'image' && acceptsImages(stream) ? 'Image path inserted. Press Enter when ready.' : 'Path inserted. Press Enter when ready.');
      onClose();
    } catch (e) {
      if (e instanceof ApiError && e.code === 'cancelled') return (phase = { k: 'review' });
      phase = { k: 'error', message: e instanceof ApiError ? e.message : 'Upload failed.' };
    }
  }

  const plainShell = $derived(kind === 'image' && !acceptsImages(stream));
</script>

<Sheet
  title={kind === 'image' ? 'Send image' : 'Send file'}
  onClose={() => {
    abort?.abort();
    onClose();
  }}
  wide
>
  {#if kind === 'image'}
    <div class="upload-preview">
      {#if img}
        <img src={img.url} alt="Preview of what will be sent" />
      {:else if prepError}
        <p class="error">{prepError}</p>
      {:else}
        <p class="hint">Preparing image…</p>
      {/if}
      {#if img}
        <p class="hint">
          {img.width} × {img.height} px PNG · {(img.blob.size / 1024 / 1024).toFixed(1)} MB{img.scaled ? ' · scaled to fit 4096 px' : ''}. Orientation corrected; camera and location details removed.
        </p>
      {/if}
    </div>
  {/if}
  {#if kind === 'file'}
    <p class="file-line"><strong>{name}</strong> · {(file.size / 1024).toFixed(file.size > 1024 * 1024 ? 0 : 1)} KB</p>
  {/if}
  {#if plainShell}
    <p class="note" role="note">
      This stream is {stream.kind === 'agent' ? `${stream.tool}, which may not read images` : `a ${stream.kind === 'shell' ? 'plain shell' : stream.kind}`}. Briardr will insert the uploaded file’s path as text; the program won’t see an image attachment.
    </p>
  {/if}
  <p class="hint">The file is uploaded to a private folder on {theHost()} and its quoted path is inserted as one paste. Briardr never presses Enter.</p>
  {#if phase.k === 'error'}<p class="error" role="alert">{phase.message}</p>{/if}
  {#if phase.k === 'not_inserted'}
    <p class="note" role="alert">
      Uploaded, but not inserted: the connection to {theHost()} changed since you picked this, so Briardr didn’t type into the terminal. The file is at <code>{phase.path}</code>.
    </p>
  {/if}
  {#snippet footer()}
    {#if phase.k === 'uploading'}
      <progress max={phase.total} value={phase.sent} aria-label="Upload progress"></progress>
      <button class="btn" onclick={() => abort?.abort()}>Cancel upload</button>
    {:else if phase.k === 'not_inserted'}
      {@const path = phase.path}
      <button class="btn" onclick={() => navigator.clipboard.writeText(path).then(() => toast('Path copied'))}>Copy path</button>
      <button class="btn btn-primary" onclick={onClose}>Done</button>
    {:else}
      <button class="btn" onclick={onClose}>Cancel</button>
      <button class="btn btn-primary" disabled={(kind === 'image' && !img) || !!prepError} onclick={start}>Upload and insert path</button>
    {/if}
  {/snippet}
</Sheet>
