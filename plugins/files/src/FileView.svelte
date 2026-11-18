<!--
  One file from the Files surface: highlighted text with line numbers (they stay in view while the
  code scrolls sideways and are never copied), images as images, PDFs as pages (zoomable), binary
  files described.
  Lines are picked for a prompt by tapping their numbers (a second tap makes a range; Shift-click
  extends one) or by selecting text in them; lines already in the prompt are marked. A search result
  opens the file at its line, picked.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { Empty, errorMessage, Icon, type PluginContext, type Stream } from '@briardr/sdk';
  import { filesView, formatSize, HTML_EXT, IMAGE_EXT, languageLabel, languageOf, PDF_EXT, type FileText } from './files.ts';
  import FileIcon from './FileIcon.svelte';
  import PdfView from './PdfView.svelte';
  import { addSnippet, promptOf } from './prompt.ts';

  interface Props {
    stream: Stream;
    briardr: PluginContext;
    path: string;
    compact: boolean;
    /** Changes when the surface refreshes, to reload this file too. */
    revision: number;
    treeOpen: boolean;
    /** A line to show and pick (a search result); a new `n` goes there again. */
    goTo?: { line: number; n: number } | null;
    onBack: () => void;
    onToggleTree: () => void;
    /** Opens the prompt to write the question. */
    onAsk: () => void;
  }
  let { stream, briardr, path, compact, revision, treeOpen, goTo = null, onBack, onToggleTree, onAsk }: Props = $props();
  /** PDF zoom steps, times the pane's width. */
  const ZOOMS = [0.5, 0.75, 1, 1.25, 1.5, 2, 3];
  const toast = (text: string) => briardr.toast(text);

  let file = $state.raw<FileText | null>(null);
  let rendered = $state.raw<{ lines: string[]; highlighted: boolean } | null>(null);
  let error = $state<string | null>(null);
  let scroller: HTMLElement | undefined = $state();
  let code: HTMLElement | undefined = $state();
  let pdfInfo = $state<{ pages: number; size: number } | null>(null);
  let zoomAt = $state(ZOOMS.indexOf(1));
  /** Picked lines (1-based, inclusive); `fromText` when they came from a text selection. */
  let sel = $state<{ anchor: number; from: number; to: number; fromText?: boolean } | null>(null);

  const name = $derived(path.split('/').pop()!);
  const dir = $derived(path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
  const language = $derived(languageOf(path));
  const image = $derived(IMAGE_EXT.test(path));
  const pdf = $derived(PDF_EXT.test(path));
  const htmlPage = $derived(HTML_EXT.test(path));
  /** An HTML page shows as the page it describes until the source is asked for. */
  let showSource = $state(false);
  /** The link the server shows this stream's folder as pages through (server/src/site.rs). */
  let site = $state<{ sid: string; token: string } | null>(null);
  const siteToken = $derived(site?.sid === stream.id ? site.token : null);
  const previewing = $derived(htmlPage && !showSource && file?.text != null && siteToken != null);
  const pageUrl = $derived(`/site/${siteToken}/${path.split('/').map(encodeURIComponent).join('/')}`);
  const asset = $derived(`/api/streams/${encodeURIComponent(stream.id)}/docs/asset?path=${encodeURIComponent(path)}`);
  const sid = $derived(stream.id);
  const wrap = $derived(filesView.value.wrap ?? compact);

  $effect(() => {
    const p = path;
    void revision;
    const same = untrack(() => file?.path === p);
    if (!same) {
      file = null;
      rendered = null;
      sel = null;
      pdfInfo = null;
      showSource = false;
      zoomAt = ZOOMS.indexOf(1);
    }
    error = null;
    if (untrack(() => image || pdf)) return;
    untrack(() => load(p, same));
  });

  $effect(() => {
    if (!htmlPage || siteToken) return;
    const id = stream.id;
    briardr.core<{ token: string }>('POST', `/api/streams/${encodeURIComponent(id)}/files/site`).then(
      (r) => (site = { sid: id, token: r.token }),
      () => (error = 'Couldn’t preview this page.'),
    );
  });

  async function load(p: string, keepScroll: boolean) {
    const top = keepScroll ? scroller?.scrollTop : 0;
    try {
      const f = await briardr.core<FileText>('GET', `/api/streams/${encodeURIComponent(sid)}/files/file?path=${encodeURIComponent(p)}`, undefined, { timeout: 20_000 });
      if (p !== path) return;
      const lines = f.text === null ? null : (await import('./highlight.ts')).highlightLines(f.text, languageOf(p));
      if (p !== path) return;
      file = f;
      rendered = lines;
      if (!keepScroll) requestAnimationFrame(() => scroller?.scrollTo(0, 0));
      else if (top) requestAnimationFrame(() => scroller && (scroller.scrollTop = top));
    } catch (e) {
      if (p === path) error = errorMessage(e, 'Couldn’t open this file.');
    }
  }

  // A search result's line, once the file's lines are on the page (and again for another line of it).
  $effect(() => {
    const target = goTo;
    void html;
    const line = target && code?.children[target.line - 1];
    if (!target || !line) return;
    untrack(() => (sel = { anchor: target.line, from: target.line, to: target.line }));
    requestAnimationFrame(() => line.scrollIntoView({ block: 'center' }));
  });

  // One string for the whole file: thousands of lines as separate components would be slow to mount.
  const html = $derived(rendered ? rendered.lines.map((l) => `<span class="cl"><span class="lc">${l}</span></span>`).join('') : '');
  const lineCount = $derived(rendered?.lines.length ?? 0);

  function copy() {
    if (file?.text == null) return;
    navigator.clipboard.writeText(file.text).then(
      () => toast('Copied'),
      () => toast('Couldn’t copy'),
    );
  }
  const lineOf = (el: Element | null | undefined) => {
    const line = el?.closest('.cl');
    return line && code?.contains(line) ? Array.prototype.indexOf.call(code.children, line) + 1 : 0;
  };

  // A click on a line's number. The numbers stay at the left edge while the code scrolls sideways.
  function onCodeClick(ev: MouseEvent) {
    const n = lineOf(ev.target as Element);
    const line = n && code!.children[n - 1];
    const lc = line && line.querySelector('.lc');
    if (!lc || !scroller) return;
    const box = line.getBoundingClientRect();
    const gutterEnd = Math.max(box.left, scroller.getBoundingClientRect().left) + (lc.getBoundingClientRect().left - box.left);
    if (ev.clientX < gutterEnd) pickLine(n, ev.shiftKey);
  }
  function pickLine(n: number, extend: boolean) {
    if (sel && !sel.fromText && (extend || sel.from === sel.to) && n !== sel.anchor) sel = { anchor: sel.anchor, from: Math.min(sel.anchor, n), to: Math.max(sel.anchor, n) };
    else if (sel && !sel.fromText && sel.from === sel.to && n === sel.from) sel = null;
    else sel = { anchor: n, from: n, to: n };
  }

  // Selecting text in the code picks the lines it touches.
  $effect(() => {
    const onChange = () => {
      const s = document.getSelection();
      if (!s || s.isCollapsed || !s.rangeCount || !code) return;
      const r = s.getRangeAt(0);
      if (!code.contains(r.commonAncestorContainer)) return;
      const at = (node: Node) => lineOf(node instanceof Element ? node : node.parentElement);
      const from = at(r.startContainer) || 1;
      let to = at(r.endContainer) || lineOf(code.lastElementChild);
      // A selection that ends at the very start of a line (a triple-click) doesn't include it.
      if (to > from) {
        const head = document.createRange();
        head.setStart(code.children[to - 1], 0);
        head.setEnd(r.endContainer, r.endOffset);
        if (!head.toString()) to--;
      }
      sel = { anchor: from, from, to, fromText: true };
    };
    document.addEventListener('selectionchange', onChange);
    return () => document.removeEventListener('selectionchange', onChange);
  });

  // Marks picked lines and those already in the prompt. Classes are set on the rendered lines directly:
  // the file is one HTML string.
  const added = $derived(promptOf(sid).snippets.filter((s) => s.path === path));
  $effect(() => {
    void html;
    const s = sel;
    const lines = code?.children;
    if (!lines) return;
    const inPrompt = new Set<number>();
    for (const a of added) for (let i = a.from; i <= a.to; i++) inPrompt.add(i);
    for (let i = 0; i < lines.length; i++) {
      lines[i].classList.toggle('is-sel', !!s && i + 1 >= s.from && i + 1 <= s.to);
      lines[i].classList.toggle('is-added', inPrompt.has(i + 1));
    }
  });

  function addSelection(): boolean {
    if (!sel || file?.text == null) return false;
    addSnippet(sid, path, sel.from, Math.min(sel.to, lineCount), file.text);
    if (sel.fromText) document.getSelection()?.removeAllRanges();
    sel = null;
    return true;
  }

  function copyPath() {
    navigator.clipboard.writeText(path).then(() => toast('Path copied'));
  }
</script>

<div class="file-view">
  <div class="fv-head">
    {#if compact}
      <button class="icon-btn" aria-label="Back to folder" onclick={onBack}><Icon name="back" /></button>
    {:else}
      <button class="icon-btn" aria-label={treeOpen ? 'Hide folder' : 'Show folder'} aria-pressed={treeOpen} title={treeOpen ? 'Hide folder' : 'Show folder'} onclick={onToggleTree}><Icon name="sidebar" /></button>
    {/if}
    <FileIcon {name} size={20} />
    <button class="fv-title" title="Copy path" onclick={copyPath}>
      <strong>{name}</strong>
      {#if dir}<span class="fv-dir">{dir}</span>{/if}
    </button>
    {#if pdf}
      <button class="icon-btn" aria-label="Zoom out" title="Zoom out" disabled={zoomAt === 0} onclick={() => zoomAt--}><Icon name="minus" /></button>
      <button class="icon-btn" aria-label="Zoom in" title="Zoom in" disabled={zoomAt === ZOOMS.length - 1} onclick={() => zoomAt++}><Icon name="plus" /></button>
    {:else if !image && file?.text != null}
      {#if htmlPage}
        <button class={['icon-btn', previewing && 'is-on']} aria-label={previewing ? 'Show source' : 'Preview page'} aria-pressed={previewing} title={previewing ? 'Show source' : 'Preview page'} onclick={() => (showSource = !showSource)}><Icon name={previewing ? 'file' : 'eye'} /></button>
      {/if}
      <button class={['icon-btn', wrap && 'is-on']} aria-label="Wrap lines" aria-pressed={wrap} title="Wrap lines" onclick={() => (filesView.value = { ...filesView.value, wrap: !wrap })}><Icon name="wrap" /></button>
      <button class="icon-btn" aria-label="Copy file" title="Copy file" onclick={copy}><Icon name="copy" /></button>
    {/if}
  </div>
  {#if file || image || pdf}
    <div class="fv-meta">
      {#if image}<span>Image</span>{:else if pdf}
        <span>PDF</span>
        {#if pdfInfo}<span>{pdfInfo.pages.toLocaleString()} {pdfInfo.pages === 1 ? 'page' : 'pages'}</span><span>{formatSize(pdfInfo.size)}</span>{/if}
        {#if ZOOMS[zoomAt] !== 1}<span>{Math.round(ZOOMS[zoomAt] * 100)}%</span>{/if}
      {:else if file}
        <span>{previewing ? 'HTML preview' : file.binary ? 'Binary' : languageLabel(language)}</span>
        {#if !file.binary}<span>{lineCount.toLocaleString()} {lineCount === 1 ? 'line' : 'lines'}</span>{/if}
        <span>{formatSize(file.size)}</span>
        {#if rendered && !rendered.highlighted && language && language !== 'plaintext'}<span>Large file: no highlighting</span>{/if}
      {/if}
    </div>
  {/if}
  {#if error}<p class="error fv-error" role="alert">{error}</p>{/if}
  {#if image}
    <div class="fv-image">
      <img src={asset} alt={name} onerror={() => (error = 'Couldn’t load this image.')} />
    </div>
  {:else if pdf}
    {#key path}
      <PdfView url={asset} {name} zoom={ZOOMS[zoomAt]} onOpen={(pages, size) => (pdfInfo = { pages, size })} onError={(m) => (error = m)} />
    {/key}
  {:else if file?.binary}
    <Empty title="Binary file">{name} isn’t text, so Files can’t show it.</Empty>
  {:else if previewing && file}
    <!-- Scripts run, but the page has no origin of its own, so it can't act as Briardr; the server limits what it can reach. -->
    {#key revision}
      <iframe class="fv-html" title={`Preview of ${name}`} sandbox="allow-scripts allow-popups allow-popups-to-escape-sandbox" referrerpolicy="no-referrer" src={pageUrl}></iframe>
    {/key}
  {:else if rendered}
    <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
    <div
      class="fv-scroll"
      bind:this={scroller}
      tabindex="0"
      role="region"
      aria-label={`Contents of ${name}`}
      onkeydown={(e) => e.key === 'Escape' && sel && (e.stopPropagation(), (sel = null))}
      onpointerdown={() => sel?.fromText && (sel = null)}
    >
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <pre bind:this={code} class={['code', 'hljs', wrap ? 'is-wrap' : 'is-nowrap']} style:--digits={String(lineCount).length} style:font-size="{briardr.fontSize()}px" onclick={onCodeClick}>{@html html}</pre>
    </div>
    {#if sel}
      <div class="fv-selbar" role="toolbar" aria-label="Selected lines">
        <span class="fv-sel-label">
          {sel.from === sel.to ? `Line ${sel.from}` : `Lines ${sel.from}–${sel.to}`}
          {#if !sel.fromText && sel.from === sel.to}<small>Tap another number for a range</small>{/if}
        </span>
        <button class="btn btn-small" aria-label="Add to prompt" onclick={addSelection}>Add</button>
        <button class="btn btn-small btn-primary" onclick={() => addSelection() && onAsk()}>Add & ask…</button>
        <button class="icon-btn" aria-label="Clear selection" title="Clear selection" onclick={() => (sel = null)}><Icon name="close" /></button>
      </div>
    {/if}
  {:else if !error}
    <p class="fv-loading muted">Opening…</p>
  {/if}
</div>

<style>
  .file-view { flex: 1; min-height: 0; display: flex; flex-direction: column; }
  .fv-head { flex: none; display: flex; align-items: center; gap: 2px; min-height: 50px; padding: 3px 8px; border-bottom: 1px solid var(--line-default); }
  /* In full screen the way back floats in the top right corner: the head ends before it. */
  :global(.is-full .surfaces) .fv-head { padding-right: var(--full-clear); }
  .fv-title { flex: 1; min-width: 0; display: flex; flex-direction: column; align-items: flex-start; gap: 1px; border: 0; background: none; color: inherit; font: inherit; text-align: left; padding: 3px 6px; border-radius: 8px; cursor: pointer; }
  .fv-title:hover { background: var(--bg-sunken); }
  .fv-title strong, .fv-dir { max-width: 100%; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .fv-dir { font-size: 0.78rem; color: var(--text-secondary); font-family: var(--font-mono); }
  .fv-meta { flex: none; display: flex; flex-wrap: wrap; gap: 2px 14px; padding: 5px 16px; font-size: 0.78rem; color: var(--text-secondary); border-bottom: 1px solid var(--line-default); background: var(--bg-sunken); }
  .fv-error { margin: 12px 16px; }
  .fv-loading { padding: 16px; margin: 0; }
  .fv-scroll { flex: 1; min-height: 0; overflow: auto; background: var(--bg-surface); overscroll-behavior: contain; }
  .fv-scroll:focus-visible { outline: 2px solid var(--ink-default); outline-offset: -2px; }
  .fv-image {
    flex: 1; min-height: 0; overflow: auto; display: flex; align-items: center; justify-content: center; padding: 16px;
    background: repeating-conic-gradient(var(--bg-sunken) 0% 25%, var(--bg-surface) 0% 50%) 50% / 20px 20px;
  }
  .fv-image img { max-width: 100%; height: auto; border-radius: 4px; box-shadow: var(--shadow-card); }
  .fv-html { flex: 1; min-height: 0; width: 100%; border: 0; background: #fff; color-scheme: light; }
  /* The file's lines, numbered in a gutter that stays put when they scroll sideways. Line numbers pick
     lines for a prompt; picked lines are tinted, lines already in it carry a bar. */
  .code { margin: 0; padding: 8px 0 calc(28px + env(safe-area-inset-bottom)); font-family: var(--font-mono); line-height: 1.55; tab-size: 4; color: var(--text-primary); counter-reset: ln; }
  .code.is-nowrap { width: max-content; min-width: 100%; }
  .code :global(.cl) { display: flex; }
  .code :global(.cl::before) {
    counter-increment: ln; content: counter(ln); flex: none; position: sticky; left: 0; box-sizing: content-box;
    min-width: calc(var(--digits) * 1ch); padding: 0 14px 0 10px; text-align: right; color: var(--text-tertiary); background: var(--bg-surface);
    -webkit-user-select: none; user-select: none; cursor: pointer;
  }
  .code :global(.lc) { flex: 1; min-width: 0; padding-right: 16px; white-space: pre; }
  .code.is-wrap :global(.lc) { white-space: pre-wrap; overflow-wrap: anywhere; }
  @media (hover: hover) {
    .code :global(.cl:hover) { background: color-mix(in srgb, var(--text-primary) 5%, transparent); }
    .code :global(.cl:hover::before) { color: var(--text-primary); }
  }
  .code :global(.cl.is-sel) { background: color-mix(in srgb, var(--ink-default) 14%, transparent); }
  .code :global(.cl.is-sel::before) { color: var(--ink-default); font-weight: 700; background: color-mix(in srgb, var(--ink-default) 14%, var(--bg-surface)); }
  .code :global(.cl.is-added::before) { box-shadow: inset -3px 0 0 color-mix(in srgb, var(--ink-default) 60%, transparent); }
  /* highlight.js's classes in the theme's colours (--hl-*). */
  .code :global(.hljs-doctag), .code :global(.hljs-keyword), .code :global(.hljs-meta .hljs-keyword), .code :global(.hljs-template-tag), .code :global(.hljs-template-variable), .code :global(.hljs-type), .code :global(.hljs-variable.language_) { color: var(--hl-keyword); }
  .code :global(.hljs-title), .code :global(.hljs-title.class_), .code :global(.hljs-title.class_.inherited__), .code :global(.hljs-title.function_) { color: var(--hl-title); }
  .code :global(.hljs-attr), .code :global(.hljs-attribute), .code :global(.hljs-literal), .code :global(.hljs-meta), .code :global(.hljs-number), .code :global(.hljs-operator), .code :global(.hljs-selector-attr), .code :global(.hljs-selector-class), .code :global(.hljs-selector-id), .code :global(.hljs-variable) { color: var(--hl-const); }
  .code :global(.hljs-meta .hljs-string), .code :global(.hljs-regexp), .code :global(.hljs-string) { color: var(--hl-string); }
  .code :global(.hljs-built_in), .code :global(.hljs-symbol) { color: var(--hl-builtin); }
  .code :global(.hljs-code), .code :global(.hljs-comment), .code :global(.hljs-formula) { color: var(--hl-comment); font-style: italic; }
  .code :global(.hljs-name), .code :global(.hljs-quote), .code :global(.hljs-selector-pseudo), .code :global(.hljs-selector-tag) { color: var(--hl-tag); }
  .code :global(.hljs-subst) { color: var(--text-primary); }
  .code :global(.hljs-section) { color: var(--hl-const); font-weight: 700; }
  .code :global(.hljs-bullet) { color: var(--hl-bullet); }
  .code :global(.hljs-emphasis) { font-style: italic; }
  .code :global(.hljs-strong) { font-weight: 700; }
  .code :global(.hljs-addition) { color: var(--diff-add); background: var(--diff-add-bg); }
  .code :global(.hljs-deletion) { color: var(--diff-del); background: var(--diff-del-bg); }
  .fv-selbar {
    flex: none; display: flex; align-items: center; gap: 8px; flex-wrap: wrap;
    padding: 8px 8px max(8px, env(safe-area-inset-bottom)) 16px; border-top: 1px solid var(--line-default); background: var(--bg-sunken);
  }
  /* The prompt's bar is under it then, and has the safe area. */
  :global(.has-prompt) .fv-selbar { padding-bottom: 8px; }
  .fv-sel-label { flex: 1; min-width: 0; display: flex; flex-direction: column; font-weight: 600; font-size: 0.9rem; font-variant-numeric: tabular-nums; }
  .fv-sel-label small { font-weight: 400; font-size: 0.78rem; color: var(--text-secondary); }
</style>
