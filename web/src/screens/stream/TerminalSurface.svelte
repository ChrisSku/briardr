<!--
  The live terminal surface.
  Layout: the terminal is sized to the visible area. When the on-screen keyboard opens, the surface
  ends on it: the key bar sits on the keyboard and, while controlling, the pane is resized to the rows
  above it, so a full-screen program (an agent with its input box at the bottom) lays itself out to
  fit. Until the new size arrives, the screen stays aligned to the bottom, keeping the input in view.
  If the Mac's screen is shorter than the view (watching), it's aligned to the bottom too.
  Its parts are in surface/: fingers, the mouse and the wheel (pointer.ts), files dropped on it
  (drop.svelte.ts), dictation (dictation.svelte.ts), who has control (OwnershipBanner.svelte), and the
  sheets over it (Sheets.svelte), most opened from the + menu or a paste (insert-menu.ts).
-->
<script lang="ts">
  import { onMount } from 'svelte';
  import { keyboardInset } from '../../keyboard.svelte.ts';
  import { hardwareKeyboard } from '../../store.ts';
  import { appOf } from '../../terminal/barkeys.ts';
  import type { TerminalController } from '../../terminal/controller.ts';
  import type { Outgoing } from '../../terminal/keys.ts';
  import type { Stream } from '../../types.ts';
  import KeyBar from './keybar/KeyBar.svelte';
  import { Dictation } from './surface/dictation.svelte.ts';
  import { FileDrop } from './surface/drop.svelte.ts';
  import { insertMenu, pasted } from './surface/insert-menu.ts';
  import OwnershipBanner from './surface/OwnershipBanner.svelte';
  import { terminalPointer } from './surface/pointer.ts';
  import Sheets, { type SurfaceSheet } from './surface/Sheets.svelte';

  interface Props {
    ctl: TerminalController;
    stream: Stream;
    visible: boolean;
    compact: boolean;
  }
  let { ctl, stream, visible, compact }: Props = $props();

  let host: HTMLDivElement;
  let screen: HTMLDivElement;
  let shift: HTMLDivElement;
  let wrap: HTMLDivElement | undefined = $state();
  const kb = keyboardInset();
  // The key bar's home-indicator padding isn't needed above the keyboard: the surface ends this far
  // above the bottom, so the bar sits directly on the keyboard and the terminal gets the rest.
  const lift = $derived.by(() => {
    if (!kb.current) return 0;
    const safeBottom = wrap ? parseFloat(getComputedStyle(wrap).paddingBottom) || 0 : 0;
    return Math.max(0, kb.current - safeBottom);
  });
  let sheet = $state.raw<SurfaceSheet | null>(null);
  const open = (s: SurfaceSheet) => (sheet = s);
  const st = $derived(ctl.status.value);
  const controlling = $derived(st.ownership === 'controlling');
  let resized = $state(0); // bumped when the terminal takes a new size (e.g. the Mac's, while watching)

  // The controller is this stream's for the surface's whole life (StreamScreen is keyed by stream).
  // svelte-ignore state_referenced_locally
  const pointer = terminalPointer(ctl, { keyBar: () => showKeyBar, open });
  // svelte-ignore state_referenced_locally
  const drop = new FileDrop(ctl, () => stream, (items) => open({ k: 'upload', items }));
  // svelte-ignore state_referenced_locally
  const dictation = new Dictation(ctl);

  // The on-screen keyboard showing means the touch screen is in use again, so the key bar comes back.
  $effect(() => {
    if ('ontouchstart' in window && kb.current > 0) hardwareKeyboard.value = false;
  });

  onMount(() => {
    ctl.mount(host, screen);
    const sizes = ctl.term.onResize(() => resized++);
    ctl.onPaste = (p) => pasted(ctl, p, open);
    // Physical keyboard detection on touch devices: a keydown while no on-screen keyboard is showing.
    const onKey = () => {
      if ('ontouchstart' in window && kb.current === 0 && document.activeElement === ctl.term.textarea) hardwareKeyboard.value = true;
    };
    window.addEventListener('keydown', onKey, true);
    return () => {
      sizes.dispose();
      ctl.onPaste = null;
      window.removeEventListener('keydown', onKey, true);
      pointer.dispose();
    };
  });

  // While watching, the screen has the Mac's size (we never resize it); scale it down to fit.
  $effect(() => {
    void [st, visible, resized];
    const inner = shift.querySelector<HTMLElement>('.xterm-screen');
    const scale = !controlling && inner && inner.offsetWidth > host.clientWidth ? host.clientWidth / inner.offsetWidth : 1;
    shift.style.transformOrigin = 'bottom left';
    shift.style.transform = scale < 1 ? `scale(${scale})` : '';
  });

  const isLive = $derived(controlling || st.ownership === 'watching');
  const showKeyBar = $derived(!hardwareKeyboard.value || compact);
</script>

<div class={['term-surface', !visible && 'is-hidden']} aria-hidden={!visible} style:bottom={lift ? `${lift}px` : undefined}>
  <!-- Pointer gestures and drops on the terminal; the keyboard goes to xterm's own input. -->
  <div class={['term-host', !isLive && 'is-stale', drop.over && 'is-drop']} bind:this={host} {...pointer.handlers} {...drop.handlers}>
    <div class="term-shift" bind:this={shift}>
      <div class="term-screen" bind:this={screen}></div>
    </div>
    {#if !isLive && (st.cachedAt || st.liveAt) && st.ownership !== 'connecting'}
      <div class="stale-tag" role="status">Last seen screen · not live</div>
    {/if}
  </div>
  <OwnershipBanner {ctl} {visible} />
  {#if showKeyBar}
    <div class={['keybar-wrap', kb.current && 'is-on-keyboard']} bind:this={wrap}>
      <KeyBar
        enabled={controlling}
        app={appOf(stream)}
        keyboardOpen={kb.current > 0}
        onKeyboard={() => (kb.current ? ctl.blur() : ctl.focus())}
        send={(o: Outgoing) => ctl.input(o)}
        onPlus={(anchor) => insertMenu(anchor, stream, open)}
        onDictateStart={dictation.start}
        onDictateEnd={dictation.end}
        dictating={dictation.active}
        level={dictation.level}
      />
    </div>
  {/if}
  <Sheets {ctl} {stream} {sheet} onClose={() => (sheet = null)} />
</div>
