<!--
  Files: the stream's folder as a tree, and any text file in it, read-only with syntax highlighting.
  Folders load one level at a time when opened. Side by side on iPad and Mac (the divider drags);
  on iPhone the tree and the file take turns. Changed files (uncommitted, per git) carry a dot.
  Lines picked in files gather in a prompt (a bar along the bottom), sent to the terminal with a question.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, AsOf, Empty, Icon, PullToRefresh, type StreamTabProps } from '@briardr/sdk';
  import { filesView, type DirList, type FileEntry, type FileSearch } from './files.ts';
  import FileRow from './FileRow.svelte';
  import FileView from './FileView.svelte';
  import { addFileReference, promptOf } from './prompt.ts';
  import PromptSheet from './PromptSheet.svelte';

  let { stream, visible, compact, briardr }: StreamTabProps = $props();
  const toast = (text: string) => briardr.toast(text);

  let dirs = $state.raw<Record<string, DirList>>({});
  let expanded = $state.raw<Set<string>>(new Set());
  let loading = $state.raw<Set<string>>(new Set());
  let error = $state<string | null>(null);
  let selected = $state<string | null>(null);
  let treeOpen = $state(true);
  /** Bumped to make the open file reload with the tree. */
  let revision = $state(0);
  let tree: HTMLElement | undefined = $state();
  let query = $state('');
  let search = $state.raw<FileSearch | null>(null);
  let searchError = $state<string | null>(null);
  let searching = $state(false);
  let actionBusy = $state(false);
  let searchInput: HTMLInputElement | undefined = $state();
  const searchQuery = $derived(query.trim());

  const base = $derived(`/api/streams/${encodeURIComponent(stream.id)}/files`);

  async function loadDir(path: string) {
    loading = new Set(loading).add(path);
    try {
      const list = await briardr.core<DirList>('GET', `${base}?path=${encodeURIComponent(path)}`, undefined, { timeout: 20_000 });
      dirs = { ...dirs, [path]: list };
      if (!path) error = null;
    } catch (e) {
      const msg = e instanceof ApiError ? e.message : 'Couldn’t list this folder.';
      if (path) {
        // A folder that vanished closes; the rest of the tree stays.
        const next = new Set(expanded);
        next.delete(path);
        expanded = next;
      }
      error = msg;
    } finally {
      const next = new Set(loading);
      next.delete(path);
      loading = next;
    }
  }

  /** Reloads the root and every open folder, and the open file. */
  function refresh() {
    revision++;
    const open = ['', ...expanded];
    dirs = Object.fromEntries(Object.entries(dirs).filter(([p]) => open.includes(p)));
    for (const p of open) void loadDir(p);
  }

  $effect(() => {
    if (visible && !untrack(() => dirs[''] || loading.has(''))) untrack(() => loadDir(''));
  });

  $effect(() => {
    const q = searchQuery;
    const url = base;
    void revision;
    if (!visible || !q) {
      search = null;
      searchError = null;
      searching = false;
      return;
    }
    search = null;
    searchError = null;
    searching = true;
    const ctrl = new AbortController();
    let active = true;
    const timer = setTimeout(async () => {
      try {
        const result = await briardr.core<FileSearch>('GET', `${url}/search?q=${encodeURIComponent(q)}`, undefined, { signal: ctrl.signal, timeout: 20_000 });
        if (active) search = result;
      } catch (e) {
        if (active) searchError = e instanceof ApiError ? e.message : 'Couldn’t search files.';
      } finally {
        if (active) searching = false;
      }
    }, 250);
    return () => {
      active = false;
      clearTimeout(timer);
      ctrl.abort();
    };
  });

  function clearSearch() {
    query = '';
    searchInput?.focus();
  }

  function toggle(e: FileEntry) {
    const next = new Set(expanded);
    if (next.has(e.path)) next.delete(e.path);
    else {
      next.add(e.path);
      if (!dirs[e.path] && !loading.has(e.path)) void loadDir(e.path);
    }
    expanded = next;
  }

  function collapseAll() {
    expanded = new Set();
  }

  type Row = { kind: 'entry'; entry: FileEntry; depth: number } | { kind: 'loading' | 'more'; key: string; depth: number; text?: string };
  const rows = $derived.by(() => {
    if (searchQuery) return (search?.entries ?? []).map((entry): Row => ({ kind: 'entry', entry, depth: 0 }));
    const out: Row[] = [];
    const walk = (path: string, depth: number) => {
      const list = dirs[path];
      if (!list) {
        if (loading.has(path)) out.push({ kind: 'loading', key: `loading:${path}`, depth });
        return;
      }
      for (const entry of list.entries) {
        out.push({ kind: 'entry', entry, depth });
        if (entry.dir && expanded.has(entry.path)) walk(entry.path, depth + 1);
      }
      if (list.truncated) out.push({ kind: 'more', key: `more:${path}`, depth, text: `${list.total - list.entries.length} more not shown` });
    };
    walk('', 0);
    return out;
  });
  const root = $derived(dirs['']);

  function open(e: FileEntry) {
    if (e.dir) toggle(e);
    else selected = e.path;
  }

  function entryMenu(e: FileEntry, at: { x: number; y: number } | HTMLElement) {
    briardr.menu([
      { label: 'Send to terminal…', icon: 'terminal', disabled: actionBusy, onSelect: () => void askAbout(e) },
      { label: 'Delete…', icon: 'trash', destructive: true, disabled: actionBusy, onSelect: () => void deleteEntry(e) },
    ], at, e.path);
  }

  function askAbout(e: FileEntry) {
    if (actionBusy) return;
    addFileReference(stream.id, e.path, e.dir);
    asking = true;
  }

  async function deleteEntry(e: FileEntry) {
    if (actionBusy) return;
    const description = e.link ? 'This removes the link. Its target is kept.' : e.dir ? 'This permanently deletes the folder and everything inside it.' : 'This permanently deletes the file.';
    if (!(await briardr.confirm({ title: `Delete “${e.name}”?`, message: `${e.path}\n\n${description}`, confirm: 'Delete', destructive: true }))) return;
    actionBusy = true;
    try {
      await briardr.core('DELETE', `${base}?path=${encodeURIComponent(e.path)}`, undefined, { timeout: 20_000 });
      const beneath = (p: string) => p === e.path || p.startsWith(`${e.path}/`);
      if (selected && beneath(selected)) selected = null;
      expanded = new Set([...expanded].filter((p) => !beneath(p)));
      dirs = Object.fromEntries(Object.entries(dirs).filter(([p]) => !beneath(p)));
      refresh();
      toast(`Deleted ${e.name}`);
    } catch (err) {
      toast(err instanceof ApiError ? err.message : 'Couldn’t delete this entry.');
    } finally { actionBusy = false; }
  }

  // Arrow keys move through the tree; Right opens a folder, Left closes it or goes to its parent.
  function onkeydown(ev: KeyboardEvent) {
    const items = [...(tree?.querySelectorAll<HTMLButtonElement>('.ft-row') ?? [])];
    const i = items.indexOf(document.activeElement as HTMLButtonElement);
    if (i < 0) return;
    const entry = (rows.filter((r) => r.kind === 'entry') as Extract<Row, { kind: 'entry' }>[])[i];
    const focus = (j: number) => items[Math.max(0, Math.min(items.length - 1, j))]?.focus();
    if (ev.key === 'ArrowDown') focus(i + 1);
    else if (ev.key === 'ArrowUp') focus(i - 1);
    else if (ev.key === 'Home') focus(0);
    else if (ev.key === 'End') focus(items.length - 1);
    else if (ev.key === 'ArrowRight' && !searchQuery && entry?.entry.dir) {
      if (!expanded.has(entry.entry.path)) toggle(entry.entry);
      else focus(i + 1);
    } else if (ev.key === 'ArrowLeft' && !searchQuery && entry) {
      if (entry.entry.dir && expanded.has(entry.entry.path)) toggle(entry.entry);
      else {
        const parent = entry.entry.path.includes('/') ? entry.entry.path.slice(0, entry.entry.path.lastIndexOf('/')) : null;
        const j = parent === null ? -1 : (rows.filter((r) => r.kind === 'entry') as Extract<Row, { kind: 'entry' }>[]).findIndex((r) => r.entry.path === parent);
        if (j >= 0) focus(j);
      }
    } else return;
    ev.preventDefault();
  }

  // Dragging the divider resizes the tree (iPad and Mac); remembered on this device.
  function startResize(ev: PointerEvent) {
    const handle = ev.currentTarget as HTMLElement;
    const host = handle.parentElement!;
    handle.setPointerCapture(ev.pointerId);
    const left = host.getBoundingClientRect().left;
    const move = (e: PointerEvent) => {
      const max = Math.max(220, host.clientWidth * 0.6);
      filesView.value = { ...filesView.value, treeWidth: Math.round(Math.max(180, Math.min(max, e.clientX - left))) };
    };
    const up = () => {
      handle.removeEventListener('pointermove', move);
      handle.removeEventListener('pointerup', up);
      handle.removeEventListener('pointercancel', up);
    };
    handle.addEventListener('pointermove', move);
    handle.addEventListener('pointerup', up);
    handle.addEventListener('pointercancel', up);
  }
  function resizeKey(ev: KeyboardEvent) {
    const step = ev.key === 'ArrowLeft' ? -24 : ev.key === 'ArrowRight' ? 24 : 0;
    if (!step) return;
    ev.preventDefault();
    filesView.value = { ...filesView.value, treeWidth: Math.max(180, Math.min(640, filesView.value.treeWidth + step)) };
  }

  let asking = $state(false);
  const prompt = $derived(promptOf(stream.id));
  const promptFiles = $derived(new Set(prompt.snippets.map((x) => x.path)).size);
  const promptItems = $derived(prompt.snippets.length + (prompt.references?.length ?? 0));
  const hasReferences = $derived(!!prompt.references?.length);
  const plural = (n: number, one: string, many: string) => `${n} ${n === 1 ? one : many}`;

  const ptr = new PullToRefresh(refresh);
  const showTree = $derived(compact ? !selected : treeOpen);
  const showFile = $derived(!compact || !!selected);
</script>

{#if visible}
  <div class={['surface', 'files', compact ? 'is-stacked' : 'is-split', (promptItems || prompt.question) && 'has-prompt']} style:--tree-w="{filesView.value.treeWidth}px">
    <div class="files-panes">
      {#if showTree}
        <section class="ft-pane" aria-label="Folder" {...compact ? ptr.handlers : {}}>
          {#if ptr.pull > 0}<div class="ptr" style:height="{ptr.pull}px">{ptr.pull > 64 ? 'Release to refresh' : 'Pull to refresh'}</div>{/if}
          <div class="ft-toolbar">
            <div class="surface-head ft-head">
              <div class="surface-title">
                <strong>Files</strong>
                <span class="muted ft-folder">{stream.folder}</span>
                {#if compact}<AsOf at={root?.asOf} />{/if}
              </div>
              {#if expanded.size && !searchQuery}
                <button class="icon-btn" aria-label="Collapse all folders" title="Collapse all" onclick={collapseAll}>
                  <Icon name="collapse" />
                </button>
              {/if}
              <button class="icon-btn" aria-label="Refresh files" title="Refresh" onclick={refresh}>
                <Icon name="refresh" />
              </button>
            </div>
            <div class="search ft-search" role="search">
              <Icon name="search" size={17} />
              <input bind:this={searchInput} bind:value={query} type="search" aria-label="Search files by filename" placeholder="Search filenames…" autocomplete="off" autocapitalize="off" spellcheck="false" onkeydown={(ev) => {
                if (ev.key === 'Escape') { ev.preventDefault(); clearSearch(); }
                else if (ev.key === 'ArrowDown') { ev.preventDefault(); tree?.querySelector<HTMLButtonElement>('.ft-row')?.focus(); }
              }} />
              {#if query}<button class="icon-btn" aria-label="Clear filename search" title="Clear search" onclick={clearSearch}><Icon name="close" size={16} /></button>{/if}
            </div>
          </div>
          {#if searchQuery}
            <p class="ft-search-status muted" role="status">
              {#if searching}Searching…{:else if search}{search.entries.length} {search.entries.length === 1 ? 'file' : 'files'} found{/if}
            </p>
            {#if searchError}<p class="error" role="alert">{searchError}</p>{/if}
            {#if search && !search.entries.length && !search.truncated}<Empty title="No matching files">Try a different filename.</Empty>{/if}
          {/if}
          {#if error}<p class="error" role="alert">{error}</p>{/if}
          {#if !searchQuery && root && !root.entries.length}<Empty title="Empty folder">There are no files in this stream’s folder.</Empty>{/if}
          <!-- svelte-ignore a11y_no_noninteractive_element_interactions -->
          <ul class="ft" bind:this={tree} {onkeydown}>
            {#each rows as r (r.kind === 'entry' ? r.entry.path : r.key)}
              {#if r.kind === 'entry'}
                {@const e = r.entry}
                <li>
                  <FileRow entry={e} depth={r.depth} selected={selected === e.path} expanded={expanded.has(e.path)} searching={!!searchQuery} onOpen={() => open(e)} onMenu={(at) => entryMenu(e, at)} />
                </li>
              {:else}
                <li class="ft-note" style:--depth={r.depth}>{r.kind === 'loading' ? 'Loading…' : r.text}</li>
              {/if}
            {/each}
          </ul>
          {#if searchQuery && search?.truncated}<p class="ft-search-status muted">Search limited. Try a more specific filename.</p>{/if}
        </section>
        {#if !compact}
          <!-- svelte-ignore a11y_no_noninteractive_tabindex, a11y_no_noninteractive_element_interactions -->
          <div class="ft-resize" role="separator" aria-orientation="vertical" aria-label="Resize folder pane" aria-valuenow={filesView.value.treeWidth} tabindex="0" onpointerdown={startResize} onkeydown={resizeKey}></div>
        {/if}
      {/if}
      {#if showFile}
        <section class="fv-pane" aria-label="File">
          {#if selected}
            <FileView {stream} {briardr} path={selected} {compact} {revision} {treeOpen} onBack={() => (selected = null)} onToggleTree={() => (treeOpen = !treeOpen)} onAsk={() => (asking = true)} />
          {:else}
            <div class="fv-empty">
              {#if !treeOpen}
                <button class="icon-btn fv-tree-btn" aria-label="Show folder" title="Show folder" onclick={() => (treeOpen = true)}><Icon name="sidebar" /></button>
              {/if}
              <Empty title="No file open">Choose a file in the folder to read it here.</Empty>
            </div>
          {/if}
        </section>
      {/if}
    </div>
    {#if promptItems || prompt.question}
      <div class="files-prompt" role="region" aria-label="Prompt">
        <Icon name="notes" size={18} />
        <span class="files-prompt-label">
          {hasReferences ? `${plural(promptItems, 'item', 'items')} in prompt` : prompt.snippets.length ? `${plural(prompt.snippets.length, 'snippet', 'snippets')} from ${plural(promptFiles, 'file', 'files')}` : 'Question drafted'}
        </span>
        <button class="btn btn-small btn-primary" onclick={() => (asking = true)}>Ask…</button>
      </div>
    {/if}
  </div>
  {#if asking}
    <PromptSheet {stream} {briardr} onClose={() => (asking = false)} onSent={() => {
      asking = false;
      briardr.openTab('terminal');
    }} />
  {/if}
{/if}
