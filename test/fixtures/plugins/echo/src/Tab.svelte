<script lang="ts">
  import { Empty, Icon, type StreamTabProps } from '@briardr/sdk';
  import { untrack } from 'svelte';

  let { stream, visible, briardr }: StreamTabProps = $props();
  let count = $state(0);
  const doubled = $derived(count * 2);
  untrack(() => count);
  let answer = $state('');
  let notes = $state(0);

  async function ask() {
    const r = await briardr.api<{ hello: string; stream: string }>('GET', `/hello?stream=${encodeURIComponent(stream.id)}`);
    answer = `${r.hello} in ${r.stream}`;
  }
  // Events the backend pushes; the unsubscribe runs when the tab goes away.
  $effect(() => briardr.onEvent('note', () => notes++));
</script>

{#if visible}
  <div class="surface echo-tab">
    <div class="surface-head">
      <div class="surface-title"><strong>Echo</strong><span class="muted">{` · ${stream.name}`}</span></div>
    </div>
    <p class="padded">
      <button class="btn" onclick={() => count++}><Icon name="plus" size={16} /> Clicked {count}, doubled {doubled}</button>
      <button class="btn" disabled={!briardr.terminal.canSend} onclick={() => briardr.terminal.send('echo from plugin')}>Send to terminal</button>
      <button class="btn" onclick={ask}>Ask backend</button>
      <button class="btn" onclick={() => briardr.api('POST', '/emit')}>Emit</button>
    </p>
    <p class="padded" data-testid="answer">{answer}</p>
    <p class="padded" data-testid="notes">Events: {notes}</p>
    <Empty title="The echo plugin is running">Served by {briardr.host().the}.</Empty>
  </div>
{/if}

<style>
  .echo-tab strong {
    color: rgb(102, 51, 153);
  }
</style>
