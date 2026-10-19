<!--
  A modal sheet: slides up from the bottom on phones, centred on wider screens. `at` instead pins it
  below a point near the top (e.g. under a header) and limits its height, e.g. to above the keyboard.
-->
<script lang="ts">
  import { onMount, type Snippet } from 'svelte';
  import { Icon } from './icons/index.ts';

  interface Props {
    title: string;
    onClose: () => void;
    children: Snippet;
    footer?: Snippet;
    wide?: boolean;
    class?: string;
    initialFocus?: string;
    at?: { top: number; maxHeight: number };
  }
  let { title, onClose, children, footer, wide = false, class: cls = '', initialFocus, at }: Props = $props();

  const focusable = 'button:not([disabled]), [href], input:not([disabled]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"])';
  const id = 'sh' + Math.random().toString(36).slice(2, 8);
  // What had focus before the sheet opened (read before anything inside takes focus).
  const prev = document.activeElement as HTMLElement | null;
  let sheet: HTMLDivElement;

  onMount(() => {
    // Focus the sheet's own field (initialFocus, else an autofocus one: browsers skip autofocus on
    // elements added after load), else the first control. Never move focus that is already inside
    // the sheet: a field there may already be taking input.
    if (!sheet.contains(document.activeElement)) {
      const first = (initialFocus && sheet.querySelector<HTMLElement>(initialFocus)) || sheet.querySelector<HTMLElement>('[autofocus]') || sheet.querySelector<HTMLElement>(focusable);
      first?.focus();
    }
    return () => prev?.focus?.();
  });

  function onkeydown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.stopPropagation();
      onClose();
    } else if (e.key === 'Tab') {
      const items = Array.from(sheet.querySelectorAll<HTMLElement>(focusable)).filter((x) => x.offsetParent !== null);
      if (!items.length) return;
      const a = items[0];
      const b = items[items.length - 1];
      if (e.shiftKey && document.activeElement === a) {
        e.preventDefault();
        b.focus();
      } else if (!e.shiftKey && document.activeElement === b) {
        e.preventDefault();
        a.focus();
      }
    }
  }
</script>

<!-- The scrim closes on a click outside the sheet; Escape is the keyboard way. -->
<!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
<div class={['scrim', at && 'scrim-top']} style:padding-top={at ? `${at.top}px` : undefined} onclick={(e) => e.target === e.currentTarget && onClose()}>
  <!-- Focus moves to a control inside the sheet; the dialog itself can take focus but has no tab stop. -->
  <div bind:this={sheet} class={['sheet', wide && 'wide', cls]} style:max-height={at ? `${at.maxHeight}px` : undefined} role="dialog" aria-modal="true" aria-labelledby={id} tabindex="-1" {onkeydown}>
    <header class="sheet-head">
      <h2 {id}>{title}</h2>
      <button class="icon-btn" onclick={onClose} aria-label="Close">
        <Icon name="close" />
      </button>
    </header>
    <div class="sheet-body">{@render children()}</div>
    {#if footer}
      <footer class="sheet-foot">{@render footer()}</footer>
    {/if}
  </div>
</div>
