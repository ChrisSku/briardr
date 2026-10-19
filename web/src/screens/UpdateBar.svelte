<!-- An installed update waiting for work in progress (web/src/updates.ts), with the reload it waits for. -->
<script lang="ts">
  import { theHost } from '../host.ts';
  import { reloadNow, updateReady } from '../updates.ts';
  import { Icon } from '../ui/icons/index.ts';

  const u = $derived(updateReady.value);
</script>

{#if u}
  <div class="update-bar" role="status" aria-live="polite">
    <Icon name="refresh" size={16} />
    <span class="update-bar-text">
      <strong>{u.reason === 'server_restart' ? `Briardr on ${theHost()} restarted.` : 'A Briardr update is ready.'}</strong>
      {u.waiting ? `It reloads once ${u.waiting}.` : 'Reloading…'}
    </span>
    <button class="btn btn-small" onclick={reloadNow}>Reload now</button>
  </div>
{/if}
