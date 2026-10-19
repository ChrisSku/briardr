<!--
  Plugins: built-in features and installed plugins, turned on or off for every device. A plugin with a
  backend runs a program on the host, so turning it on first shows that command for approval.
-->
<script lang="ts">
  import { api, ApiError } from '../../api.ts';
  import { theHost } from '../../host.ts';
  import { loadErrors, plugins, setPlugins } from '../../plugins/registry.svelte.ts';
  import type { PluginInfo } from '../../plugins/types.ts';
  import { toast } from '../../store.ts';
  import { confirmDialog } from '../../ui/kit.ts';
  import Section from './Section.svelte';

  let busy = $state<string | null>(null);
  /** The plugin whose backend command the confirmation shows. */
  let asking = $state.raw<PluginInfo | null>(null);
  const installed = $derived(plugins.value.filter((p) => !p.builtin));

  // Fresh from the server: plugins may have been installed or changed since the cached list.
  api<{ plugins: PluginInfo[] }>('GET', '/api/plugins').then((r) => setPlugins(r.plugins), () => {});

  async function toggle(p: PluginInfo, on: boolean, el: HTMLInputElement) {
    let approve: string | undefined;
    if (on && p.backend && !p.backend.approved) {
      asking = p;
      const ok = await confirmDialog(`Run ${p.name} on ${theHost()}?`, approval, 'Turn on');
      if (!ok) return void (el.checked = false);
      approve = p.backend.hash;
    }
    busy = p.id;
    try {
      const r = await api<{ plugins: PluginInfo[] }>('PUT', `/api/plugins/${encodeURIComponent(p.id)}`, { enabled: on, approve });
      setPlugins(r.plugins);
    } catch (e) {
      el.checked = !on;
      toast(e instanceof ApiError ? e.message : 'Couldn’t change the plugin.');
    } finally {
      busy = null;
    }
  }
</script>

{#snippet approval()}
  <p>This plugin starts a program on {theHost()} that can read and change your files:</p>
  <p><code class="plugin-command">{asking?.backend?.command.join(' ')}</code></p>
  <p>Only turn it on if you trust where it came from.</p>
{/snippet}

{#snippet row(p: PluginInfo)}
  <label class="switch-row">
    <input type="checkbox" role="switch" checked={p.enabled} disabled={busy === p.id || (!!p.problem && !p.enabled)} onchange={(e) => toggle(p, e.currentTarget.checked, e.currentTarget)} />
    <span>
      {p.name}{#if p.version}<small class="plugin-version">{` ${p.version}`}</small>{/if}
      <small>{p.description}</small>
      {#if p.backend}<small>Runs <code>{p.backend.command.join(' ')}</code> on {theHost()}.</small>{/if}
      {#if p.problem}<small class="error">{p.problem}</small>{/if}
      {#if !p.builtin && !p.problem && p.build === 'missing'}<small class="error">Not built yet. On {theHost()}, run <code>briardr plugin build {p.id}</code>.</small>{/if}
      {#if !p.builtin && !p.problem && p.build === 'stale'}<small class="error">Built for another version of Briardr. On {theHost()}, run <code>briardr plugin build {p.id}</code>.</small>{/if}
      {#if p.enabled && loadErrors.value[p.id]}<small class="error">Couldn’t load: {loadErrors.value[p.id]}</small>{/if}
    </span>
  </label>
{/snippet}

<Section id="plugins" title="Plugins">
  <p class="hint">Turned on or off for every device. Turning one off hides it and stops {theHost()} answering for it.</p>
  <div class="switches">
    {#each plugins.value.filter((p) => p.builtin) as p (p.id)}{@render row(p)}{/each}
  </div>
  <h3 class="set-sub">Installed</h3>
  {#if installed.length}
    <div class="switches">
      {#each installed as p (p.id)}{@render row(p)}{/each}
    </div>
  {:else}
    <p class="hint">None yet. To make one, run <code>briardr create plugin</code> on {theHost()}.</p>
  {/if}
</Section>
