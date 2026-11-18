<script lang="ts">
  import { Icon } from '@briardr/sdk';
  import type { FileEntry } from './files.ts';
  import FileIcon from './FileIcon.svelte';

  let { entry: e, depth, selected, expanded, searching, onOpen, onMenu }: {
    entry: FileEntry; depth: number; selected: boolean; expanded: boolean; searching: boolean;
    onOpen: () => void; onMenu: (at: HTMLElement) => void;
  } = $props();
</script>

<div class={['ft-entry', selected && 'is-on']}>
  <button
    class={['ft-row', searching && 'ft-result', selected && 'is-on', e.changed && 'is-changed']}
    style:--depth={depth}
    aria-expanded={e.dir ? expanded : undefined}
    aria-current={selected ? 'true' : undefined}
    aria-label={`${searching ? e.path : e.name}${e.dir ? ', folder' : ''}${e.changed ? ', changed' : ''}`}
    title={e.path}
    onclick={onOpen}
  >
    {#if e.dir}
      <Icon name="chevron" size={14} class={['ft-chev', expanded && 'is-open']} />
    {:else}
      <span class="ft-chev"></span>
    {/if}
    <FileIcon name={e.name} folder={e.dir} {expanded} />
    <span class="ft-name">{e.name}{#if searching}<small class="ft-path">{e.path.includes('/') ? e.path.slice(0, e.path.lastIndexOf('/')) : '.'}</small>{/if}</span>
    {#if e.link}<Icon name="link" size={13} class="ft-link" />{/if}
    {#if e.changed}<span class="ft-dot"></span>{/if}
  </button>
  <button class="icon-btn ft-more" aria-label={`More actions for ${e.path}`} aria-haspopup="menu" title="More actions" onclick={(ev) => onMenu(ev.currentTarget)}>
    <Icon name="more" size={18} />
  </button>
</div>

<style>
  .ft-entry { display: flex; align-items: stretch; border-radius: 8px; }
  .ft-entry:hover { background: color-mix(in srgb, var(--text-primary) 6%, transparent); }
  .ft-entry.is-on { background: color-mix(in srgb, var(--ink-default) 16%, transparent); }
  .icon-btn.ft-more { flex: none; align-self: center; min-width: 32px; width: 32px; min-height: 32px; height: 32px; opacity: 0.55; }
  .icon-btn.ft-more:hover, .icon-btn.ft-more:focus-visible { opacity: 1; }
  @media (pointer: coarse) { .icon-btn.ft-more { min-width: 44px; width: 44px; min-height: 44px; height: 44px; } }
  .ft-row.ft-result { padding-top: 6px; padding-bottom: 6px; }
  .ft-row :global(.ft-chev) { flex: none; width: 14px; color: var(--text-tertiary); transition: transform 0.15s ease; }
  .ft-row :global(.ft-chev.is-open) { transform: rotate(90deg); }
  .ft-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .ft-row :global(.ft-link) { flex: none; color: var(--text-tertiary); }
</style>
