<script lang="ts">
  import { theHost } from '../../host.ts';
  import { toast } from '../../store.ts';
  import Section from './Section.svelte';
  import type { Save, Settings } from './settings.ts';

  let { s, save }: { s: Settings | null; save: Save } = $props();
  // The field shows the saved path; typing overrides it until the Mac reports a different one.
  const saved = $derived(s?.herdrPath ?? '');
  let path = $derived(saved);
  const core = $derived(s?.herdrBackend === 'core');

  async function choose(backend: 'tui' | 'core') {
    if (!s || s.herdrBackend === backend) return;
    if (await save({ herdrBackend: backend })) toast(backend === 'core' ? 'Briardr now uses the Herdr app' : 'Briardr now uses Herdr TUI');
  }

  function status(s: Settings): string {
    if (s.herdr.running) return core ? `herdr-core ${s.herdr.version} is running.` : `Herdr ${s.herdr.version} is running${s.herdrSession ? ` (session “${s.herdrSession}”)` : ''}.`;
    if (!core) return s.herdr.problem === 'not_found' ? 'No herdr executable at this path.' : 'Herdr isn’t running.';
    return s.herdr.problem === 'timeout' ? 'herdr-core isn’t answering.' : 'herdr-core isn’t running.';
  }
</script>

<Section id="herdr" title="Herdr">
  <div class="segmented" role="group" aria-label="Herdr to use">
    <button class={[!core && 'is-on']} aria-pressed={!core} disabled={!s} onclick={() => choose('tui')}>Herdr TUI</button>
    <button class={[core && 'is-on']} aria-pressed={core} disabled={!s} onclick={() => choose('core')}>Herdr app</button>
  </div>
  {#if core}
    <p class="muted">{s ? status(s) : '…'}</p>
    {#if s && !s.herdr.running}
      <p class="hint">
        {s.herdr.problem === 'timeout'
          ? `On ${theHost()}, look for a macOS privacy prompt for Herdr Core.`
          : `Open the Herdr app on ${theHost()}; Briardr connects when its core is up.`}
      </p>
    {/if}
    <p class="hint">Briardr talks to herdr-core at <code>{s?.herdrCoreSocket ?? '…'}</code> on {theHost()}, and never starts it.</p>
  {:else}
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
        <span class="muted">{s ? status(s) : '…'}</span>
        <button class="btn" disabled={!s || path === s.herdrPath}>Save</button>
      </div>
    </form>
    {#if s?.herdr.problem === 'not_found'}
      <p class="hint">To install the Herdr this Briardr is tested with, run <code>briardr setup</code> on {theHost()}.</p>
    {:else if s && !s.herdr.running}
      <p class="hint">Start it on {theHost()} with <code>{s.herdrSession ? `herdr --session ${s.herdrSession}` : 'herdr'}</code>; Briardr connects when it’s up.</p>
    {/if}
  {/if}
  <p class="hint">Briardr talks to Herdr only on {theHost()}. Herdr’s control socket is never exposed to the network.</p>
</Section>
