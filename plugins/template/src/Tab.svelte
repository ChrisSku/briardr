<!--
  {{name}}: a tab in every stream, next to Files and Changes. {{description}}
  Briardr keeps the tab mounted while the stream is open; render only while `visible`.
-->
<script lang="ts">
{{#backend}}
  import { AsOf, Icon, PullToRefresh, type StreamTabProps } from '@briardr/sdk';
  import { untrack } from 'svelte';
{{/backend}}
{{^backend}}
  import { Empty, PullToRefresh, type StreamTabProps } from '@briardr/sdk';
{{/backend}}

  let { stream, visible, briardr }: StreamTabProps = $props();
{{#backend}}
  let data = $state.raw<{ message: string; asOf: number } | null>(null);
  let error = $state<string | null>(null);

  async function load() {
    error = null;
    try {
      // Goes to this plugin's backend (server/main.ts), with the stream it's for.
      data = await briardr.api('GET', `/hello?stream=${encodeURIComponent(stream.id)}`);
    } catch (e) {
      error = e instanceof Error ? e.message : 'Couldn’t load.';
    }
  }
  // Load when the tab is shown, and again when the agent's state changes while it's shown.
  const agentState = $derived(stream.state);
  $effect(() => {
    void agentState;
    if (visible) untrack(load);
  });
  const ptr = new PullToRefresh(load);
{{/backend}}
{{^backend}}
  const ptr = new PullToRefresh(async () => {});
{{/backend}}
</script>

{#if visible}
  <div class="surface" {...ptr.handlers}>
    <div class="surface-head">
      <div class="surface-title">
        <strong>{{name}}</strong>
        <span class="muted">{` · ${stream.folder ?? stream.name}`}</span>
{{#backend}}
        <AsOf at={data?.asOf} />
{{/backend}}
      </div>
{{#backend}}
      <button class="icon-btn" aria-label="Refresh" onclick={load}>
        <Icon name="refresh" />
      </button>
{{/backend}}
    </div>
{{#backend}}
    {#if error}<p class="error padded" role="alert">{error}</p>{/if}
    {#if data}<p class="padded">{data.message}</p>{/if}
{{/backend}}
{{^backend}}
    <Empty title="{{name}}">Nothing here yet. {stream.name} is a {stream.kind} in {stream.cwd}.</Empty>
{{/backend}}
  </div>
{/if}
