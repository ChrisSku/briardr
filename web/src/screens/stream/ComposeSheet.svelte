<!--
  Compose: a message for the program, written in this device's own text field (its autocorrection, the
  keyboard's dictation, the usual editing) and inserted as one paste. Like every insert, it's never
  submitted: Enter stays with the user. What's written and not inserted is kept as the stream's draft, and
  the messages inserted last are offered again; both stay on this device.
-->
<script module lang="ts">
  import { persisted } from '../../store.ts';

  const drafts = persisted<Record<string, string>>('briardr.compose.drafts', {});
  const recent = persisted<string[]>('briardr.compose.recent', []);
</script>

<script lang="ts">
  import { record } from '../../diag.ts';
  import { prefs, toast } from '../../store.ts';
  import type { TerminalController } from '../../terminal/controller.ts';
  import type { Stream } from '../../types.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { MAX_PASTE_BYTES, remembered, withDraft } from './insert.ts';

  let { ctl, stream, onClose }: { ctl: TerminalController; stream: Stream; onClose: () => void } = $props();
  const enc = new TextEncoder();
  // svelte-ignore state_referenced_locally
  let text = $state(drafts.value[stream.id] ?? '');
  const bytes = $derived(enc.encode(text).length);
  const tooBig = $derived(bytes > MAX_PASTE_BYTES);
  const others = $derived(recent.value.filter((r) => r !== text.trim()));

  // Saved as it's written: closing the sheet, leaving the stream or an update's reload doesn't lose it.
  function write(next: string) {
    text = next;
    drafts.value = withDraft(drafts.value, stream.id, next);
  }

  function insert() {
    // The press is the request: the text goes to the stream as it's connected now, or not at all.
    if (!ctl.paste(text, prefs.value.bracketedPaste)) return;
    record('compose_inserted', { bytes });
    recent.value = remembered(recent.value, text);
    write('');
    toast('Inserted. Press Enter when you’re ready.');
    onClose();
  }
</script>

<Sheet title="Compose" {onClose} wide class="compose">
  <!-- svelte-ignore a11y_autofocus -->
  <textarea class="field compose-area" value={text} oninput={(e) => write(e.currentTarget.value)} aria-label="Message for {stream.name}" autofocus placeholder="Write to {stream.name}"></textarea>
  {#if tooBig}<p class="error" role="alert">Messages are limited to 128 KB.</p>{/if}
  <p class="hint">Inserted as one paste. Briardr never presses Enter for you.</p>
  {#if others.length}
    <section class="compose-recent" aria-labelledby="compose-recent-h">
      <h3 class="section-head">
        <span class="overline" id="compose-recent-h">Recent</span>
        <button class="section-action" onclick={() => (recent.value = [])}>Clear</button>
      </h3>
      <ul class="rows">
        {#each others as r (r)}
          <li class="row"><button class="row-main compose-again" onclick={() => write(r)}><span class="row-sub">{r}</span></button></li>
        {/each}
      </ul>
    </section>
  {/if}
  {#snippet footer()}
    <span class={['counter', tooBig && 'over']}>{tooBig ? `${(bytes / 1024).toFixed(1)} KB of 128 KB` : text.trim() ? 'Draft kept on this device' : ''}</span>
    <button class="btn btn-primary" disabled={!text.trim() || tooBig} onclick={insert}>Insert</button>
  {/snippet}
</Sheet>
