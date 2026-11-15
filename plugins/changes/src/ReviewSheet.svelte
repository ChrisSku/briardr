<!--
  The review put together in Changes: comments on picked diff lines and an optional note, looked over and
  sent to the terminal as one paste (the SDK's PromptSheet, which never presses Enter).
-->
<script lang="ts">
  import { Icon, PromptSheet, type Stream, type StreamContext } from '@briardr/sdk';
  import { clearReview, formatReview, removeComment, reviewOf, setNote } from './review.ts';

  let { stream, briardr, onClose, onSent }: { stream: Stream; briardr: StreamContext; onClose: () => void; onSent: () => void } = $props();

  const sid = $derived(stream.id);
  const review = $derived(reviewOf(sid));

  function sent(bytes: number) {
    briardr.record('review_sent', { bytes, comments: review.comments.length });
    clearReview(sid);
    onSent();
  }
  function clear() {
    clearReview(sid);
    onClose();
  }
</script>

<PromptSheet
  title="Review comments"
  noun="review"
  {briardr}
  text={formatReview(review)}
  bind:message={() => review.note, (n) => setNote(sid, n)}
  messageLabel="Note for the agent (optional)"
  placeholder="Anything that applies to all of it"
  {onClose}
  onSent={sent}
  onClear={clear}
>
  {#if review.comments.length}
    <ul class="prompt-snippets" aria-label="Comments">
      {#each review.comments as c, i (i)}
        <li class="prompt-snippet">
          <div class="prompt-snippet-head">
            <span class="mono">{c.path} <strong>{c.where}</strong></span>
            <button class="icon-btn" aria-label={`Remove the comment on ${c.path} ${c.where}`} title="Remove" onclick={() => removeComment(sid, i)}><Icon name="close" size={18} /></button>
          </div>
          <pre class="prompt-snippet-code">{c.lines}</pre>
          <p class="prompt-snippet-note">{c.text}</p>
        </li>
      {/each}
    </ul>
  {:else}
    <p class="hint">No comments yet. In a diff, tap line numbers and choose Comment.</p>
  {/if}
</PromptSheet>
