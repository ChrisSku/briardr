<!-- Installing Briardr as an app (web/src/install.ts), and whether it opens offline (web/src/offline.ts). -->
<script lang="ts">
  import { theHost } from '../../host.ts';
  import { installPrompt, installSteps, justInstalled, launchHint, platform, promptInstall, standalone } from '../../install.ts';
  import { offline } from '../../offline.ts';
  import { navigate } from '../../router.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import Section from './Section.svelte';

  const p = platform();
  const apple = p === 'iphone' || p === 'ipad';
</script>

<Section id="install" title="Install app">
  {#if standalone}
    <p>Briardr is installed on this device, and open as an app now.</p>
  {:else if justInstalled.value}
    <p role="status">Briardr is installed. {launchHint(p)}</p>
  {:else}
    <p>Install Briardr to open it like an app: in its own window, from {apple || p === 'android' ? 'the Home Screen' : 'the Dock or your apps'}, without the browser’s controls.{apple ? ' On iPhone and iPad, notifications only arrive in the installed app.' : ''}</p>
    {#if installPrompt.value}
      <button class="btn btn-primary" onclick={promptInstall}><Icon name="device" size={16} /> Install Briardr</button>
    {:else}
      <p class="install-steps">{installSteps(p)}</p>
    {/if}
    <p class="hint">
      Then {launchHint(p).replace(/^Open/, 'open')} The installed app may not share this browser’s sign-in: if it asks for a pairing code, get one here in Settings → Devices → Add a device, or with <code>mise run pair</code> on {theHost()}.
    </p>
  {/if}
  <p class="hint offline-ready" role="status">
    {#if offline.value.state === 'ready'}
      Opens offline: ready. While {theHost()} is unreachable, Briardr opens with the last known state.
    {:else if offline.value.state === 'preparing'}
      Saving Briardr for offline use…
    {:else if offline.value.state === 'unsupported'}
      This browser can’t open Briardr offline.
    {:else}
      Briardr can’t open offline yet. <a href="/settings/diagnostics" onclick={(e) => (e.preventDefault(), navigate({ name: 'settings', section: 'diagnostics' }, { replace: true }))}>See Diagnostics</a>.
    {/if}
  </p>
</Section>
