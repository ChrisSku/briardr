<script lang="ts">
  import { record } from '../../diag.ts';
  import { prefs, toast } from '../../store.ts';
  import type { TerminalController } from '../../terminal/controller.ts';
  import type { Stream } from '../../types.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { MAX_PASTE_BYTES } from './insert.ts';

  let { ctl, initial, onClose, stream }: { ctl: TerminalController; initial: string; onClose: () => void; stream: Stream } = $props();
  const enc = new TextEncoder();
  // svelte-ignore state_referenced_locally
  let text = $state(initial);
  let bracketed = $state(prefs.value.bracketedPaste);
  // The connection generation when the sheet opened; inserting after a reconnect is refused.
  // svelte-ignore state_referenced_locally
  const gen = ctl.generation.value;
  const bytes = $derived(enc.encode(text).length);
  const tooBig = $derived(bytes > MAX_PASTE_BYTES);
  const multiline = $derived(text.includes('\n'));

  function insert() {
    if (ctl.paste(text, bracketed, gen)) {
      record('paste_inserted', { bytes, bracketed });
      toast('Inserted. Press Enter when you’re ready.');
      onClose();
    }
  }
</script>

<Sheet title="Paste text" {onClose} wide>
  <!-- svelte-ignore a11y_autofocus -->
  <textarea class="field mono paste-area" bind:value={text} aria-label="Text to paste" autofocus spellcheck={false} autocapitalize="off" autocorrect="off" placeholder="Paste or type here"></textarea>
  <label class="switch-row">
    <input type="checkbox" role="switch" bind:checked={bracketed} />
    <span>
      Bracketed paste
      <small>{bracketed ? 'The program receives this as one paste, line breaks included.' : multiline ? (stream.kind === 'shell' ? 'Without it, each line break runs like pressing Return.' : 'Without it, line breaks are typed as Return presses.') : 'Text is typed as-is.'}</small>
    </span>
  </label>
  {#if tooBig}<p class="error" role="alert">Pastes are limited to 128 KB.</p>{/if}
  <p class="hint">Briardr never presses Enter for you.</p>
  {#snippet footer()}
    <span class={['counter', tooBig && 'over']}>{(bytes / 1024).toFixed(1)} KB of 128 KB</span>
    <button class="btn" onclick={onClose}>Cancel</button>
    <button class="btn btn-primary" disabled={!text || tooBig} onclick={insert}>Insert</button>
  {/snippet}
</Sheet>
