<!--
  The key bar: two rows of keys that share the width, like an extra keyboard row:
    (the keys chosen in Settings for this app; by default | ~ / - ⌃C ⌃D ⌃S ⇤ ⇥) Delete
    + esc ⌃ ⌥ ⇧ ⌘ (arrow pad) (keyboard) Enter
  Enter: hold to dictate. Delete (backspace) sits above Enter, as wide; hold to keep deleting.
  Modifiers: tap = next key only, double tap = lock, hold = chord while held.
  No press on the bar, keys or the gaps between them, takes focus: the on-screen keyboard stays up.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import { rowFor, type BarKey } from '../../../terminal/barkeys.ts';
  import { activeMods, applyModsToText, chord, consumeMods, type Outgoing } from '../../../terminal/keys.ts';
  import { Icon } from '../../../ui/icons/index.ts';
  import ArrowPad from './ArrowPad.svelte';
  import DeleteKey from './DeleteKey.svelte';
  import EnterKey from './EnterKey.svelte';
  import ModKey from './ModKey.svelte';

  interface Props {
    enabled: boolean;
    /** The pane's app, which picks the top row's keys. */
    app: string;
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

  const row = $derived(rowFor(p.app));

  function sendKey(base: string) {
    p.send({ keys: [chord(base, activeMods())] });
    consumeMods();
  }
  function sendText(t: string) {
    p.send(applyModsToText(t, activeMods()));
    consumeMods();
  }
  function press(k: BarKey) {
    const s = k.send;
    if ('text' in s) sendText(s.text);
    else if ('named' in s) sendKey(s.named);
    else p.send(s);
  }
</script>

{#snippet key(aria: string, onPress: (el: HTMLElement) => void, content: Snippet | string, cls = '')}
  <button
    class="key {cls}"
    aria-label={aria}
    title={aria}
    onclick={(e) => onPress(e.currentTarget)}
  >
    {#if typeof content === 'string'}{content}{:else}{@render content()}{/if}
  </button>
{/snippet}

{#snippet plus()}<Icon name="plus" size={20} />{/snippet}
{#snippet keyboard()}
  {#if p.keyboardOpen}<Icon name="keyboard-down" size={22} />{:else}<Icon name="keyboard" size={22} />{/if}
{/snippet}

<!-- It only keeps focus where it is; the keys are the controls. -->
<!-- svelte-ignore a11y_interactive_supports_focus -->
<div
  class={['keybar', !p.enabled && 'is-disabled']}
  role="toolbar"
  aria-label="Terminal keys"
  onpointerdown={(e) => e.preventDefault() /* keep the on-screen keyboard up */}
>
  <div class="keys-top" role="group" aria-label="Symbols and shortcuts">
    {#each row as k (k.id)}
      {@render key(k.name, () => press(k), k.label)}
    {/each}
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
