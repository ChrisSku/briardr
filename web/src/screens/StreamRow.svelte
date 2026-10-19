<script lang="ts">
  import { clockSkew, now, visibleStreamId } from '../store.ts';
  import type { Stream } from '../types.ts';
  import { Icon, ToolIcon } from '../ui/icons/index.ts';
  import { ago, longPress, openMenu, StateBadge } from '../ui/kit.ts';
  import { contextOf, isSeen, openStream, streamMenu } from './streams.ts';

  interface Props {
    s: Stream;
    onPeek: (s: Stream) => void;
    dim?: boolean;
    showContext?: boolean;
    role?: 'treeitem';
  }
  let { s, onPeek, dim = false, showContext = true, role }: Props = $props();

  const lp = longPress((pt) => openMenu(streamMenu(s, onPeek), pt, s.name));
  const seenDone = $derived(s.state === 'done' && isSeen(s));
  const since = $derived(s.stateSince ? now.value + clockSkew.value - s.stateSince : null);
</script>

<li class={['row stream-row', dim && 'is-dim', seenDone && 'is-seen']} {role} aria-selected={role === 'treeitem' ? s.id === visibleStreamId.value : undefined}>
  <button
    class="row-main"
    onclick={(e) => {
      if (lp.consumeClick()) return e.preventDefault();
      openStream(s);
    }}
    {...lp.handlers}
  >
    <ToolIcon tool={s.tool} kind={s.kind} />
    <span class="row-text">
      <span class="row-title">{#if s.state === 'done' && !seenDone}<span class="new-dot" aria-label="New"></span>{/if}{s.name}</span>
      <span class="row-sub">{showContext ? contextOf(s) || s.folder : [s.kind === 'agent' ? null : s.tool, s.folder].filter(Boolean).join(' · ')}</span>
    </span>
    <span class="row-trail">
      <StateBadge state={s.state} seen={seenDone} />
      {#if since !== null && s.state}<span class="row-since">{ago(since)}</span>{/if}
    </span>
  </button>
  <button class="icon-btn row-more" aria-label="More actions for {s.name}" onclick={(e) => openMenu(streamMenu(s, onPeek), e.currentTarget, s.name)}>
    <Icon name="more" />
  </button>
</li>
