<!--
  The review put together in Changes: comments on picked diff lines and an optional note, looked over
  and sent to the terminal as one paste. Like every insert, it never presses Enter.
-->
<script lang="ts">
  import { Icon, Sheet, type Stream, type StreamContext } from '@briardr/sdk';
  import { clearReview, removeComment, reviewOf, setNote } from './changes.ts';
  import { formatReview } from './review.ts';

  let { stream, briardr, onClose, onSent }: { stream: Stream; briardr: StreamContext; onClose: () => void; onSent: () => void } = $props();

  const enc = new TextEncoder();
  const sid = $derived(stream.id);
  const review = $derived(reviewOf(sid));
  const text = $derived(formatReview(review));
  const bytes = $derived(enc.encode(text).length);
  const tooBig = $derived(bytes > briardr.terminal.maxBytes);
  const canSend = $derived(briardr.terminal.canSend);
  // The connection generation when the sheet opened; sending after a reconnect is refused.
  // svelte-ignore state_referenced_locally
  const gen = briardr.terminal.generation;

  function send() {
    if (!briardr.terminal.send(text, { generation: gen })) return;
    briardr.record('review_sent', { bytes, comments: review.comments.length });
    clearReview(sid);
    briardr.toast('Sent to the terminal. Press Enter there when ready.');
    onSent();
  }
  function copy() {
    navigator.clipboard.writeText(text).then(
      () => briardr.toast('Copied'),
      () => briardr.toast('Copying isn’t allowed here.'),
    );
  }
  function clear() {
    clearReview(sid);
    onClose();
  }
</script>

<Sheet title="Review comments" {onClose} wide initialFocus=".review-note">
  <label class="review-label" for="review-note">Note for the agent (optional)</label>
  <textarea id="review-note" class="field review-note" value={review.note} oninput={(e) => setNote(sid, e.currentTarget.value)} placeholder="Anything that applies to all of it" rows="2"></textarea>
  {#if review.comments.length}
    <ul class="review-comments" aria-label="Comments">
      {#each review.comments as c, i (i)}
        <li class="review-item">
          <div class="review-head">
            <span class="mono">{c.path} <strong>{c.where}</strong></span>
            <button class="icon-btn" aria-label={`Remove the comment on ${c.path} ${c.where}`} title="Remove" onclick={() => removeComment(sid, i)}><Icon name="close" size={18} /></button>
          </div>
          <pre class="review-code">{c.lines}</pre>
          <p class="review-text">{c.text}</p>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="hint">No comments yet. In a diff, tap line numbers and choose Comment.</p>
  {/if}
  {#if text}
    <div class="review-more">
      <details class="review-raw">
        <summary>What will be pasted</summary>
        <pre>{text}</pre>
      </details>
      <button class="btn btn-small btn-danger-text" onclick={clear}>Clear review</button>
    </div>
  {/if}
  {#if tooBig}<p class="error" role="alert">Pastes are limited to 128 KB. Remove some comments.</p>{/if}
  <p class="hint">{canSend ? 'Briardr never presses Enter for you.' : 'Take control in Terminal to send.'}</p>
  {#snippet footer()}
    <span class={['counter', tooBig && 'over']}>{(bytes / 1024).toFixed(1)} KB of 128 KB</span>
    <button class="icon-btn" aria-label="Copy review" title="Copy review" disabled={!text} onclick={copy}><Icon name="copy" /></button>
    <button class="btn btn-primary" disabled={!text || tooBig || !canSend} onclick={send}>Send to terminal</button>
  {/snippet}
</Sheet>

<style>
  .review-label { display: block; font-weight: 600; font-size: 0.92rem; margin: 4px 0 6px; }
  .review-note { resize: vertical; min-height: 3.2em; font-size: max(16px, 1rem); line-height: 1.45; }
  .review-comments { list-style: none; margin: 12px 0 0; padding: 0; display: flex; flex-direction: column; gap: 8px; }
  .review-item { border: 1px solid var(--line-default); border-radius: 12px; background: var(--bg-sunken); overflow: hidden; }
  .review-head { display: flex; align-items: center; gap: 4px; padding: 0 2px 0 12px; border-bottom: 1px solid var(--line-default); font-size: 0.82rem; }
  .review-head .mono { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .review-code, .review-raw pre { margin: 0; padding: 8px 12px; font-family: var(--font-mono); font-size: 0.8rem; line-height: 1.45; overflow-x: auto; white-space: pre; }
  .review-text { margin: 0; padding: 8px 12px; border-top: 1px solid var(--line-default); background: var(--bg-surface); white-space: pre-wrap; overflow-wrap: anywhere; }
  .review-more { display: flex; align-items: flex-start; gap: 8px; margin-top: 10px; }
  .review-raw { flex: 1; min-width: 0; }
  .review-raw summary { cursor: pointer; color: var(--text-secondary); font-size: 0.88rem; line-height: 34px; }
  .review-raw pre { max-height: 40vh; overflow: auto; background: var(--bg-sunken); border: 1px solid var(--line-default); border-radius: 10px; margin-top: 6px; }
</style>
