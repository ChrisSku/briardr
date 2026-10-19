<!-- Peek: a live, read-only view that never resizes the Mac's pane. -->
<script lang="ts">
  import { onMount } from 'svelte';
  import { prefs, toast } from '../store.ts';
  import { TerminalController } from '../terminal/controller.ts';
  import type { Stream } from '../types.ts';
  import { Sheet } from '../ui/kit.ts';
  import { openStream } from './streams.ts';

  let { stream, onClose }: { stream: Stream; onClose: () => void } = $props();
  let host: HTMLDivElement;
  let screen: HTMLDivElement;
  let ctl = $state.raw<TerminalController | null>(null);
  let resized = $state(0); // bumped when the Mac's screen size reaches the terminal

  onMount(() => {
    const c = new TerminalController({
      streamId: stream.id,
      mode: 'observe',
      fontSize: prefs.value.fontSize,
      screenReader: prefs.value.screenReaderTerminal,
      onLink: () => {},
      onLongPress: () => {},
      onNotice: (t) => toast(t),
    });
    c.mount(host, screen);
    c.connect('observe');
    ctl = c;
    const sub = c.term.onResize(() => resized++);
    return () => {
      sub.dispose();
      c.dispose();
    };
  });

  const st = $derived(ctl?.status.value);
  // Scale the Mac-sized screen down to fit, never asking the Mac to change size. Measured again when
  // the terminal takes the Mac's size, which can be after the status says it's live.
  $effect(() => {
    void resized;
    if (!st?.width) return;
    const inner = screen.querySelector<HTMLElement>('.xterm-screen');
    if (!inner) return;
    const k = Math.min(1, host.clientWidth / inner.offsetWidth, host.clientHeight / inner.offsetHeight);
    screen.style.transform = `scale(${k})`;
  });
</script>

<Sheet title="Peek: {stream.name}" {onClose} wide class="peek">
  <div class="peek-host" bind:this={host}>
    <div class="peek-screen" bind:this={screen}></div>
  </div>
  {#snippet footer()}
    <span class="peek-state">{st?.ownership === 'watching' ? 'Watching live · read-only' : st?.ownership === 'reconnecting' ? 'Reconnecting…' : 'Connecting…'}</span>
    <button
      class="btn btn-primary"
      onclick={() => {
        onClose();
        openStream(stream);
      }}
    >
      Open
    </button>
  {/snippet}
</Sheet>
