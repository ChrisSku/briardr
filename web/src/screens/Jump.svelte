<!--
  Jump search: open any stream by name, project, workspace or folder. Streams that need input come
  first; the rest are ordered by their latest activity: opened on this device, finished, or started
  working. It opens at the top, just under the screen's header, and never reaches under the keyboard,
  so the field stays in place while the list shrinks.
-->
<script module lang="ts">
  import { model } from '../store.ts';
  import type { Stream } from '../types.ts';

  let open = $state(false);
  export const openJump = () => (open = true);

  export function matches(s: Stream, q: string): boolean {
    if (!q) return true;
    const m = model.value;
    const hay = [s.name, s.folder, s.cwd, s.tool, m?.workspaces.find((w) => w.id === s.workspaceId)?.name, m?.projects.find((p) => p.id === s.projectId)?.name].filter(Boolean).join(' ').toLowerCase();
    return q.toLowerCase().split(/\s+/).every((t) => hay.includes(t));
  }

  /** Where the sheet goes: below the visible screen's header, down to the top of the keyboard. */
  function measure() {
    const vv = window.visualViewport;
    const bottom = vv ? vv.offsetTop + vv.height : innerHeight;
    const head = document.querySelector('.main .screen-head')?.getBoundingClientRect().bottom;
    const top = Math.round((head || 0) + 8);
    return { top, maxHeight: Math.max(160, Math.floor(bottom - top - 8)) };
  }
</script>

<script lang="ts">
  import { byActivity } from '../activity.ts';
  import { lastOpened } from '../store.ts';
  import { ToolIcon } from '../ui/icons/index.ts';
  import { Sheet, StateBadge } from '../ui/kit.ts';
  import { contextOf, openStream } from './streams.ts';

  let q = $state('');
  let sel = $state(0);
  let at = $state(measure());

  $effect(() => {
    if (!open) return;
    const vv = window.visualViewport;
    const update = () => (at = measure());
    update();
    vv?.addEventListener('resize', update);
    vv?.addEventListener('scroll', update);
    window.addEventListener('resize', update);
    return () => {
      vv?.removeEventListener('resize', update);
      vv?.removeEventListener('scroll', update);
      window.removeEventListener('resize', update);
    };
  });

  const list = $derived(open ? byActivity((model.value?.streams || []).filter((s) => matches(s, q)), lastOpened.value).slice(0, 50) : []);

  function close() {
    open = false;
    q = '';
    sel = 0;
  }
  function go(s: Stream) {
    close();
    openStream(s);
  }
  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      sel = Math.min(list.length - 1, sel + 1);
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      sel = Math.max(0, sel - 1);
    } else if (e.key === 'Enter' && list[sel]) go(list[sel]);
  }
</script>

{#if open}
  <Sheet title="Jump to stream" onClose={close} class="jump" {at}>
    <!-- svelte-ignore a11y_autofocus -->
    <input
      class="field"
      type="search"
      placeholder="Stream, project, workspace or folder"
      aria-label="Search streams"
      aria-controls="jump-list"
      aria-activedescendant={list[sel] ? `jump-${sel}` : undefined}
      bind:value={q}
      oninput={() => (sel = 0)}
      autofocus
      {onkeydown}
    />
    <ul class="rows jump-list" id="jump-list" role="listbox" aria-label="Streams">
      {#each list as s, i}
        <li id="jump-{i}" role="option" aria-selected={i === sel} class={['row', i === sel && 'is-sel']}>
          <button class="row-main" onclick={() => go(s)} onmouseenter={() => (sel = i)}>
            <ToolIcon tool={s.tool} kind={s.kind} />
            <span class="row-text">
              <span class="row-title">{s.name}</span>
              <span class="row-sub">{contextOf(s)}</span>
            </span>
            <span class="row-trail">
              <StateBadge state={s.state} />
            </span>
          </button>
        </li>
      {/each}
      {#if !list.length}<li class="row-empty">No matching streams.</li>{/if}
    </ul>
  </Sheet>
{/if}
