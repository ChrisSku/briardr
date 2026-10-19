<!-- Drag from the centre to send arrows; holding past the edge repeats, faster the longer it's held. -->
<script lang="ts">
  import { Icon } from '../../../ui/icons/index.ts';
  import { capture } from './capture.ts';

  type Dir = 'up' | 'down' | 'left' | 'right';
  let { send }: { send: (dir: Dir) => void } = $props();
  let start: { x: number; y: number } | null = null;
  let dir: Dir | null = null;
  let timer: number | null = null;
  let active = $state<Dir | null>(null);
  const DIRS: Dir[] = ['up', 'down', 'left', 'right'];

  function stop() {
    if (timer) clearTimeout(timer);
    timer = null;
    dir = null;
    start = null;
    active = null;
  }
  function repeat(n: number) {
    if (!dir) return;
    send(dir);
    timer = window.setTimeout(() => repeat(n + 1), n === 0 ? 380 : Math.max(45, 130 - n * 8));
  }
</script>

<div class="arrowpad" role="group" aria-label="Arrow keys">
  <!-- A drag surface for touch; the hidden buttons below are its keyboard and assistive equivalent. -->
  <!-- svelte-ignore a11y_no_static_element_interactions -->
  <div
    class={['pad', active && 'pad-' + active]}
    aria-hidden="true"
    onpointerdown={(e) => {
      e.preventDefault();
      capture(e);
      start = { x: e.clientX, y: e.clientY };
    }}
    onpointermove={(e) => {
      if (!start) return;
      const dx = e.clientX - start.x;
      const dy = e.clientY - start.y;
      if (Math.hypot(dx, dy) < 12) return;
      const d: Dir = Math.abs(dx) > Math.abs(dy) ? (dx > 0 ? 'right' : 'left') : dy > 0 ? 'down' : 'up';
      if (d !== dir) {
        if (timer) clearTimeout(timer);
        dir = d;
        active = d;
        repeat(0);
      }
    }}
    onpointerup={stop}
    onpointercancel={stop}
  >
    <Icon name="arrows" size={24} />
  </div>
  {#each DIRS as d}
    <button class="sr-only" onclick={() => send(d)}>Arrow {d}</button>
  {/each}
</div>
