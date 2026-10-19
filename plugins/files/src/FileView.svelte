<!--
  One file from the Files surface: highlighted text with line numbers (they stay in view while the
  code scrolls sideways and are never copied), images as images, binary files described.
  Lines are picked for a prompt by tapping their numbers (a second tap makes a range; Shift-click
  extends one) or by selecting text in them; lines already in the prompt are marked.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, Empty, Icon, type PluginContext, type Stream } from '@briardr/sdk';
  import { filesView, formatSize, IMAGE_EXT, languageLabel, languageOf, type FileText } from './files.ts';
  import FileIcon from './FileIcon.svelte';
  import { addSnippet, promptOf } from './prompt.ts';

  interface Props {
    stream: Stream;
    briardr: PluginContext;
    path: string;
    compact: boolean;
    /** Changes when the surface refreshes, to reload this file too. */
    revision: number;
    treeOpen: boolean;
    onBack: () => void;
    onToggleTree: () => void;
    /** Opens the prompt to write the question. */
    onAsk: () => void;
  }
  let { stream, briardr, path, compact, revision, treeOpen, onBack, onToggleTree, onAsk }: Props = $props();
  const toast = (text: string) => briardr.toast(text);

  let file = $state.raw<FileText | null>(null);
  let rendered = $state.raw<{ lines: string[]; highlighted: boolean } | null>(null);
  let error = $state<string | null>(null);
  let scroller: HTMLElement | undefined = $state();
  let code: HTMLElement | undefined = $state();
  /** Picked lines (1-based, inclusive); `fromText` when they came from a text selection. */
  let sel = $state<{ anchor: number; from: number; to: number; fromText?: boolean } | null>(null);

  const name = $derived(path.split('/').pop()!);
  const dir = $derived(path.includes('/') ? path.slice(0, path.lastIndexOf('/')) : '');
  const language = $derived(languageOf(path));
  const image = $derived(IMAGE_EXT.test(path));
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
    }
    error = null;
    if (untrack(() => image)) return;
    untrack(() => load(p, same));
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
      if (p === path) error = e instanceof ApiError ? e.message : 'Couldn’t open this file.';
    }
  }

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
    {#if !image && file?.text != null}
      <button class={['icon-btn', wrap && 'is-on']} aria-label="Wrap lines" aria-pressed={wrap} title="Wrap lines" onclick={() => (filesView.value = { ...filesView.value, wrap: !wrap })}><Icon name="wrap" /></button>
      <button class="icon-btn" aria-label="Copy file" title="Copy file" onclick={copy}><Icon name="copy" /></button>
    {/if}
  </div>
  {#if file || image}
    <div class="fv-meta">
      {#if image}<span>Image</span>{:else if file}
        <span>{file.binary ? 'Binary' : languageLabel(language)}</span>
        {#if !file.binary}<span>{lineCount.toLocaleString()} {lineCount === 1 ? 'line' : 'lines'}</span>{/if}
        <span>{formatSize(file.size)}</span>
        {#if rendered && !rendered.highlighted && language && language !== 'plaintext'}<span>Large file: no highlighting</span>{/if}
      {/if}
    </div>
  {/if}
  {#if error}<p class="error fv-error" role="alert">{error}</p>{/if}
  {#if image}
    <div class="fv-image">
      <img src={`/api/streams/${encodeURIComponent(sid)}/docs/asset?path=${encodeURIComponent(path)}`} alt={name} onerror={() => (error = 'Couldn’t load this image.')} />
    </div>
  {:else if file?.binary}
    <Empty title="Binary file">{name} isn’t text, so Files can’t show it.</Empty>
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
