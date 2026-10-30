<!--
  App shell. Compact (iPhone, narrow windows): one column with a bottom tab bar (Agents, Workspaces,
  Settings) that hides while a stream is open. Regular (iPad, Mac): a sidebar with Agents and Workspaces,
  Jump, and Settings as a gear in the corner; with a stream open, the agent list or workspace tree stays
  in view beside it, next to the sidebar when the window is wide enough (else with its own switch). A
  stream in full screen hides both.
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { pluginContext } from './plugins/context.ts';
  import { appTabs, loadBuiltins, streamTabs } from './plugins/registry.svelte.ts';
  import { navigate, route, type Route } from './router.ts';
  import AgentsList from './screens/AgentsList.svelte';
  import AgentsScreen from './screens/AgentsScreen.svelte';
  import ConnectionBar from './screens/ConnectionBar.svelte';
  import UpdateBar from './screens/UpdateBar.svelte';
  import JumpHost, { openJump } from './screens/Jump.svelte';
  import PairScreen from './screens/Pair.svelte';
  import SettingsScreen from './screens/SettingsScreen.svelte';
  import { streamScreen } from './screens/lazy.svelte.ts';
  import WorkspacesScreen from './screens/WorkspacesScreen.svelte';
  import WorkspaceTree from './screens/WorkspaceTree.svelte';
  import { banners, dismissBanner, fullScreen, herdr, listPane, model, session, toasts, topOverlay } from './store.ts';
  import { groupOf } from './types.ts';
  import { Icon, iconName, type IconName } from './ui/icons/index.ts';
  import { ConfirmHost, Empty, MenuHost } from './ui/kit.ts';


  const query = matchMedia('(max-width: 759px)');
  let compact = $state(query.matches);
  // Wide enough for the sidebar beside an open stream's list.
  const wideQuery = matchMedia('(min-width: 1200px)');
  let wide = $state(wideQuery.matches);
  // The release this page belongs to (package.json's version, written in by scripts/build.ts).
  const version = document.querySelector<HTMLMetaElement>('meta[name="briardr-release"]')?.content;
  const r = $derived(route.value);
  const s = $derived(session.value);
  const inStream = $derived(r.name === 'stream');
  const attention = $derived((model.value?.streams || []).filter((x) => groupOf(x) === 'attention').length);
  interface NavTab {
    route: Route;
    label: string;
    icon: IconName;
  }
  // Installed plugins' app tabs sit after Workspaces. Settings ends the tab bar; on iPad and Mac it's
  // the gear in the sidebar's corner.
  const places: NavTab[] = $derived([
    { route: { name: 'agents' }, label: 'Agents', icon: 'agents' },
    { route: { name: 'workspaces' }, label: 'Workspaces', icon: 'workspaces' },
    ...appTabs.value.map((t): NavTab => ({ route: { name: 'app', tab: t.id }, label: t.label, icon: iconName(t.icon) })),
  ]);
  const tabs: NavTab[] = $derived([...places, { route: { name: 'settings' }, label: 'Settings', icon: 'settings' }]);
  const isOn = (t: NavTab) => r.name === t.route.name && (r.name !== 'app' || (t.route.name === 'app' && r.tab === t.route.tab));
  // Beside a stream, the sidebar picks the list next to it instead of leaving the stream.
  const sideOn = (t: NavTab) => (inStream ? t.route.name === listPane.value : isOn(t));
  const sidePick = (t: NavTab) => (inStream && (t.route.name === 'agents' || t.route.name === 'workspaces') ? (listPane.value = t.route.name) : navigate(t.route));
  const full = $derived(inStream && fullScreen.value);
  const showSidebar = $derived(!compact && !full && (!inStream || wide));
  const appTab = $derived(r.name === 'app' ? appTabs.value.find((t) => t.id === r.tab) : undefined);
  $effect(() => {
    if (appTab && !appTab.component) void loadBuiltins('appTabs');
  });
  $effect(() => {
    if (inStream && !streamScreen.component.value) void streamScreen.load();
  });
  $effect(() => {
    if (!inStream) fullScreen.value = false;
  });

  onMount(() => {
    const onChange = () => ((compact = query.matches), (wide = wideQuery.matches));
    query.addEventListener('change', onChange);
    wideQuery.addEventListener('change', onChange);
    // Keyboard shortcuts read the current route when pressed.
    const onKey = (e: KeyboardEvent) => {
      const cur = route.value;
      if ((e.metaKey || e.ctrlKey) && e.key.toLowerCase() === 'k') {
        e.preventDefault();
        openJump();
      } else if (e.metaKey && cur.name === 'stream' && /^[1-9]$/.test(e.key)) {
        // ⌘1 is the terminal, then the plugins' tabs in order.
        const surface = ['terminal', ...streamTabs.value.map((t) => t.id)][Number(e.key) - 1];
        if (!surface) return;
        e.preventDefault();
        navigate({ name: 'stream', id: cur.id, surface }, { replace: true });
      } else if (e.metaKey && e.key === '[' && cur.name === 'stream') {
        e.preventDefault();
        history.back();
      } else if (e.metaKey && e.shiftKey && e.key === 'Enter' && cur.name === 'stream') {
        e.preventDefault();
        fullScreen.value = !fullScreen.value;
      }
    };
    window.addEventListener('keydown', onKey);
    return () => {
      query.removeEventListener('change', onChange);
      wideQuery.removeEventListener('change', onChange);
      window.removeEventListener('keydown', onKey);
    };
  });

</script>

{#if s.status === 'unknown'}
  <div class="splash" aria-busy="true"></div>
{:else if s.status === 'signed_out'}
  <PairScreen reason={s.reason} />
{:else}
  <div class={['app', compact ? 'is-compact' : 'is-regular', inStream && 'in-stream', full && 'is-full']}>
    <a class="skip" href="#main">Skip to content</a>
    <!-- Banners float over the header instead of pushing the screen down. -->
    <div class="top-overlay">
      <ConnectionBar />
      <UpdateBar />
      <div class="top-overlay-slot">{@render topOverlay.value?.()}</div>
      {#if banners.value.length}
        <div class="banners" role="region" aria-label="Alerts" aria-live="assertive">
          {#each banners.value as b}
            <div class="banner banner-{b.kind}">
              <button
                class="banner-main"
                onclick={() => {
                  dismissBanner(b.id);
                  if (b.streamId) navigate({ name: 'stream', id: b.streamId, surface: 'terminal' });
                }}
              >
                <strong>{b.title}</strong>
                {#if b.body}<span>{b.body}</span>{/if}
              </button>
              <button class="icon-btn" aria-label="Dismiss" onclick={() => dismissBanner(b.id)}>
                <Icon name="close" size={16} />
              </button>
            </div>
          {/each}
        </div>
      {/if}
    </div>
    <div class="app-body">
      {#if showSidebar}
        <nav class="sidebar" aria-label="Main">
          <div class="sidebar-brand"><span class="brand-tile" aria-hidden="true"><img src="/icons/briard.png" alt="" /></span>Briardr</div>
          {#each places as t}
            <button class={['side-item', sideOn(t) && 'is-on']} aria-current={isOn(t) ? 'page' : undefined} onclick={() => sidePick(t)}>
              <Icon name={t.icon} size={18} />
              <span>{t.label}</span>
              {#if t.route.name === 'agents' && attention > 0}<span class="cnt tone-attention" aria-label="{attention} need you">{attention}</span>{/if}
            </button>
          {/each}
          <div class="sidebar-foot">
            <button class="side-jump" onclick={() => openJump()}>
              <Icon name="search" size={16} />
              <span>Jump to stream</span>
              <kbd>⌘K</kbd>
            </button>
            <div class="side-settings">
              <button class={['icon-btn', r.name === 'settings' && 'is-on']} aria-label="Settings" aria-current={r.name === 'settings' ? 'page' : undefined} onclick={() => navigate({ name: 'settings' })}>
                <Icon name="settings" size={18} />
              </button>
              <span class="side-version">{[`Briardr ${version}`, herdr.value?.version && `Herdr ${herdr.value.version}`].filter(Boolean).join(' · ')}</span>
            </div>
          </div>
        </nav>
      {/if}
      {#if !compact && inStream && !full}
        <!-- Beside an open stream on iPad and Mac: agents or the workspace tree. -->
        <aside class="list-pane" aria-label={listPane.value === 'agents' ? 'Agents' : 'Workspaces'}>
          {#if showSidebar}
            <h2 class="list-title">{listPane.value === 'agents' ? 'Agents' : 'Workspaces'}</h2>
          {:else}
            <div class="segmented list-switch" role="tablist">
              <button role="tab" aria-selected={listPane.value === 'agents'} class={listPane.value === 'agents' ? 'is-on' : ''} onclick={() => (listPane.value = 'agents')}>Agents</button>
              <button role="tab" aria-selected={listPane.value === 'workspaces'} class={listPane.value === 'workspaces' ? 'is-on' : ''} onclick={() => (listPane.value = 'workspaces')}>Workspaces</button>
            </div>
          {/if}
          <div class="list-pane-body">
            {#if listPane.value === 'agents'}<AgentsList compact />{:else}<WorkspaceTree compact />{/if}
          </div>
        </aside>
      {/if}
      <main id="main" class="main" tabindex="-1">
        {#if r.name === 'stream'}
          {#if streamScreen.component.value}
            {@const StreamScreen = streamScreen.component.value}
            {#key r.id}
              <StreamScreen id={r.id} surface={r.surface} {compact} />
            {/key}
          {:else if streamScreen.failed.value}
            <div class="screen">
              <Empty title="Couldn’t open the stream">Briardr couldn’t load the terminal. <button class="btn btn-small" onclick={() => streamScreen.load()}>Try again</button></Empty>
            </div>
          {:else}
            <div class="screen" aria-busy="true"></div>
          {/if}
        {:else if r.name === 'workspaces'}
          <WorkspacesScreen />
        {:else if appTab}
          {#if appTab.component}<appTab.component {compact} briardr={pluginContext(appTab.plugin)} />{/if}
        {:else if r.name === 'settings'}
          <SettingsScreen section={r.section} />
        {:else}
          <AgentsScreen />
        {/if}
      </main>
    </div>
    {#if compact && !inStream}
      <nav class="tabbar" aria-label="Main">
        {#each tabs as t}
          <button class={isOn(t) ? 'is-on' : ''} aria-current={isOn(t) ? 'page' : undefined} onclick={() => navigate(t.route)}>
            <span class="tab-icon">
              <Icon name={t.icon} size={24} />
              {#if t.route.name === 'agents' && attention > 0}<span class="cnt tone-attention tab-badge" aria-label="{attention} need you">{attention}</span>{/if}
            </span>
            <span class="tab-label">{t.label}</span>
          </button>
        {/each}
      </nav>
    {/if}
    <JumpHost />
    <MenuHost />
    <ConfirmHost />
    <div class="toasts" role="status" aria-live="polite">
      {#each toasts.value as t}
        <div class="toast">{t.text}</div>
      {/each}
    </div>
  </div>
{/if}
