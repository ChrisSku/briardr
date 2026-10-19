<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { AgentState } from '../../types.ts';
  import { Icon } from '../../ui/icons/index.ts';

  interface Props {
    title: string;
    sub?: string;
    state?: AgentState | null;
    onBack: () => void;
    /** Tapping the name (and the space after it) does this; the trailing buttons stay the accessible way in. */
    onTitle?: () => void;
    icon?: Snippet;
    trailing?: Snippet;
  }
  let { title, sub, state, onBack, onTitle, icon, trailing }: Props = $props();
</script>

<header class="screen-head stream-head">
  <button class="icon-btn back" aria-label="Back" onclick={onBack}>
    <Icon name="back" />
  </button>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="head-main" class:tappable={!!onTitle} onclick={onTitle}>
    {@render icon?.()}
    <div class="head-text">
      <h1 class={state ? `st-${state}` : undefined}>{title}</h1>
      {#if sub}<span class="head-sub">{sub}</span>{/if}
    </div>
  </div>
  <div class="head-trail">{@render trailing?.()}</div>
</header>
