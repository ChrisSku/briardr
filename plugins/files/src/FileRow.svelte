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
