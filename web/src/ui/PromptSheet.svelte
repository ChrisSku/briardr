<!--
  A prompt for the agent, put together in a plugin (lines picked in Files, comments on a diff in Changes)
  and looked over here: a message from the user, what the plugin shows of the rest (children), and the
  text as it will be pasted. Sent to the terminal as one paste; like every insert, it never presses Enter,
  only goes while this device controls the terminal, and is refused after a reconnect.
-->
<script lang="ts">
  import type { Snippet } from 'svelte';
  import type { StreamContext } from '../plugins/types.ts';
  import { Icon } from './icons/index.ts';
  import Sheet from './Sheet.svelte';

  interface Props {
    title: string;
    briardr: StreamContext;
    /** What is pasted, the message included. */
    text: string;
    /** What the user writes to the agent, kept where the plugin keeps the rest. */
    message: string;
    messageLabel: string;
    placeholder?: string;
    /** What the sheet puts together, for its buttons: "Copy prompt", "Clear prompt". */
    noun?: string;
    /** The rest of the prompt, under the message: the picked lines, with a way to remove each. */
    children?: Snippet;
    onClose: () => void;
    /** It was sent (`bytes` long): forget what was put together. */
    onSent: (bytes: number) => void;
    onClear: () => void;
  }
  let { title, briardr, text, message = $bindable(), messageLabel, placeholder, noun = 'prompt', children, onClose, onSent, onClear }: Props = $props();

  const enc = new TextEncoder();
  const bytes = $derived(enc.encode(text).length);
  const max = $derived(briardr.terminal.maxBytes);
  const tooBig = $derived(bytes > max);
  const canSend = $derived(briardr.terminal.canSend);
  // The connection generation when the sheet opened; sending after a reconnect is refused.
  // svelte-ignore state_referenced_locally
  const gen = briardr.terminal.generation;
  const id = 'pm' + Math.random().toString(36).slice(2, 8);

  function send() {
    if (!briardr.terminal.send(text, { generation: gen })) return;
    briardr.toast('Sent to the terminal. Press Enter there when ready.');
    onSent(bytes);
  }
  function copy() {
    navigator.clipboard.writeText(text).then(
      () => briardr.toast('Copied'),
      () => briardr.toast('Copying isn’t allowed here.'),
    );
  }
</script>

<Sheet {title} {onClose} wide initialFocus=".prompt-message">
  <label class="prompt-label" for={id}>{messageLabel}</label>
  <textarea {id} class="field prompt-message" bind:value={message} {placeholder} rows="3"></textarea>
  {@render children?.()}
  {#if text}
    <div class="prompt-more">
      <details class="prompt-raw">
        <summary>What will be pasted</summary>
        <pre>{text}</pre>
      </details>
      <button class="btn btn-small btn-danger-text" onclick={onClear}>Clear {noun}</button>
    </div>
  {/if}
  {#if tooBig}<p class="error" role="alert">Pastes are limited to {max / 1024} KB. Make the {noun} shorter.</p>{/if}
  <p class="hint">{canSend ? 'Briardr never presses Enter for you.' : 'Take control in Terminal to send.'}</p>
  {#snippet footer()}
    <span class={['counter', tooBig && 'over']}>{(bytes / 1024).toFixed(1)} KB of {max / 1024} KB</span>
    <button class="icon-btn" aria-label="Copy {noun}" title="Copy {noun}" disabled={!text} onclick={copy}><Icon name="copy" /></button>
    <button class="btn btn-primary" disabled={!text || tooBig || !canSend} onclick={send}>Send to terminal</button>
  {/snippet}
</Sheet>
