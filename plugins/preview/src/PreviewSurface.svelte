<!--
  Preview: a dev server on the host's localhost, shown through Briardr's authenticated connection.
  Direct (Tailscale/LAN): another port of this host. Behind a public proxy: the preview's own origin,
  entered through a one-time launch link from the server (a new one for every load).
-->
<script module lang="ts">
  export function parseLocalUrl(url: string): { port: number; path: string } | null {
    try {
      const u = new URL(url);
      const port = Number(u.port || (u.protocol === 'https:' ? 443 : 80));
      return { port, path: `${u.pathname}${u.search}${u.hash}` || '/' };
    } catch {
      return null;
    }
  }
</script>

<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, Icon, type StreamTabProps } from '@briardr/sdk';
  import { consumePreviewRequest, previewRequests } from './requests.ts';

  interface PreviewInfo {
    id: string;
    targetPort: number;
    listenPort: number;
    origin?: string;
    launch?: string;
  }

  let { stream, visible, briardr }: StreamTabProps = $props();
  const theHost = () => briardr.host().the;
  const request = $derived(previewRequests.value[stream.id] ?? null);
  const onConsumed = () => consumePreviewRequest(stream.id);
  let port = $state('');
  let path = $state('/');
  let prev = $state.raw<PreviewInfo | null>(null);
  let src = $state<string | null>(null);
  let error = $state<string | null>(null);
  let busy = $state(false);
  let nonce = $state(0);
  let zoom = $state(100);

  async function close() {
    if (prev) await briardr.core('DELETE', `/api/previews/${prev.id}`).catch(() => {});
    prev = null;
    src = null;
    zoom = 100;
  }

  async function open(p: number, pth: string) {
    busy = true;
    error = null;
    try {
      if (prev && prev.targetPort !== p) await close();
      const r = await briardr.core<{ preview: PreviewInfo }>('POST', '/api/previews', { port: p });
      prev = r.preview;
      port = String(p);
      path = pth;
      const rel = pth.startsWith('/') ? pth : '/' + pth;
      src = r.preview.launch ? `${r.preview.launch}&path=${encodeURIComponent(rel)}` : `https://${location.hostname}:${r.preview.listenPort}${rel}`;
      nonce++;
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Couldn’t open the preview.';
    } finally {
      busy = false;
    }
  }

  // A tapped terminal link (keeps its path and query).
  $effect(() => {
    if (!request) return;
    const p = parseLocalUrl(request);
    untrack(() => {
      onConsumed();
      if (p) open(p.port, p.path);
    });
  });

  // Release the dev-server connection when the preview closes (leaving the stream unmounts this).
  const previewId = $derived(prev?.id);
  $effect(() => {
    const id = previewId;
    return () => void (id && briardr.core('DELETE', `/api/previews/${id}`).catch(() => {}));
  });

  function onsubmit(e: Event) {
    e.preventDefault();
    const n = Number(port);
    if (!Number.isInteger(n) || n < 1 || n > 65535) return (error = 'Enter a port between 1 and 65535.');
    open(n, path || '/');
  }
</script>

<div class={['surface preview', !visible && 'is-hidden']} aria-hidden={!visible}>
  <form class="preview-bar" {onsubmit}>
    <span class="muted">localhost:</span>
    <input class="field port" inputmode="numeric" pattern="[0-9]*" placeholder="5173" aria-label="Port" bind:value={port} />
    <input class="field path" placeholder="/" aria-label="Path" bind:value={path} autocapitalize="off" autocorrect="off" />
    <button class="btn btn-primary" disabled={busy}>{busy ? 'Opening…' : 'Open'}</button>
  </form>
  {#if src}
    <div class="preview-tools">
      <button type="button" class="icon-btn" aria-label="Reload preview" onclick={() => (prev?.launch ? open(prev.targetPort, path || '/') : nonce++)}>
        <Icon name="refresh" />
      </button>
      <button type="button" class="icon-btn" aria-label="Close preview" onclick={close}>
        <Icon name="close" />
      </button>
      <div class="preview-zoom" role="group" aria-label="Preview zoom">
        <button type="button" class="icon-btn" aria-label="Zoom out preview" title="Zoom out" disabled={zoom <= 50} onclick={() => (zoom -= 25)}>
          <Icon name="minus" />
        </button>
        <button type="button" class="preview-zoom-reset" aria-label="Reset preview zoom to 100%" title="Reset zoom" onclick={() => (zoom = 100)}>{zoom}%</button>
        <button type="button" class="icon-btn" aria-label="Zoom in preview" title="Zoom in" disabled={zoom >= 200} onclick={() => (zoom += 25)}>
          <Icon name="plus" />
        </button>
      </div>
    </div>
  {/if}
  {#if error}<p class="error padded" role="alert">{error}</p>{/if}
  {#if src}
    <div class="preview-viewport" style:--preview-scale={zoom / 100} style:--preview-size={`${10000 / zoom}%`}>
      {#key nonce}
        <iframe class="preview-frame" {src} title="Preview of localhost:{port}" sandbox="allow-scripts allow-forms allow-same-origin allow-popups allow-modals allow-downloads"></iframe>
      {/key}
    </div>
  {:else if !error}
    <p class="hint padded">Show a web server running on {theHost()}. Tap a localhost link in the terminal, or enter its port. The server itself stays private to {theHost()}.</p>
  {/if}
</div>
