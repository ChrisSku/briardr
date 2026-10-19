<!--
  The key bar: two rows of fixed keys that share the width, like an extra keyboard row:
    | ~ / - ⌃C ⌃D ⌃S ⇤ ⇥ Delete
    + esc ⌃ ⌥ ⇧ ⌘ (arrow pad) (keyboard) Enter
  Enter: hold to dictate. Delete (backspace) sits above Enter, as wide; hold to keep deleting.
  Modifiers: tap = next key only, double tap = lock, hold = chord while held.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { activeMods, applyModsToText, chord, consumeMods, type Outgoing } from '../../../terminal/keys.ts';
  import { Icon } from '../../../ui/icons/index.ts';
  import ArrowPad from './ArrowPad.svelte';
  import DeleteKey from './DeleteKey.svelte';
  import EnterKey from './EnterKey.svelte';
  import ModKey from './ModKey.svelte';

  interface Props {
    enabled: boolean;
    /** The on-screen keyboard is showing. */
    keyboardOpen: boolean;
    onKeyboard: () => void;
    send: (o: Outgoing) => void;
    onPlus: (anchor: HTMLElement) => void;
    onDictateStart: () => void;
    onDictateEnd: (cancel: boolean) => void;
    dictating: boolean;
    level: number;
  }
  let p: Props = $props();

  const SYMBOLS = ['|', '~', '/', '-'];
  const CTRL_KEYS = ['c', 'd', 's'];

  function sendKey(base: string) {
    p.send({ keys: [chord(base, activeMods())] });
    consumeMods();
  }
  function sendText(t: string) {
    p.send(applyModsToText(t, activeMods()));
    consumeMods();
  }
</script>

{#snippet key(aria: string, onPress: (el: HTMLElement) => void, content: Snippet | string, cls = '')}
  <button
    class="key {cls}"
    aria-label={aria}
    title={aria}
    onpointerdown={(e) => e.preventDefault() /* keep the on-screen keyboard up */}
    onclick={(e) => onPress(e.currentTarget)}
  >
    {#if typeof content === 'string'}{content}{:else}{@render content()}{/if}
  </button>
{/snippet}

{#snippet plus()}<Icon name="plus" size={20} />{/snippet}
{#snippet keyboard()}
  {#if p.keyboardOpen}<Icon name="keyboard-down" size={22} />{:else}<Icon name="keyboard" size={22} />{/if}
{/snippet}

<div class={['keybar', !p.enabled && 'is-disabled']} role="toolbar" aria-label="Terminal keys">
  <div class="keys-top" role="group" aria-label="Symbols and shortcuts">
    {#each SYMBOLS as c}
      {@render key(c === '-' ? 'Hyphen' : c, () => sendText(c), c === '-' ? '−' : c)}
    {/each}
    {#each CTRL_KEYS as c}
      {@render key(`Control-${c.toUpperCase()}`, () => p.send({ keys: [`ctrl+${c}`] }), `⌃${c.toUpperCase()}`)}
    {/each}
    {@render key('Shift-Tab', () => p.send({ keys: ['shift+tab'] }), '⇤')}
    {@render key('tab (Tab)', () => sendKey('tab'), '⇥')}
    <DeleteKey onPress={() => sendKey('backspace')} />
  </div>
  <div class="keys-main">
    {@render key('Insert text, image or file', (el) => p.onPlus(el), plus, 'key-live key-wide')}
    {@render key('esc (Escape)', () => sendKey('esc'), 'esc')}
    <ModKey m="ctrl" label="⌃" name="Control" />
    <ModKey m="alt" label="⌥" name="Option" />
    <ModKey m="shift" label="⇧" name="Shift" />
    <ModKey m="super" label="⌘" name="Command" />
    <ArrowPad send={(d) => sendKey(d)} />
    {@render key(p.keyboardOpen ? 'Hide keyboard' : 'Show keyboard', p.onKeyboard, keyboard, 'key-live key-wide')}
    <EnterKey onTap={() => p.send({ keys: ['enter'] })} onHoldStart={p.onDictateStart} onHoldEnd={p.onDictateEnd} dictating={p.dictating} level={p.level} />
  </div>
</div>
