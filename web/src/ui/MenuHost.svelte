<script lang="ts">
  import { tick, untrack } from 'svelte';
  import { hardwareKeyboard } from '../store.ts';
  import { Icon } from './icons/index.ts';
  import { menuReq } from './overlays.ts';
  import { preventSelection } from './prevent-selection.ts';

  const r = $derived(menuReq.value);
  let menu: HTMLDivElement | undefined = $state();
  let pos = $state<{ left: number; top: number } | null>(null);

  // Place the menu once it has a size: next to the anchor, kept inside the visible viewport.
  $effect(() => {
    if (!r || !menu) {
      pos = null;
      return;
    }
    const w = menu.offsetWidth;
    const h = menu.offsetHeight;
    const vw = window.visualViewport?.width || innerWidth;
    const vh = window.visualViewport?.height || innerHeight;
    pos = { left: Math.max(8, Math.min(r.x - w + 24, vw - w - 8)), top: Math.max(8, r.y + h > vh - 8 ? r.y - h - 8 : r.y) };
  });
  let opener: HTMLElement | null = null;
  // Then focus the first item, so arrow keys and Escape reach the menu. Only once the placed menu is
  // in the DOM (after tick: this effect runs before the template applies `pos`): a menu that is still
  // visibility: hidden can't take focus. Without a hardware keyboard, a text field keeps its focus:
  // taking it would close the on-screen keyboard under the terminal or a note.
  $effect(() => {
    if (!pos || !menu) return;
    const m = menu;
    if (!m.contains(document.activeElement)) opener = document.activeElement as HTMLElement | null;
    if (!untrack(() => hardwareKeyboard.value) && opener && (opener.matches('input, textarea') || opener.isContentEditable)) return;
    void tick().then(() => m.querySelector<HTMLElement>('[role=menuitem]:not([disabled])')?.focus());
  });

  // Focus goes back where it was, before an item's action runs (a sheet it opens returns focus there).
  const close = () => {
    if (menu?.contains(document.activeElement)) opener?.focus();
    menuReq.value = null;
  };
  function onkeydown(e: KeyboardEvent) {
    const items = Array.from(menu!.querySelectorAll<HTMLElement>('[role=menuitem]:not([disabled])'));
    const i = items.indexOf(document.activeElement as HTMLElement);
    if (e.key === 'Escape') close();
    else if (e.key === 'ArrowDown') {
      e.preventDefault();
      items[(i + 1) % items.length]?.focus();
    } else if (e.key === 'ArrowUp') {
      e.preventDefault();
      items[(i - 1 + items.length) % items.length]?.focus();
    }
  }
</script>

{#if r}
  <!-- A click outside closes the menu; Escape is the keyboard way. -->
  <!-- svelte-ignore a11y_click_events_have_key_events, a11y_no_static_element_interactions -->
  <div class="menu-scrim" onclick={close} oncontextmenu={(e) => (e.preventDefault(), close())}>
    <!-- Focus goes to the first item; the menu itself can take focus but has no tab stop. -->
    <div
      bind:this={menu}
      use:preventSelection
      class="menu"
      role="menu"
      tabindex="-1"
      aria-label={r.title || 'Actions'}
      style:left={pos ? pos.left + 'px' : undefined}
      style:top={pos ? pos.top + 'px' : undefined}
      style:visibility={pos ? undefined : 'hidden'}
      onclick={(e) => e.stopPropagation()}
      {onkeydown}
    >
      {#if r.title}<div class="menu-title">{r.title}</div>{/if}
      {#each r.items as it}
        <button
          role="menuitem"
          class={['menu-item', it.destructive && 'danger']}
          disabled={it.disabled}
          onclick={() => {
            close();
            it.onSelect();
          }}
        >
          {#if it.icon}<span class="menu-icon"><Icon name={it.icon} size={18} /></span>{/if}
          <span class="menu-label">{it.label}</span>
          {#if it.hint}<span class="menu-hint">{it.hint}</span>{/if}
        </button>
      {/each}
    </div>
  </div>
{/if}
