<!--
  {{name}}: a tab of its own in Briardr's tab bar (sidebar on iPad and Mac). {{description}}
-->
<script lang="ts">
{{#backend}}
  import type { AppTabProps } from '@briardr/sdk';
  import { onMount } from 'svelte';
{{/backend}}
{{^backend}}
  import { Empty, type AppTabProps } from '@briardr/sdk';
{{/backend}}

  let { briardr }: AppTabProps = $props();
{{#backend}}
  let data = $state.raw<{ message: string } | null>(null);
  onMount(() => {
    briardr.api('GET', '/hello').then((r) => (data = r), (e) => briardr.toast(e.message));
  });
{{/backend}}
</script>

<div class="screen">
  <header class="screen-head">
    <h1>{{name}}</h1>
  </header>
  <div class="screen-body">
{{#backend}}
    {#if data}<p class="padded">{data.message}</p>{/if}
{{/backend}}
{{^backend}}
    <Empty title="{{name}}">Nothing here yet. Briardr runs on {briardr.host().the}.</Empty>
{{/backend}}
  </div>
</div>
