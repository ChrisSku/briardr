<script lang="ts">
  import { theHost } from '../../host.ts';
  import { toast } from '../../store.ts';
  import Section from './Section.svelte';
  import type { Save, Settings } from './settings.ts';

  let { s, save }: { s: Settings | null; save: Save } = $props();
  // The field shows the saved path; typing overrides it until the Mac reports a different one.
  const saved = $derived(s?.herdrPath ?? '');
  let path = $derived(saved);
</script>

<Section id="herdr" title="Herdr">
  <form
    class="form"
    onsubmit={async (e) => {
      e.preventDefault();
      if (await save({ herdrPath: path })) toast('Herdr path saved');
    }}
  >
    <label class="field-label">
      Path to the herdr executable on {theHost()}
      <input class="field mono" bind:value={path} autocapitalize="off" autocorrect="off" spellcheck={false} />
    </label>
    <div class="form-actions">
      <span class="muted">
        {s ? (s.herdr.running ? `Herdr ${s.herdr.version} is running${s.herdrSession ? ` (session “${s.herdrSession}”)` : ''}.` : s.herdr.problem === 'not_found' ? 'No herdr executable at this path.' : 'Herdr isn’t running.') : '…'}
      </span>
      <button class="btn" disabled={!s || path === s.herdrPath}>Save</button>
    </div>
  </form>
  <p class="hint">Briardr talks to Herdr only on {theHost()}. Herdr’s control socket is never exposed to the network.</p>
</Section>
