<!--
  A PDF in the Files surface, drawn with pdf.js. Pages fit the pane's width (times the zoom) and are
  drawn as they scroll near view; pages far out of view drop their pixels again (iOS limits canvas memory).
-->
<script lang="ts">
  import { tick, untrack } from 'svelte';
  import type { PDFDocumentProxy, RenderTask } from './pdf.ts';

  interface Props {
    url: string;
    name: string;
    zoom: number;
    /** Called once the PDF is open, with its page count and size in bytes. */
    onOpen: (pages: number, size: number) => void;
    onError: (message: string) => void;
  }
  let { url, name, zoom, onOpen, onError }: Props = $props();

  /** Device pixels per canvas, kept under what iOS allows one canvas. */
  const MAX_PIXELS = 12_000_000;

  let doc = $state.raw<PDFDocumentProxy | null>(null);
  let failed = $state(false);
  /** Width over height for each page drawn so far; others take page 1's. */
  let ratios = $state<number[]>([]);
  let width = $state(0);
  let scroller: HTMLElement | undefined = $state();
  const pageWidth = $derived(Math.round(Math.min(Math.max(width - 32, 120), 960) * zoom));

  $effect(() => {
    const u = url;
    let cancelled = false;
    let opened: PDFDocumentProxy | null = null;
    (async () => {
      try {
        const res = await fetch(u, { credentials: 'same-origin' }).catch(() => null);
        if (!res?.ok) throw new Error((await res?.json().catch(() => null))?.message || 'Couldn’t load this PDF.');
        // pdf.js takes over the buffer, so the size is read first.
        const data = new Uint8Array(await res.arrayBuffer());
        const size = data.byteLength;
        const { openPdf } = await import('./pdf.ts');
        opened = await openPdf(data);
        if (cancelled) return void opened.loadingTask.destroy();
        const first = (await opened.getPage(1)).getViewport({ scale: 1 });
        ratios = Array(opened.numPages).fill(first.width / first.height);
        doc = opened;
        onOpen(opened.numPages, size);
      } catch (e) {
        if (cancelled) return;
        failed = true;
        onError(e instanceof Error ? e.message : 'Couldn’t open this PDF.');
      }
    })();
    return () => {
      cancelled = true;
      opened?.loadingTask.destroy();
    };
  });

  interface Page {
    el: HTMLElement;
    task: RenderTask | null;
    /** The width it's drawn (or being drawn) at; 0 when it isn't. */
    width: number;
  }
  const pages = new Map<number, Page>();
  const near = new Set<number>();
  let observer: IntersectionObserver | null = null;

  async function draw(n: number) {
    const p = pages.get(n);
    const target = pageWidth;
    if (!p || !doc || p.width === target) return;
    p.task?.cancel();
    p.task = null;
    p.width = target;
    const page = await doc.getPage(n);
    if (pages.get(n) !== p || p.width !== target) return;
    const base = page.getViewport({ scale: 1 });
    if (ratios[n - 1] !== base.width / base.height) ratios[n - 1] = base.width / base.height;
    const cssScale = target / base.width;
    const area = target * (target * (base.height / base.width));
    const px = Math.min(devicePixelRatio || 1, 3, Math.sqrt(MAX_PIXELS / area));
    const viewport = page.getViewport({ scale: cssScale * px });
    // Drawn off to the side and swapped in when done, so a page being redrawn (zoom, resize) stays visible.
    const canvas = document.createElement('canvas');
    canvas.width = Math.floor(viewport.width);
    canvas.height = Math.floor(viewport.height);
    const task = page.render({ canvas, viewport });
    p.task = task;
    try {
      await task.promise;
    } catch {
      if (p.task === task) (p.task = null), (p.width = 0);
      return;
    }
    if (p.task !== task) return;
    p.task = null;
    release(p.el);
    p.el.append(canvas);
  }

  function release(el: HTMLElement) {
    for (const c of el.querySelectorAll('canvas')) (c.width = 0), (c.height = 0), c.remove();
  }

  function forget(n: number) {
    const p = pages.get(n);
    if (!p) return;
    p.task?.cancel();
    p.task = null;
    p.width = 0;
    release(p.el);
  }

  /** Each page: drawn when within a screen of the view, cleared when it leaves. */
  function page(n: number) {
    return (el: HTMLElement) => {
      observer ??= new IntersectionObserver(
        (entries) => {
          for (const e of entries) {
            const at = Number((e.target as HTMLElement).dataset.page);
            if (e.isIntersecting) near.add(at), draw(at);
            else near.delete(at), forget(at);
          }
        },
        { root: el.closest('.fv-pdf'), rootMargin: '100% 0px' },
      );
      pages.set(n, { el, task: null, width: 0 });
      observer.observe(el);
      return () => {
        observer?.unobserve(el);
        forget(n);
        pages.delete(n);
        near.delete(n);
      };
    };
  }

  // Redraws the pages in view at the new width.
  $effect(() => {
    void pageWidth;
    untrack(() => near.forEach(draw));
  });

  // Keeps the same spot in view when zooming.
  let lastZoom = untrack(() => zoom);
  $effect.pre(() => {
    const z = zoom;
    untrack(() => {
      const s = scroller;
      if (!s || z === lastZoom) return;
      lastZoom = z;
      const at = (s.scrollTop + s.clientHeight / 2) / s.scrollHeight;
      tick().then(() => {
        s.scrollTop = at * s.scrollHeight - s.clientHeight / 2;
        s.scrollLeft = (s.scrollWidth - s.clientWidth) / 2;
      });
    });
  });

  $effect(() => () => observer?.disconnect());
</script>

<!-- svelte-ignore a11y_no_noninteractive_tabindex -->
<div class="fv-pdf" bind:this={scroller} bind:clientWidth={width} tabindex="0" role="region" aria-label={`Pages of ${name}`}>
  {#if doc}
    <div class="fv-pdf-pages">
      {#each ratios as ratio, i (i)}
        <div class="fv-pdf-page" role="img" aria-label={`Page ${i + 1} of ${ratios.length}`} data-page={i + 1} style:width="{pageWidth}px" style:aspect-ratio={ratio} {@attach page(i + 1)}></div>
      {/each}
    </div>
  {:else if !failed}
    <p class="fv-loading muted">Opening…</p>
  {/if}
</div>

