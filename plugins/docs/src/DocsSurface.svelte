<!-- Docs: Markdown files in the stream's folder, rendered natively, with a Source toggle. -->
<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, AsOf, Empty, Icon, PullToRefresh, type StreamTabProps } from '@briardr/sdk';
  import DocView from './DocView.svelte';

  interface DocEntry {
    path: string;
    name: string;
    changed: boolean;
    readme: boolean;
    size: number;
  }

  let { stream, visible, briardr }: StreamTabProps = $props();
  let list = $state.raw<{ docs: DocEntry[]; total: number; truncated: boolean; asOf: number } | null>(null);
  let error = $state<string | null>(null);
  let stack = $state.raw<string[]>([]);
  const current = $derived(stack[stack.length - 1] || null);

  async function load() {
    error = null;
    try {
      list = await briardr.core('GET', `/api/streams/${encodeURIComponent(stream.id)}/docs`, undefined, { timeout: 20_000 });
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Couldn’t list documents.';
    }
  }
  const id = $derived(stream.id);
  $effect(() => {
    void id;
    if (visible && !untrack(() => current)) untrack(load);
  });
  const ptr = new PullToRefresh(load);
</script>

{#if visible}
  {#if current}
    <DocView {stream} {briardr} path={current} canBack onBack={() => (stack = stack.slice(0, -1))} onOpen={(p) => (stack = [...stack, p])} />
  {:else}
    <div class="surface docs" {...ptr.handlers}>
      <div class="surface-head">
        <div class="surface-title">
          <strong>Docs</strong>
          <span class="muted">{` · ${stream.folder}`}</span>
          <AsOf at={list?.asOf} />
        </div>
        <button class="icon-btn" aria-label="Refresh documents" onclick={load}>
          <Icon name="refresh" />
        </button>
      </div>
      {#if error}<p class="error padded" role="alert">{error}</p>{/if}
      {#if list && !list.docs.length}<Empty title="No Markdown files">There are no .md files in this stream’s folder.</Empty>{/if}
      {#if list?.truncated}<p class="note">Showing {list.docs.length} of {list.total} documents.</p>{/if}
      <ul class="rows doc-list">
        {#each list?.docs ?? [] as d}
          <li class="row">
            <button class="row-main" onclick={() => (stack = [d.path])}>
              <span class="row-text">
                <span class="row-title">{d.name}</span>
                <span class="row-sub">{d.path}</span>
              </span>
              <span class="row-trail">
                {#if d.changed}<span class="tag tag-changed">changed</span>{/if}
                {#if d.readme && !d.changed}<span class="tag">readme</span>{/if}
              </span>
            </button>
          </li>
        {/each}
      </ul>
    </div>
  {/if}
{/if}
