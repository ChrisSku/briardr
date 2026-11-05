<!-- Renaming a workspace, project or stream, from any list or menu. App renders it; any module can open it. -->
<script module lang="ts">
  import { state } from '../reactive.svelte.ts';

  export interface RenameTarget {
    noun: 'workspace' | 'project' | 'stream';
    /** The rename endpoint. */
    path: string;
    name: string;
    /** What shows when the name is left empty. Without one, a name is required. */
    fallback?: string;
  }
  const target = state<RenameTarget | null>(null);
  export const openRename = (t: RenameTarget) => (target.value = t);
</script>

<script lang="ts">
  import { theHost } from '../host.ts';
  import { api } from '../api.ts';
  import { toast } from '../store.ts';
  import { Sheet } from '../ui/kit.ts';
  import { Submit } from './submit.svelte.ts';

  const t = $derived(target.value);
  // Starts from the current name; editing is local.
  let name = $derived(t?.name ?? '');
  const close = () => (target.value = null);
  const s = new Submit(async () => {
    await api('POST', t!.path, { name });
    toast('Renamed');
    close();
  });
</script>

{#if t}
  <Sheet title="Rename {t.noun}" onClose={close}>
    <form class="form" onsubmit={s.run}>
      <label class="field-label">
        Name
        <!-- svelte-ignore a11y_autofocus -->
        <input class="field" bind:value={name} autofocus placeholder={t.fallback} required={!t.fallback} />
      </label>
      <p class="hint">{t.fallback ? 'Leave empty to show the tool’s name again. ' : ''}Herdr shows this name on {theHost()} too.</p>
      {#if s.error}<p class="error" role="alert">{s.error}</p>{/if}
      <div class="form-actions">
        <button type="button" class="btn" onclick={close}>Cancel</button>
        <button class="btn btn-primary" disabled={s.busy}>Save</button>
      </div>
    </form>
  </Sheet>
{/if}
