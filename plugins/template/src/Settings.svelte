<!--
  {{name}}: a section in Briardr's Settings (Briardr adds the heading). {{description}}
  Values kept with briardr.prefs are per device; keep shared settings in the backend.
-->
<script lang="ts">
  import type { SettingsProps } from '@briardr/sdk';

  let { briardr }: SettingsProps = $props();
  // `briardr` is the same object for the plugin's whole life, so reading it once is right.
  // svelte-ignore state_referenced_locally
  let on = $state(briardr.prefs.get('on', false));
</script>

<label class="switch-row">
  <input type="checkbox" role="switch" bind:checked={on} onchange={() => briardr.prefs.set('on', on)} />
  <span>
    An example switch
    <small>Saved on this device only.</small>
  </span>
</label>
