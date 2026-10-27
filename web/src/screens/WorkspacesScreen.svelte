<script lang="ts">
  import { live } from '../store.ts';
  import { Icon } from '../ui/icons/index.ts';
  import InstallSuggestion from './InstallSuggestion.svelte';
  import WorkspaceTree from './WorkspaceTree.svelte';

  let newWorkspace = $state(false);
</script>

<div class="screen">
  <header class="screen-head">
    <h1>Workspaces</h1>
    <button class="icon-btn round ink" aria-label="New workspace" disabled={!live.value} onclick={() => (newWorkspace = true)}>
      <Icon name="plus" size={18} />
    </button>
  </header>
  <div class="screen-body">
    <InstallSuggestion />
    <WorkspaceTree bind:newWorkspace />
  </div>
</div>
