<!-- After pairing in a browser tab: a suggestion to install Briardr, on the lists only (never over a terminal). -->
<script lang="ts">
  import { theHost } from '../host.ts';
  import { installPrompt, installSuggestion, justInstalled, promptInstall, standalone } from '../install.ts';
  import { navigate } from '../router.ts';
  import { Icon } from '../ui/icons/index.ts';
</script>

{#if installSuggestion.value === 'pending' && !standalone && !justInstalled.value}
  <aside class="install-card" aria-label="Install Briardr">
    <Icon name="device" />
    <div class="install-card-text">
      <strong>Install Briardr as an app</strong>
      <span>It opens in its own window, shows the last known state while {theHost()} is unreachable, and can get notifications.</span>
    </div>
    <div class="install-card-actions">
      {#if installPrompt.value}
        <button class="btn btn-small btn-primary" onclick={promptInstall}>Install</button>
      {:else}
        <button class="btn btn-small btn-primary" onclick={() => navigate({ name: 'settings', section: 'install' })}>How</button>
      {/if}
      <button class="btn btn-small" onclick={() => (installSuggestion.value = 'dismissed')}>Not now</button>
    </div>
  </aside>
{/if}
