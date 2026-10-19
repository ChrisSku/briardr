<script lang="ts">
  import { TheHost } from '../../host.ts';
  import { api } from '../../api.ts';
  import { model, toast } from '../../store.ts';
  import type { Project } from '../../types.ts';
  import { ToolIcon } from '../../ui/icons/index.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { openStream } from '../streams.ts';
  import { Submit } from '../submit.svelte.ts';

  let { project, onClose }: { project: Project; onClose: () => void } = $props();
  let name = $state('');
  const KINDS = ['shell', 'claude', 'codex'] as const;
  let kind = $state<(typeof KINDS)[number]>('shell');
  const s = new Submit(async () => {
    const r = await api('POST', '/api/streams', { projectId: project.id, kind, name }, { timeout: 45_000 });
    toast(r.note || 'Created stream', r.note ? 6000 : 2600);
    onClose();
    const st = model.value?.streams.find((x) => x.id === r.streamId);
    if (st) openStream(st);
  });
</script>

<Sheet title="New stream in {project.name}" {onClose}>
  <form onsubmit={s.run} class="form">
    <fieldset class="segmented" aria-label="Run">
      {#each KINDS as k}
        <label class={kind === k ? 'is-on' : ''}>
          <input type="radio" name="kind" value={k} bind:group={kind} />
          <ToolIcon tool={k === 'shell' ? 'zsh' : k} kind={k === 'shell' ? 'shell' : 'agent'} size={18} />
          {k === 'shell' ? 'Shell' : k === 'claude' ? 'Claude' : 'Codex'}
        </label>
      {/each}
    </fieldset>
    <label class="field-label">
      Stream name (optional)
      <input class="field" bind:value={name} placeholder="e.g. refactor auth" />
    </label>
    <p class="hint">Briardr splits the largest pane in this project. {TheHost()} keeps its current focus.</p>
    {#if s.error}<p class="error" role="alert">{s.error}</p>{/if}
    <div class="form-actions">
      <button type="button" class="btn" onclick={onClose}>Cancel</button>
      <button class="btn btn-primary" disabled={s.busy}>{s.busy ? 'Starting…' : 'Create'}</button>
    </div>
  </form>
</Sheet>
