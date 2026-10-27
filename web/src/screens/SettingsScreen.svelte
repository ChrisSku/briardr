<!--
  Settings: a grouped list. The computer Briardr runs on (with Power) first, then Usage (when its plugin
  is on), this device (devices, notifications, terminal, key bar, installing the app) and Briardr itself
  (plugins and their settings, diagnostics, about). Each row opens its section as a page at
  /settings/<section>, with a way back to the list.
-->
<script lang="ts">
  import { caps, HostLabel } from '../host.ts';
  import { api, ApiError } from '../api.ts';
  import { pluginContext } from '../plugins/context.ts';
  import { isOn, loadBuiltins, plugins, settingsSections } from '../plugins/registry.svelte.ts';
  import { pushStatus } from '../push.ts';
  import { navigate } from '../router.ts';
  import { prefs, toast } from '../store.ts';
  import { Icon, type IconName } from '../ui/icons/index.ts';
  import { ago } from '../ui/kit.ts';
  import AboutSection from './settings/AboutSection.svelte';
  import DevicesSection from './settings/DevicesSection.svelte';
  import DiagnosticsSection from './settings/DiagnosticsSection.svelte';
  import HerdrSection from './settings/HerdrSection.svelte';
  import HostCard from './settings/HostCard.svelte';
  import InstallSection from './settings/InstallSection.svelte';
  import KeyBarSection from './settings/KeyBarSection.svelte';
  import NotificationsSection from './settings/NotificationsSection.svelte';
  import PluginsSection from './settings/PluginsSection.svelte';
  import PowerSection from './settings/PowerSection.svelte';
  import Section from './settings/Section.svelte';
  import type { Settings, UpdateStatus } from './settings/settings.ts';
  import TerminalSection from './settings/TerminalSection.svelte';

  let { section }: { section?: string } = $props();

  const hasPower = $derived(caps().battery || caps().keepAwake || caps().sleep);
  let s = $state.raw<Settings | null>(null);
  let err = $state<string | null>(null);
  const reload = () => api<Settings>('GET', '/api/settings').then((r) => (s = r), (e) => (err = e instanceof ApiError ? e.message : 'Couldn’t load settings.'));
  reload();
  void loadBuiltins('settings');

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

  const open = (id: string) => navigate({ name: 'settings', section: id });
  const device = (() => {
    const ua = navigator.userAgent;
    if (/iPad/.test(ua) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1)) return 'iPad';
    if (/iPhone/.test(ua)) return 'iPhone';
    return 'device';
  })();

  // A plugin's section (Usage, installed plugins'), when this page is one.
  const plugin = $derived(settingsSections.value.find((x) => x.id === section));
  const TITLES: Record<string, string> = {
    devices: 'Devices',
    terminal: 'Terminal',
    keybar: 'Key bar',
    install: 'Install app',
    notifications: 'Notifications',
    plugins: 'Plugins',
    power: `${HostLabel()} power`,
    diagnostics: 'Diagnostics',
    about: 'About',
  };
  const title = $derived(section === 'herdr' ? HostLabel() : plugin ? plugin.label : section ? TITLES[section] : undefined);

  // ---------- the list's values ----------
  let devices = $state<number | null>(null);
  let update = $state<string | null>(null);
  let usage = $state.raw<Record<string, { report: Report | null; tapReport?: Report | null } | undefined> | null>(null);
  interface Report {
    windows: { label: string; usedPercent: number | null; resetsAt: number | null }[];
    capturedAt: number;
  }
  $effect(() => {
    if (section) return;
    api<{ devices: unknown[] }>('GET', '/api/devices').then((r) => (devices = r.devices.length), () => {});
    api<UpdateStatus>('GET', '/api/update').then((r) => (update = r.available ? r.latest : null), () => {});
    if (isOn('usage')) api<{ usage: typeof usage }>('GET', '/api/usage').then((r) => (usage = r.usage), () => {});
  });
  /** A provider's fullest window in its newest report, for its row. */
  function fullest(p: 'claude' | 'codex') {
    const x = usage?.[p];
    const rep = x?.report && x?.tapReport ? (x.tapReport.capturedAt > x.report.capturedAt ? x.tapReport : x.report) : x?.report || x?.tapReport;
    const now = Date.now();
    const w = rep?.windows.filter((w) => w.usedPercent !== null && !(w.resetsAt !== null && w.resetsAt <= now)).sort((a, b) => b.usedPercent! - a.usedPercent!)[0];
    return w ? { label: w.label, pct: Math.round(w.usedPercent!), at: rep!.capturedAt } : null;
  }
  const providers = $derived((['claude', 'codex'] as const).map((p) => ({ id: p, name: p === 'claude' ? 'Claude' : 'Codex', top: fullest(p) })));
  const usageAt = $derived(Math.max(0, ...providers.map((p) => p.top?.at ?? 0)));
  const tone = (pct: number) => (pct >= 90 ? 'tone-danger' : pct >= 70 ? 'tone-attention' : '');
  const push = $derived(pushStatus.value.state === 'on' ? 'On' : pushStatus.value.state === 'broken' ? 'Needs attention' : pushStatus.value.state === 'off' ? 'Off' : '');
  const pluginsOn = $derived(plugins.value.filter((p) => p.enabled).length);
  const others = $derived(settingsSections.value.filter((x) => x.id !== 'usage'));
  const version = $derived(s?.version ?? document.querySelector<HTMLMetaElement>('meta[name="briardr-release"]')?.content);
</script>

{#snippet row(id: string, icon: IconName, label: string, value?: string | null, cls?: string)}
  <a class="set-row" href="/settings/{id}" onclick={(e) => (e.preventDefault(), open(id))}>
    <span class={['set-tile', cls]}><Icon name={icon} size={17} /></span>
    <span class="set-label">{label}</span>
    {#if value}<span class="set-value">{value}</span>{/if}
    <Icon name="chevron" size={15} class="set-chev" />
  </a>
{/snippet}

<div class="screen">
  {#if section}
    <div class="settings-back">
      <a href="/settings" onclick={(e) => (e.preventDefault(), navigate({ name: 'settings' }))}><Icon name="back" size={20} />Settings</a>
    </div>
  {/if}
  <header class="screen-head">
    <h1>{title ?? 'Settings'}</h1>
  </header>
  <div class="screen-body settings">
    {#if err}<p class="error">{err}</p>{/if}
    {#if !section}
      <HostCard />
      {#if isOn('usage')}
        <section class="set-group" id="set-usage" aria-labelledby="set-usage-h">
          <h2 class="section-head">
            <span class="overline" id="set-usage-h">Usage</span>
            {#if usageAt}<span class="section-trail">{ago(Date.now() - usageAt)}</span>{/if}
          </h2>
          <div class="card">
            {#each providers as p}
              <a class="set-row usage-row" href="/settings/usage" onclick={(e) => (e.preventDefault(), open('usage'))}>
                <span class="set-tile"><Icon name={p.id} size={16} /></span>
                <span class="usage-row-main">
                  <span class="usage-row-top">
                    <span class="set-label">{p.name}</span>
                    <span class={['set-value', 'usage-row-value', p.top && tone(p.top.pct)]}>{p.top ? `${p.top.label} ${p.top.pct}%` : 'Not set up'}</span>
                  </span>
                  {#if p.top}
                    <span class="meter small" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={p.top.pct} aria-label="{p.name} {p.top.label} {p.top.pct} percent used">
                      <span class={['meter-fill', tone(p.top.pct)]} style:width="{Math.min(100, p.top.pct)}%"></span>
                    </span>
                  {/if}
                </span>
                <Icon name="chevron" size={15} class="set-chev" />
              </a>
            {/each}
          </div>
        </section>
      {/if}
      <section class="set-group" aria-labelledby="set-device-h">
        <h2 class="section-head"><span class="overline" id="set-device-h">This {device}</span></h2>
        <div class="card">
          {@render row('devices', 'device', 'Devices', devices === null ? null : String(devices))}
          {@render row('notifications', 'bell', 'Notifications', push)}
          {@render row('terminal', 'terminal', 'Terminal', `${prefs.value.fontSize} pt`)}
          {@render row('keybar', 'keyboard', 'Key bar')}
          {@render row('install', 'download', 'Install app')}
        </div>
      </section>
      <section class="set-group" aria-labelledby="set-briardr-h">
        <h2 class="section-head"><span class="overline" id="set-briardr-h">Briardr</span></h2>
        <div class="card">
          {@render row('plugins', 'plug', 'Plugins', `${pluginsOn} on`)}
          {#each others as x (x.plugin + ':' + x.id)}
            {@render row(x.id, 'plug', x.label)}
          {/each}
          {@render row('diagnostics', 'warning', 'Diagnostics')}
          {@render row('about', 'dog', 'About', update ? `${update} available` : version, 'brand')}
        </div>
      </section>
    {:else if section === 'herdr'}
      <HostCard />
      <HerdrSection {s} {save} />
    {:else if section === 'devices'}
      <DevicesSection />
    {:else if section === 'terminal'}
      <TerminalSection />
    {:else if section === 'keybar'}
      <KeyBarSection />
    {:else if section === 'install'}
      <InstallSection />
    {:else if section === 'notifications'}
      <NotificationsSection {s} {save} />
    {:else if section === 'plugins'}
      <PluginsSection />
    {:else if section === 'power' && hasPower}
      <PowerSection {s} />
    {:else if section === 'diagnostics'}
      <DiagnosticsSection />
    {:else if section === 'about'}
      <AboutSection {s} {save} />
    {:else if plugin}
      <Section id={plugin.id} title={plugin.label}>
        {#if plugin.component}<plugin.component briardr={pluginContext(plugin.plugin)} {s} {save} />{/if}
      </Section>
    {:else}
      <p class="hint">There’s no such section. <a href="/settings" onclick={(e) => (e.preventDefault(), navigate({ name: 'settings' }))}>Back to Settings</a></p>
    {/if}
  </div>
</div>
