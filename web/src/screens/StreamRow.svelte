<!--
  A stream in a list: its avatar with the state dot, title, where it runs, and how long it has been in
  its state. In a list of one state (Agents) the time says it; elsewhere a Dot badge sits above it.
  Actions: "…" on hover with a pointer, a long press, or on touch a swipe to the left (Peek, More).
-->
<script lang="ts">
  import { clockSkew, now, visibleStreamId } from '../store.ts';
  import { STATE_LABEL, type Stream } from '../types.ts';
  import { Icon, ToolIcon } from '../ui/icons/index.ts';
  import { longPress, openMenu, since as sinceText, StateBadge } from '../ui/kit.ts';
  import { contextOf, isSeen, openStream, streamMenu } from './streams.ts';

  interface Props {
    s: Stream;
    onPeek: (s: Stream) => void;
    dim?: boolean;
    showContext?: boolean;
    /** The Dot badge above the time, where rows of different states mix. */
    badge?: boolean;
    role?: 'treeitem';
  }
  let { s, onPeek, dim = false, showContext = true, badge = true, role }: Props = $props();

  const lp = longPress((pt) => openMenu(streamMenu(s, onPeek), pt, s.name));
  const seenDone = $derived(s.state === 'done' && isSeen(s));
  const since = $derived(s.stateSince ? now.value + clockSkew.value - s.stateSince : null);
  const sub = $derived(showContext ? contextOf(s) || s.folder : [s.kind === 'agent' ? null : s.tool, s.folder].filter(Boolean).join(' · '));
  const time = $derived(since !== null && s.state ? sinceText(since) : '');

  // ---------- swipe (touch): drag the row left to show its actions ----------
  const ACTIONS = 144;
  let dx = $state(0);
  let open = $state(false);
  let sw: { id: number; x: number; y: number; base: number; dir: 'h' | null } | null = null;
  let swiped = false;
  function onpointerdown(e: PointerEvent) {
    if (e.pointerType === 'mouse') return;
    sw = { id: e.pointerId, x: e.clientX, y: e.clientY, base: open ? -ACTIONS : 0, dir: null };
  }
  function onpointermove(e: PointerEvent) {
    if (!sw || e.pointerId !== sw.id) return;
    const mx = e.clientX - sw.x;
    const my = e.clientY - sw.y;
    if (!sw.dir) {
      if (Math.abs(my) > 10 && Math.abs(my) > Math.abs(mx)) return void (sw = null);
      if (Math.abs(mx) <= 10) return;
      sw.dir = 'h';
    }
    dx = Math.max(-ACTIONS, Math.min(0, sw.base + mx));
  }
  function onpointerup() {
    if (sw?.dir === 'h') {
      swiped = true;
      open = dx < -ACTIONS / 2;
      dx = open ? -ACTIONS : 0;
    }
    sw = null;
  }
  const close = () => ((open = false), (dx = 0));
</script>

<li
  class={['row stream-row', dim && 'is-dim', seenDone && 'is-seen', s.id === visibleStreamId.value && 'is-sel', (open || dx) && 'is-swiped']}
  {role}
  aria-selected={role === 'treeitem' ? s.id === visibleStreamId.value : undefined}
  {onpointerdown}
  {onpointermove}
  {onpointerup}
  onpointercancel={onpointerup}
>
  {#if open || dx}
    <span class="row-actions">
      <button class="row-action" onclick={() => (close(), onPeek(s))}><Icon name="eye" size={18} />Peek</button>
      <button class="row-action" onclick={(e) => (close(), openMenu(streamMenu(s, onPeek), e.currentTarget, s.name))}><Icon name="more" size={18} />More</button>
    </span>
  {/if}
  <button
    class="row-main"
    style:transform={dx ? `translateX(${dx}px)` : undefined}
    onclick={(e) => {
      if (lp.consumeClick()) return e.preventDefault();
      if (swiped) return void (swiped = false);
      if (open) return close();
      openStream(s);
    }}
    {...lp.handlers}
  >
    <ToolIcon tool={s.tool} kind={s.kind} state={s.state} />
    <span class="row-text">
      <span class="row-title">{#if s.state === 'done' && !seenDone}<span class="new-dot" aria-label="New"></span>{/if}{s.name}</span>
      <!-- Without the badge, VoiceOver still hears the state: "Fix flaky login test, Needs you, web · auth, 2 min". -->
      {#if !badge && s.state}<span class="sr-only">, {STATE_LABEL[s.state]},</span>{/if}
      <span class="row-sub">{sub}</span>
    </span>
    <span class="row-trail">
      {#if badge}<StateBadge state={s.state} seen={seenDone} />{/if}
      {#if time}<span class={['row-since', !badge && s.state === 'blocked' && 'is-attention']}>{time}</span>{/if}
    </span>
  </button>
  <button class="icon-btn row-more" aria-label="More actions for {s.name}" onclick={(e) => openMenu(streamMenu(s, onPeek), e.currentTarget, s.name)}>
    <Icon name="more" />
  </button>
</li>
