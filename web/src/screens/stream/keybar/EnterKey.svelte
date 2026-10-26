<!-- Enter: a tap sends Enter; holding it dictates until released (where the host transcribes speech). -->
<script lang="ts">
  import { caps } from '../../../host.ts';
  import { Icon } from '../../../ui/icons/index.ts';
  import { capture } from './capture.ts';

  interface Props {
    onTap: () => void;
    onHoldStart: () => void;
    onHoldEnd: (cancel: boolean) => void;
    dictating: boolean;
    level: number;
  }
  let { onTap, onHoldStart, onHoldEnd, dictating, level }: Props = $props();
  let t: number | null = null;
  let holding = false;
  let pressed = false;
  const canDictate = $derived(caps().dictation);
</script>

<button
  class={['key key-enter', dictating && 'is-dictating']}
  aria-label={canDictate ? 'Enter. Hold to dictate.' : 'Enter'}
  title={canDictate ? 'Enter (hold to dictate)' : 'Enter'}
  style:--level={dictating ? String(Math.min(1, level * 3)) : undefined}
  onpointerdown={(e) => {
    capture(e);
    holding = false;
    pressed = true;
    // Without dictation, a press of any length is a tap.
    if (canDictate)
      t = window.setTimeout(() => {
        t = null;
        pressed = false;
        holding = true;
        onHoldStart();
      }, 420);
  }}
  onpointerup={() => {
    if (pressed) {
      if (t) clearTimeout(t);
      t = null;
      pressed = false;
      onTap();
    } else if (holding) {
      holding = false;
      onHoldEnd(false);
    }
  }}
  onpointercancel={() => {
    if (t) clearTimeout(t);
    t = null;
    pressed = false;
    if (holding) {
      holding = false;
      onHoldEnd(true);
    }
  }}
  onkeydown={(e) => {
    if (e.key === 'Enter' || e.key === ' ') {
      e.preventDefault();
      onTap();
    }
  }}
>
  {#if dictating}<Icon name="mic" size={20} />{:else}<Icon name="enter" size={20} />{/if}
</button>
