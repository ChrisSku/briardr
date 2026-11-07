<!-- A stream's header: back, one line with the state's dot, the name in the state's colour and where it runs after it, then the
     actions. On iPad and Mac the avatar leads. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { STATE_LABEL, type AgentState } from '../../types.ts';
  import { Icon } from '../../ui/icons/index.ts';
  
  interface Props {
    title: string;
    sub?: string;
    state?: AgentState | null;
    /** iPhone: no avatar. */
    compact?: boolean;
    onBack: () => void;
    /** Tapping the name (and the space after it) does this; the trailing buttons stay the accessible way in. */
    onTitle?: () => void;
    /** After the back button. */
    leading?: Snippet;
    icon?: Snippet;
    trailing?: Snippet;
  }
  let { title, sub, state, compact = false, onBack, onTitle, leading, icon, trailing }: Props = $props();
</script>

<header class="screen-head stream-head">
  <button class="icon-btn back" aria-label="Back" onclick={onBack}>
    <Icon name="back" />
  </button>
  {@render leading?.()}
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="head-main" class:tappable={!!onTitle} onclick={onTitle}>
    {#if !compact}{@render icon?.()}{/if}
    <div class="head-text">
      <h1 class={state && `st-${state}`}>
        {#if state}<span class={['dot', state === 'working' && 'is-pulsing']} aria-hidden="true"></span><span class="sr-only">{STATE_LABEL[state]}: </span>{/if}<span class="head-name">{title}</span>
        {#if sub}<span class="head-sub">{sub}</span>{/if}
      </h1>
    </div>
  </div>
  <div class="head-trail">{@render trailing?.()}</div>
</header>
