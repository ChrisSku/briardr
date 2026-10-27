<!--
  Settings: the Mac (its battery and Herdr), Herdr path, signed-in devices, terminal, the key bar, installing the app, notifications, plugins and their settings (Usage),
  Mac power, diagnostics, certificates, and About.
-->
<script lang="ts">
  import { caps } from '../host.ts';
  import { api, ApiError } from '../api.ts';
  import { navigate } from '../router.ts';
  import { toast } from '../store.ts';
  import AboutSection from './settings/AboutSection.svelte';
  import DevicesSection from './settings/DevicesSection.svelte';
  import DiagnosticsSection from './settings/DiagnosticsSection.svelte';
  import HerdrSection from './settings/HerdrSection.svelte';
  import InstallSection from './settings/InstallSection.svelte';
  import KeyBarSection from './settings/KeyBarSection.svelte';
  import MacSummary from './MacSummary.svelte';
  import NotificationsSection from './settings/NotificationsSection.svelte';
  import PowerSection from './settings/PowerSection.svelte';
  import type { Settings } from './settings/settings.ts';
  import TerminalSection from './settings/TerminalSection.svelte';
  import PluginsSection from './settings/PluginsSection.svelte';
  import Section from './settings/Section.svelte';
  import { pluginContext } from '../plugins/context.ts';
  import { loadBuiltins, settingsSections } from '../plugins/registry.svelte.ts';

  let { section }: { section?: string } = $props();

  const sections = $derived([
    ['herdr', 'Herdr'],
    ['devices', 'Devices'],
    ['terminal', 'Terminal'],
    ['keybar', 'Key bar'],
    ['install', 'Install app'],
    ['notifications', 'Notifications'],
    ['plugins', 'Plugins'],
    ...settingsSections.value.map((x) => [x.id, x.label]),
    ['power', 'Mac power'],
    ['diagnostics', 'Diagnostics'],
    ['about', 'About'],
  ]);

  const hasPower = $derived(caps().battery || caps().keepAwake || caps().sleep);
  let s = $state.raw<Settings | null>(null);
  let err = $state<string | null>(null);
  const reload = () => api<Settings>('GET', '/api/settings').then((r) => (s = r), (e) => (err = e instanceof ApiError ? e.message : 'Couldn’t load settings.'));
  reload();
  void loadBuiltins('settings');

  // Scroll to the requested section, again once the settings and the plugins' sections have loaded.
  const loaded = $derived(!!s && settingsSections.value.every((x) => x.component));
  $effect(() => {
    loaded;
    if (section) document.getElementById(`set-${section}`)?.scrollIntoView({ block: 'start' });
  });

  async function save(patch: object) {
    try {
      await api('PUT', '/api/settings', patch);
      await reload();
      return true;
    } catch (e) {
      toast(e instanceof ApiError ? e.message : 'Couldn’t save.');
      return false;
    }
  }
</script>

<div class="screen">
  <header class="screen-head">
    <h1>Settings</h1>
  </header>
  <div class="screen-body settings">
    <nav class="settings-index" aria-label="Settings sections">
      {#each sections.filter(([id]) => id !== 'power' || hasPower) as [id, label]}
        <a href="/settings/{id}" onclick={(e) => (e.preventDefault(), navigate({ name: 'settings', section: id }, { replace: true }))}>{label}</a>
      {/each}
    </nav>
    {#if err}<p class="error">{err}</p>{/if}
    <MacSummary />
    <HerdrSection {s} {save} />
    <DevicesSection />
    <TerminalSection />
    <KeyBarSection />
    <InstallSection />
    <NotificationsSection {s} {save} />
    <PluginsSection />
    {#each settingsSections.value as x (x.plugin + ':' + x.id)}
      <Section id={x.id} title={x.label}>
        {#if x.component}<x.component briardr={pluginContext(x.plugin)} {s} {save} />{/if}
      </Section>
    {/each}
    {#if hasPower}<PowerSection {s} />{/if}
    <DiagnosticsSection />
    <AboutSection {s} />
  </div>
</div>
