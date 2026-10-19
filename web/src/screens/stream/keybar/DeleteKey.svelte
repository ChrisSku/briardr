<!-- Backspace: deletes on press, then repeats while held, faster the longer it's held. -->
<script lang="ts">
  import { Icon } from '../../../ui/icons/index.ts';
  import { capture } from './capture.ts';

  let { onPress }: { onPress: () => void } = $props();
  let timer: number | null = null;

  function stop() {
    if (timer) clearTimeout(timer);
    timer = null;
  }
  function repeat(n: number) {
    timer = window.setTimeout(
      () => {
        onPress();
        repeat(n + 1);
      },
      n === 0 ? 420 : Math.max(40, 110 - n * 6),
    );
  }
</script>

<button
  class="key key-delete"
  aria-label="Delete (Backspace). Hold to keep deleting."
  title="Delete (hold to keep deleting)"
  onpointerdown={(e) => {
    e.preventDefault(); // keep the on-screen keyboard up
    capture(e);
    stop();
    onPress();
    repeat(0);
  }}
  onpointerup={stop}
  onpointercancel={stop}
  onclick={(e) => e.detail === 0 && onPress() /* keyboard or assistive activation; taps were handled on pointerdown */}
>
  <Icon name="backspace" size={20} />
</button>
