<script lang="ts">
  import { ApiError, Sheet, type PluginContext } from '@briardr/sdk';
  import type { Usage } from './usage.ts';

  let { briardr, name, available, onClose, onUsed }: { briardr: PluginContext; name: string; available: number; onClose: () => void; onUsed: (u: Usage) => void } = $props();

  const OUTCOME: Record<string, string> = {
    reset: 'Codex usage was reset.',
    nothingToReset: 'Nothing to reset right now, so no reset was used.',
    noCredit: 'No reset credits are available.',
    alreadyRedeemed: 'That reset was already used.',
  };

  let busy = $state(false);
  let error = $state<string | null>(null);

  async function use() {
    busy = true;
    error = null;
    try {
      const r = await briardr.core<{ outcome: string; usage: Usage }>('POST', '/api/usage/codex/reset-credit', { confirm: true }, { timeout: 60_000 });
      onUsed(r.usage);
      briardr.toast(OUTCOME[r.outcome] || 'Done.');
      onClose();
    } catch (e) {
      error = e instanceof ApiError ? e.message : `Couldn’t reach ${briardr.host().the}.`;
      busy = false;
    }
  }
</script>

<Sheet title="Reset {name} usage?" {onClose}>
  <p>This resets your current {name} limits now and uses one limit reset ({available} available). A used reset can’t be given back.</p>
  {#if error}<p class="note tone-error" role="alert">{error}</p>{/if}
  {#snippet footer()}
    <div class="form-actions">
      <button class="btn" onclick={onClose} disabled={busy}>Cancel</button>
      <button class="btn btn-primary" onclick={use} disabled={busy}>{busy ? 'Resetting…' : 'Use a reset'}</button>
    </div>
  {/snippet}
</Sheet>
