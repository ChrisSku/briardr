<script lang="ts">
  import { Icon } from '../ui/icons/index.ts';
  import AgentsList from './AgentsList.svelte';
  import InstallSuggestion from './InstallSuggestion.svelte';
  import { openJump } from './Jump.svelte';
</script>

<div class="screen">
  <header class="screen-head">
    <h1>Agents</h1>
    <button class="icon-btn" aria-label="Jump to a stream" title="Jump to a stream (⌘K)" onclick={() => openJump()}>
      <Icon name="search" />
    </button>
  </header>
  <div class="screen-body">
    <InstallSuggestion />
    <AgentsList />
  </div>
</div>
