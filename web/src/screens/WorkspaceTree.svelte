<!--
  Workspaces: workspace → project → stream, including shells and editors. Searchable; expansion is
  remembered; cached rows are dimmed while disconnected. Creating work never changes the Mac's focus.
-->
<script module lang="ts">
  export function shortPath(p: string): string {
    return p.replace(/^\/Users\/[^/]+/, '~');
  }
</script>

<script lang="ts">
  import { theHost } from '../host.ts';
  import { expandedNodes, live, model, stale } from '../store.ts';
  import type { Project, Stream, Workspace } from '../types.ts';
  import { Icon } from '../ui/icons/index.ts';
  import { Empty, openMenu } from '../ui/kit.ts';
  import { peekSheet } from './lazy.svelte.ts';
  import StreamRow from './StreamRow.svelte';
  import NewProject from './workspaces/NewProject.svelte';
  import NewStream from './workspaces/NewStream.svelte';
  import NewWorkspace from './workspaces/NewWorkspace.svelte';

  let { compact = false }: { compact?: boolean } = $props();
  let q = $state('');
  let peek = $state.raw<Stream | null>(null);
  let create = $state.raw<null | { kind: 'workspace' } | { kind: 'project'; ws: Workspace } | { kind: 'stream'; project: Project }>(null);
  const onPeek = (s: Stream) => {
    void peekSheet.load();
    peek = s;
  };

  // Workspaces and projects are expanded unless collapsed explicitly (an absent entry is expanded).
  function toggle(id: string) {
    expandedNodes.value = { ...expandedNodes.value, [id]: expandedNodes.value[id] === false };
  }

  const query = $derived(q.trim().toLowerCase());
  const tree = $derived.by(() => {
    const m = model.value;
    const hit = (...parts: (string | null | undefined)[]) => !query || parts.some((p) => p && p.toLowerCase().includes(query));
    return (m?.workspaces || [])
      .map((ws) => {
        const projects = (m?.projects || [])
          .filter((p) => p.workspaceId === ws.id)
          .map((p) => {
            const streams = (m?.streams || []).filter((s) => s.projectId === p.id && (hit(ws.name, ws.cwd, p.name) || hit(s.name, s.folder, s.cwd, s.tool)));
            return { p, streams, show: hit(ws.name, ws.cwd, p.name) || streams.length > 0 };
          })
          .filter((x) => x.show);
        return { ws, projects, show: hit(ws.name, ws.cwd) || projects.length > 0 };
      })
      .filter((x) => x.show);
  });
</script>

<div class="workspaces">
  <div class="search-row">
    <label class="search">
      <Icon name="search" size={18} />
      <input type="search" placeholder="Search workspaces, projects, streams, folders" aria-label="Search workspaces" bind:value={q} />
    </label>
    {#if !compact}
      <button class="btn" disabled={!live.value} onclick={() => (create = { kind: 'workspace' })}>
        <Icon name="plus" size={18} /> Workspace
      </button>
    {/if}
  </div>
  {#if !model.value}
    <Empty title="Waiting for {theHost()}" />
  {:else if !tree.length}
    {#if query}
      <Empty title="Nothing matches" />
    {:else}
      <Empty title="No workspaces">Create a workspace to start.</Empty>
    {/if}
  {/if}
  <ul class={['tree', stale.value && 'is-dim']} role="tree" aria-label="Workspaces">
    {#each tree as { ws, projects }}
      {@const open = !!query || expandedNodes.value[ws.id] !== false}
      <!-- Workspaces and projects are only expandable; a stream is selected while it is open. -->
      <li role="treeitem" aria-expanded={open} aria-selected={false} class="tree-ws">
        <div class="tree-row lvl-0">
          <button class="tree-toggle" onclick={() => toggle(ws.id)} aria-label="{open ? 'Collapse' : 'Expand'} {ws.name}">
            <Icon name="chevron" class={['chev', open && 'open']} size={16} />
          </button>
          <span class="tree-label">
            <span class="tree-title">{ws.name}</span>
            {#if ws.cwd}<span class="tree-sub">{shortPath(ws.cwd)}</span>{/if}
          </span>
          <button
            class="icon-btn"
            aria-label="Actions for {ws.name}"
            disabled={!live.value}
            onclick={(e) => openMenu([{ label: 'New project', onSelect: () => (create = { kind: 'project', ws }) }], e.currentTarget, ws.name)}
          >
            <Icon name="more" />
          </button>
        </div>
        {#if open}
          <ul role="group">
            {#each projects as { p, streams }}
              {@const popen = !!query || expandedNodes.value[p.id] !== false}
              <li role="treeitem" aria-expanded={popen} aria-selected={false} class="tree-proj">
                <div class="tree-row lvl-1">
                  <button class="tree-toggle" onclick={() => toggle(p.id)} aria-label="{popen ? 'Collapse' : 'Expand'} {p.name}">
                    <Icon name="chevron" class={['chev', popen && 'open']} size={16} />
                  </button>
                  <span class="tree-label">
                    <span class="tree-title">{p.name}</span>
                    <span class="tree-sub">{streams.length} stream{streams.length === 1 ? '' : 's'}</span>
                  </span>
                  <button
                    class="icon-btn"
                    aria-label="Actions for {p.name}"
                    disabled={!live.value}
                    onclick={(e) => openMenu([{ label: 'New stream', onSelect: () => (create = { kind: 'stream', project: p }) }], e.currentTarget, p.name)}
                  >
                    <Icon name="more" />
                  </button>
                </div>
                {#if popen}
                  <ul role="group" class="rows tree-streams">
                    {#each streams as s (s.id)}
                      <StreamRow {s} {onPeek} dim={stale.value} showContext={false} role="treeitem" />
                    {/each}
                  </ul>
                {/if}
              </li>
            {/each}
          </ul>
        {/if}
      </li>
    {/each}
  </ul>
  {#if compact}
    <button class="btn btn-block" disabled={!live.value} onclick={() => (create = { kind: 'workspace' })}>
      <Icon name="plus" size={18} /> New workspace
    </button>
  {/if}
  {#if peek && peekSheet.component.value}{@const PeekSheet = peekSheet.component.value}<PeekSheet stream={peek} onClose={() => (peek = null)} />{/if}
  {#if create?.kind === 'workspace'}<NewWorkspace onClose={() => (create = null)} />{/if}
  {#if create?.kind === 'project'}<NewProject ws={create.ws} onClose={() => (create = null)} />{/if}
  {#if create?.kind === 'stream'}<NewStream project={create.project} onClose={() => (create = null)} />{/if}
</div>
