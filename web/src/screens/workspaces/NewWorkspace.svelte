<script lang="ts">
  import { theHost, TheHost } from '../../host.ts';
  import { api } from '../../api.ts';
  import { toast } from '../../store.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { Submit } from '../submit.svelte.ts';

  let { onClose }: { onClose: () => void } = $props();
  let name = $state('');
  let folder = $state('~/');
  const s = new Submit(async () => {
    const r = await api('POST', '/api/workspaces', { name, folder });
    toast(`Created workspace${r.focusUnchanged === false ? ` (${theHost()}’s focus changed)` : ''}`);
    onClose();
  });
</script>

<Sheet title="New workspace" {onClose}>
  <form onsubmit={s.run} class="form">
    <label class="field-label">
      Name
      <!-- svelte-ignore a11y_autofocus -->
      <input class="field" bind:value={name} placeholder="e.g. api" autofocus />
    </label>
    <label class="field-label">
      Folder on {theHost()}
      <input class="field mono" bind:value={folder} autocapitalize="off" autocorrect="off" spellcheck={false} />
    </label>
    <p class="hint">{TheHost()} keeps its current focus. The new workspace opens in the background.</p>
    {#if s.error}<p class="error" role="alert">{s.error}</p>{/if}
    <div class="form-actions">
      <button type="button" class="btn" onclick={onClose}>Cancel</button>
      <button class="btn btn-primary" disabled={s.busy || !folder.trim()}>{s.busy ? 'Creating…' : 'Create'}</button>
    </div>
  </form>
</Sheet>
