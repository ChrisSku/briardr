<script lang="ts">
  import { theHost } from '../../host.ts';
  import { api, ApiError } from '../../api.ts';
  import { toast } from '../../store.ts';
  import type { Stream } from '../../types.ts';
  import { Sheet } from '../../ui/kit.ts';

  let { stream, onClose }: { stream: Stream; onClose: () => void } = $props();
  // svelte-ignore state_referenced_locally
  let name = $state(stream.label || ''); // starts from the current name; editing is local
  let err = $state<string | null>(null);

  async function save(e: Event) {
    e.preventDefault();
    try {
      await api('POST', `/api/streams/${encodeURIComponent(stream.id)}/rename`, { name });
      toast('Renamed');
      onClose();
    } catch (x) {
      err = x instanceof ApiError ? x.message : 'Couldn’t rename.';
    }
  }
</script>

<Sheet title="Rename stream" {onClose}>
  <form class="form" onsubmit={save}>
    <label class="field-label">
      Name
      <!-- svelte-ignore a11y_autofocus -->
      <input class="field" bind:value={name} autofocus placeholder={stream.tool} />
    </label>
    <p class="hint">Leave empty to show the tool’s name again. Herdr shows this name on {theHost()} too.</p>
    {#if err}<p class="error" role="alert">{err}</p>{/if}
    <div class="form-actions">
      <button type="button" class="btn" onclick={onClose}>Cancel</button>
      <button class="btn btn-primary">Save</button>
    </div>
  </form>
</Sheet>
