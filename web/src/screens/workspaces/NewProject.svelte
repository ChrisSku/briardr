<script lang="ts">
  import { api } from '../../api.ts';
  import { toast } from '../../store.ts';
  import type { Workspace } from '../../types.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { Submit } from '../submit.svelte.ts';

  let { ws, onClose }: { ws: Workspace; onClose: () => void } = $props();
  let name = $state('');
  const s = new Submit(async () => {
    await api('POST', '/api/projects', { workspaceId: ws.id, name });
    toast('Created project');
    onClose();
  });
</script>

<Sheet title="New project in {ws.name}" {onClose}>
  <form onsubmit={s.run} class="form">
    <label class="field-label">
      Name
      <!-- svelte-ignore a11y_autofocus -->
      <input class="field" bind:value={name} placeholder="e.g. backend" autofocus />
    </label>
    {#if s.error}<p class="error" role="alert">{s.error}</p>{/if}
    <div class="form-actions">
      <button type="button" class="btn" onclick={onClose}>Cancel</button>
      <button class="btn btn-primary" disabled={s.busy}>{s.busy ? 'Creating…' : 'Create'}</button>
    </div>
  </form>
</Sheet>
