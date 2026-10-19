<script lang="ts">
  import { HostLabel } from '../../host.ts';
  import { onMount } from 'svelte';
  import { api } from '../../api.ts';
  import { model, toast } from '../../store.ts';
  import type { TerminalController } from '../../terminal/controller.ts';
  import type { Stream } from '../../types.ts';
  import { AsOf, Sheet } from '../../ui/kit.ts';

  let { stream, ctl, onClose }: { stream: Stream; ctl: TerminalController; onClose: () => void } = $props();
  let info = $state.raw<any>(null);
  onMount(() => {
    api('GET', `/api/streams/${encodeURIComponent(stream.id)}/info`).then((r) => (info = r), () => (info = { error: true }));
  });

  const st = $derived(ctl.status.value);
  const rows = $derived(
    (
      [
        ['Stream', stream.id],
        ['Terminal', stream.terminalId],
        ['Workspace', `${model.value?.workspaces.find((w) => w.id === stream.workspaceId)?.name || ''} (${stream.workspaceId})`],
        ['Project', `${model.value?.projects.find((p) => p.id === stream.projectId)?.name || ''} (${stream.projectId})`],
        ['Folder', stream.cwd],
        ['Program', `${stream.tool} (${stream.kind})`],
        ['State', stream.state ? `${stream.state}${stream.stateChangeSeq !== null ? ` · change #${stream.stateChangeSeq}` : ''}` : 'No agent'],
        ['Agent name', info?.agent?.name],
        [`${HostLabel()} terminal size`, st.width ? `${st.width} × ${st.height}` : null],
        ['This view', st.ownership],
      ] as [string, string | null | undefined][]
    ).filter(([, v]) => v),
  );
</script>

<Sheet title="Stream info" {onClose}>
  <dl class="info">
    {#each rows as [k, v]}
      <dt>{k}</dt>
      <dd class="mono">
        <button class="copy-value" title="Copy" onclick={() => navigator.clipboard.writeText(String(v)).then(() => toast('Copied'))}>{v}</button>
      </dd>
    {/each}
  </dl>
  <p class="hint">
    <AsOf at={model.value?.asOf} />
  </p>
</Sheet>
