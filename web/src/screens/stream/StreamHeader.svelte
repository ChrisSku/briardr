<!-- A stream's header: back, the name with its state and where it runs under it, then the actions. On iPad
     and Mac the avatar leads and the state is a Filled badge. -->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { AgentState } from '../../types.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import { StateBadge } from '../../ui/kit.ts';

  interface Props {
    title: string;
    sub?: string;
    state?: AgentState | null;
    /** iPhone: Dot badge, no avatar. */
    compact?: boolean;
    onBack: () => void;
    /** Tapping the name (and the space after it) does this; the trailing buttons stay the accessible way in. */
    onTitle?: () => void;
    icon?: Snippet;
    trailing?: Snippet;
  }
  let { title, sub, state, compact = false, onBack, onTitle, icon, trailing }: Props = $props();
</script>

<header class="screen-head stream-head">
  <button class="icon-btn back" aria-label="Back" onclick={onBack}>
    <Icon name="back" />
  </button>
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="head-main" class:tappable={!!onTitle} onclick={onTitle}>
    {#if !compact}{@render icon?.()}{/if}
    <div class="head-text">
      <h1>{title}</h1>
      {#if state || sub}
        <div class="head-sub">
          <StateBadge state={state ?? null} filled={!compact} />
          {#if sub}<span>{state && compact ? '· ' : ''}{sub}</span>{/if}
        </div>
      {/if}
    </div>
  </div>
  <div class="head-trail">{@render trailing?.()}</div>
</header>
