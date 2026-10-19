<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, Icon, type PluginContext, type Stream } from '@briardr/sdk';
  import { renderMarkdown } from '@briardr/sdk/markdown';

  let { stream, briardr, path, onBack, onOpen, canBack }: { stream: Stream; briardr: PluginContext; path: string; onBack: () => void; onOpen: (p: string) => void; canBack: boolean } = $props();
  let doc = $state.raw<{ text: string; size: number } | null>(null);
  let error = $state<string | null>(null);
  let source = $state(false);
  let body: HTMLElement | undefined = $state();
  const sid = $derived(stream.id);
  const html = $derived(doc ? renderMarkdown(doc.text, { docPath: path, streamId: sid }) : '');

  $effect(() => {
    const p = path;
    doc = null;
    error = null;
    untrack(() => briardr.core('GET', `/api/streams/${encodeURIComponent(stream.id)}/docs/file?path=${encodeURIComponent(p)}`)).then(
      (r) => (doc = r),
      (e) => (error = e instanceof ApiError ? e.message : 'Couldn’t open this file.'),
    );
  });

  function onclick(e: MouseEvent) {
    const a = (e.target as HTMLElement).closest('a');
    if (!a) return;
    if (a.dataset.external) return; // opens in the system browser via target=_blank
    e.preventDefault();
    if (a.dataset.anchor !== undefined) {
      body?.querySelector(`#md-${CSS.escape(a.dataset.anchor)}`)?.scrollIntoView({ behavior: 'smooth' });
      return;
    }
    const target = a.dataset.doc;
    if (!target) return;
    if (/\.(md|markdown|mdown|mkd)$/i.test(target)) onOpen(target);
  }
</script>

<div class="surface docs doc-view">
  <div class="surface-head">
    {#if canBack}
      <button class="icon-btn" aria-label="Back to documents" onclick={onBack}>
        <Icon name="back" />
      </button>
    {/if}
    <div class="surface-title">
      <strong>{path.split('/').pop()}</strong>
      <span class="muted">{` · ${path}`}</span>
    </div>
    <div class="segmented small" role="group" aria-label="View">
      <button aria-pressed={!source} class={!source ? 'is-on' : ''} onclick={() => (source = false)}>Rendered</button>
      <button aria-pressed={source} class={source ? 'is-on' : ''} onclick={() => (source = true)}>Source</button>
    </div>
  </div>
  {#if error}<p class="error padded" role="alert">{error}</p>{/if}
  {#if doc}
    {#if source}
      <!-- svelte-ignore a11y_no_noninteractive_tabindex -->
      <pre class="md-source" tabindex={0}>{doc.text}</pre>
    {:else}
      <!-- Links inside are handled here: in-document anchors, other docs, external links. They keep an
           href, so the keyboard reaches them and Enter fires the click that arrives here. -->
      <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_noninteractive_element_interactions -->
      <article bind:this={body} class="md" {onclick}>{@html html}</article>
    {/if}
  {/if}
</div>
