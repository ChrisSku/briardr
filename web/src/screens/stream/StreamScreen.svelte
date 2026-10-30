<!--
  A stream: the Terminal, then the tabs of the plugins that are on (Files, Changes, Preview, Docs, Notes,
  installed plugins). The terminal stays connected while switching surfaces. Leaving, locking, or backgrounding releases control; opening or returning to it resumes by
  itself (never by taking over: if another device controls the pane now, this one watches).
  In full screen the header and tabs hide, and a button floating at the top right brings them back.
  App keys this screen by stream ID, so each stream gets its own instance and controller.
-->
<script lang="ts">
  import { isMac, theHost } from '../../host.ts';
  import { onMount, untrack } from 'svelte';
  import { record } from '../../diag.ts';
  import { navigate, openedFromAlert, type Surface } from '../../router.ts';
  import { clockSkew, fullScreen, markOpened, model, now, prefs, toast, visibleStreamId } from '../../store.ts';
  import { TerminalController } from '../../terminal/controller.ts';
  import { isLocalhostUrl, isWebUrl } from '../../terminal/links.ts';
  import { streamContext } from '../../plugins/context.ts';
  import { linkHandlers, loadBuiltins, streamTabs } from '../../plugins/registry.svelte.ts';
  import type { Stream } from '../../types.ts';
  import { Icon, iconName, ToolIcon } from '../../ui/icons/index.ts';
  import { Empty, openMenu, since } from '../../ui/kit.ts';
  import { openJump } from '../Jump.svelte';
  import { markSeen } from '../streams.ts';
  import HistorySheet from './HistorySheet.svelte';
  import InfoSheet from './InfoSheet.svelte';
  import RenameSheet from './RenameSheet.svelte';
  import StreamHeader from './StreamHeader.svelte';
  import TerminalSurface, { confirmTakeOver } from './TerminalSurface.svelte';

  let { id, surface, compact }: { id: string; surface: Surface; compact: boolean } = $props();

  const tabs = $derived([{ id: 'terminal', label: 'Terminal', icon: 'terminal' }, ...streamTabs.value]);
  // A tab whose plugin is off (or a stale link) shows the terminal.
  const shown = $derived(tabs.some((t) => t.id === surface) ? surface : 'terminal');

  const stream = $derived(model.value?.streams.find((s) => s.id === id) || null);
  // A stream that disappears from the model keeps showing as it was last seen.
  let lastKnown: Stream | null = null;
  const s = $derived.by(() => {
    if (stream) lastKnown = stream;
    return stream || lastKnown;
  });
  let sheet = $state<null | 'rename' | 'info' | 'history'>(null);

  // svelte-ignore state_referenced_locally
  const ctl = new TerminalController({
    streamId: id,
    mode: 'control',
    fontSize: prefs.value.fontSize,
    screenReader: prefs.value.screenReaderTerminal,
    onLink: async (url) => {
      // The plugins that handle links (Preview) load with the stream; a tap this early waits for them.
      await loadBuiltins('streamTabs');
      const st = model.value?.streams.find((x) => x.id === id);
      if (st && linkHandlers.value.some((h) => h.fn(url, st, briardrFor(h.plugin)))) return;
      if (isLocalhostUrl(url)) toast('Turn on the Preview plugin in Settings → Plugins to open localhost links.');
      else if (isWebUrl(url)) window.open(url, '_blank', 'noopener,noreferrer');
      else toast('Briardr only opens web links. Hold the link to copy it.');
    },
    onLongPress: () => {},
    onNotice: (t) => toast(t),
  });

  /** Each plugin's context for this stream, made once. */
  const contexts = new Map<string, ReturnType<typeof streamContext>>();
  function briardrFor(plugin: string) {
    let k = contexts.get(plugin);
    if (!k) contexts.set(plugin, (k = streamContext(plugin, id, ctl)));
    return k;
  }

  // Attach on open, including a stream restored after a relaunch.
  onMount(() => {
    void loadBuiltins('streamTabs');
    visibleStreamId.value = id;
    markOpened(id);
    ctl.connect('control', false);
    if (openedFromAlert.value) openedFromAlert.value = false;
    let away = false;
    const onVisibility = () => {
      if (document.visibilityState === 'hidden' && ctl.status.value.ownership !== 'idle') {
        // Locking or backgrounding releases control; the work keeps running on the Mac.
        ctl.suspend('background');
        away = true;
      } else if (document.visibilityState === 'visible' && away) {
        away = false;
        record('term_resume', {});
        ctl.connect('control', false);
      }
    };
    const onPageHide = () => {
      ctl.suspend('pagehide');
      away = true;
    };
    // Restored from the back-forward cache (iOS keeps suspended web apps this way).
    const onPageShow = (e: PageTransitionEvent) => e.persisted && onVisibility();
    document.addEventListener('visibilitychange', onVisibility);
    window.addEventListener('pagehide', onPageHide);
    window.addEventListener('pageshow', onPageShow);
    return () => {
      document.removeEventListener('visibilitychange', onVisibility);
      window.removeEventListener('pagehide', onPageHide);
      window.removeEventListener('pageshow', onPageShow);
      visibleStreamId.value = null;
      ctl.dispose(); // releases control now; frees the terminal after the next screen has painted
    };
  });

  const fontSize = $derived(prefs.value.fontSize);
  $effect(() => ctl.setFontSize(fontSize));

  // Opening a finished stream counts as seeing it (on this device only).
  const finished = $derived(stream?.state === 'done' ? stream.stateChangeSeq : undefined);
  $effect(() => {
    if (finished !== undefined) untrack(() => stream && markSeen(stream));
  });

  const st = $derived(ctl.status.value);

  // Under the name: the agent and where it runs ("Claude · web/auth"), and on iPad and Mac for how long.
  const TOOL: Record<string, string> = { claude: 'Claude', codex: 'Codex' };
  const sub = $derived.by(() => {
    if (!s) return '';
    const m = model.value;
    const place = [m?.workspaces.find((w) => w.id === s.workspaceId)?.name, m?.projects.find((p) => p.id === s.projectId)?.name].filter(Boolean).join('/');
    const time = !compact && s.state && s.stateSince ? since(now.value + clockSkew.value - s.stateSince) : '';
    return [TOOL[s.tool.toLowerCase()] ?? s.tool, place, time].filter(Boolean).join(' · ');
  });

  // Also offered by the watching banner, which can be dismissed.
  const takeControl = () => (st.watchReason === 'busy' || st.watchReason === 'taken_over' ? confirmTakeOver(ctl) : ctl.connect('control', false));

  const actions = (el: HTMLElement) =>
    openMenu(
      [
        { label: 'Rename', onSelect: () => (sheet = 'rename') },
        { label: 'Info', onSelect: () => (sheet = 'info') },
        { label: 'History', onSelect: () => (sheet = 'history') },
        { label: 'Jump to stream…', hint: '⌘K', onSelect: () => openJump() },
        { label: 'Full screen', hint: '⇧⌘↩', onSelect: () => (fullScreen.value = true) },
        st.ownership === 'watching'
          ? { label: 'Take control', onSelect: () => void takeControl() }
          : { label: `Hand back to ${isMac() ? 'Mac' : theHost()}`, hint: 'Release control', disabled: st.ownership !== 'controlling', onSelect: () => ctl.handBack() },
      ],
      el,
      s!.name,
    );
</script>

{#if !s}
  <div class="screen">
    <StreamHeader title="Stream" onBack={() => navigate({ name: 'agents' })} />
    <Empty title="This stream isn’t available">It may have closed on {theHost()}, or Briardr hasn’t reached {theHost()} yet.</Empty>
  </div>
{:else}
  <!-- The banner under the header explains watching and reconnecting; data-ownership is for tests. -->
  <div class="screen stream-screen" data-ownership={st.ownership}>
    {#if !fullScreen.value}
      <StreamHeader title={s.name} {sub} state={s.state} {compact} onBack={() => navigate({ name: 'agents' })} onTitle={() => openJump()}>
        {#snippet icon()}
          <ToolIcon tool={s.tool} kind={s.kind} state={s.state} />
        {/snippet}
        {#snippet trailing()}
          <button class={['icon-btn', !compact && 'round']} aria-label="Jump to a stream" onclick={() => openJump()}>
            <Icon name="search" size={compact ? 20 : 18} />
          </button>
          <button class={['icon-btn round', compact && 'tinted']} aria-label="Stream actions" onclick={(e) => actions(e.currentTarget)}>
            <Icon name="more" size={18} />
          </button>
        {/snippet}
      </StreamHeader>
      <nav aria-label="Stream surfaces">
        <div class="surface-tabs" role="tablist">
          {#each tabs as x (x.id)}
            <button role="tab" aria-selected={shown === x.id} class={shown === x.id ? 'is-on' : ''} onclick={() => navigate({ name: 'stream', id, surface: x.id }, { replace: true })}>
              <Icon name={iconName(x.icon)} size={18} />
              <span>{x.label}</span>
            </button>
          {/each}
        </div>
      </nav>
    {/if}
    <div class="surfaces">
      <TerminalSurface {ctl} stream={s} visible={shown === 'terminal'} {compact} />
      {#each streamTabs.value as t (t.plugin + ':' + t.id)}
        {#if t.component}<t.component stream={s} visible={shown === t.id} {compact} briardr={briardrFor(t.plugin)} />{/if}
      {/each}
    </div>
    {#if fullScreen.value}
      <button class="icon-btn full-exit" aria-label="Exit full screen" title="Exit full screen" onclick={() => (fullScreen.value = false)}>
        <Icon name="minimize" size={18} />
      </button>
    {/if}
    {#if sheet === 'rename'}<RenameSheet stream={s} onClose={() => (sheet = null)} />{/if}
    {#if sheet === 'info'}<InfoSheet stream={s} {ctl} onClose={() => (sheet = null)} />{/if}
    {#if sheet === 'history'}<HistorySheet stream={s} onClose={() => (sheet = null)} />{/if}
  </div>
{/if}
