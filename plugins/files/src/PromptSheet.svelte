<!--
  The prompt put together in Files: a question and the picked lines, reviewed and sent to the terminal
  as one paste. Like every insert, it never presses Enter.
-->
<script lang="ts">
  import { Icon, Sheet, type Stream, type StreamContext } from '@briardr/sdk';
  import { clearPrompt, fileMention, formatPrompt, promptOf, rangeLabel, removeSnippet, removeFileReference, setQuestion } from './prompt.ts';

  let { stream, briardr, onClose, onSent }: { stream: Stream; briardr: StreamContext; onClose: () => void; onSent: () => void } = $props();
  const toast = (text: string) => briardr.toast(text);

  const enc = new TextEncoder();
  const sid = $derived(stream.id);
  const prompt = $derived(promptOf(sid));
  const text = $derived(formatPrompt(prompt));
  const bytes = $derived(enc.encode(text).length);
  const tooBig = $derived(bytes > briardr.terminal.maxBytes);
  const canSend = $derived(briardr.terminal.canSend);
  // The connection generation when the sheet opened; sending after a reconnect is refused.
  // svelte-ignore state_referenced_locally
  const gen = briardr.terminal.generation;

  const PREVIEW_LINES = 6;

  function send() {
    if (!briardr.terminal.send(text, { generation: gen })) return;
    briardr.record('prompt_sent', { bytes, snippets: prompt.snippets.length });
    clearPrompt(sid);
    toast('Sent to the terminal. Press Enter there when ready.');
    onSent();
  }
  function copy() {
    navigator.clipboard.writeText(text).then(
      () => toast('Copied'),
      () => toast('Copying isn’t allowed here.'),
    );
  }
  function clear() {
    clearPrompt(sid);
    onClose();
  }
</script>

<Sheet title="Ask about code" {onClose} wide initialFocus=".prompt-question">
  <label class="prompt-label" for="prompt-question">Question or instructions</label>
  <textarea
    id="prompt-question"
    class="field prompt-question"
    value={prompt.question}
    oninput={(e) => setQuestion(sid, e.currentTarget.value)}
    placeholder="What should the agent do with this code?"
    rows="3"
  ></textarea>
  {#if prompt.snippets.length}
    <ul class="prompt-snippets" aria-label="Code in the prompt">
      {#each prompt.snippets as s, i (`${s.path}:${s.from}-${s.to}`)}
        {@const lines = s.text.split('\n')}
        <li class="prompt-snippet">
          <div class="prompt-snippet-head">
            <span class="mono">{s.path} <strong>{rangeLabel(s.from, s.to)}</strong></span>
            <button class="icon-btn" aria-label={`Remove ${s.path} ${rangeLabel(s.from, s.to)}`} title="Remove" onclick={() => removeSnippet(sid, i)}><Icon name="close" size={18} /></button>
          </div>
          <pre class="prompt-snippet-code">{lines.slice(0, PREVIEW_LINES).join('\n')}{#if lines.length > PREVIEW_LINES}<span class="muted">{'\n'}… {lines.length - PREVIEW_LINES} more {lines.length - PREVIEW_LINES === 1 ? 'line' : 'lines'}</span>{/if}</pre>
        </li>
      {/each}
    </ul>
  {:else if !prompt.references?.length}
    <p class="hint">No code yet. In Files, tap line numbers (or select text) and choose Add.</p>
  {/if}
  {#if prompt.references?.length}
    <ul class="prompt-snippets" aria-label="Files and folders in the prompt">
      {#each prompt.references as r, i (r.path)}
        <li class="prompt-snippet prompt-snippet-head">
          <Icon name={r.dir ? 'folder' : 'file'} size={18} />
          <span class="mono">{fileMention(r)}</span>
          <button class="icon-btn" aria-label={`Remove ${r.path}`} title="Remove" onclick={() => removeFileReference(sid, i)}><Icon name="close" size={18} /></button>
        </li>
      {/each}
    </ul>
  {/if}
  {#if text}
    <div class="prompt-more">
      <details class="prompt-raw">
        <summary>What will be pasted</summary>
        <pre>{text}</pre>
      </details>
      <button class="btn btn-small btn-danger-text" onclick={clear}>Clear prompt</button>
    </div>
  {/if}
  {#if tooBig}<p class="error" role="alert">Pastes are limited to 128 KB. Remove some code.</p>{/if}
  <p class="hint">{canSend ? 'Briardr never presses Enter for you.' : 'Take control in Terminal to send.'}</p>
  {#snippet footer()}
    <span class={['counter', tooBig && 'over']}>{(bytes / 1024).toFixed(1)} KB of 128 KB</span>
    <button class="icon-btn" aria-label="Copy prompt" title="Copy prompt" disabled={!text} onclick={copy}><Icon name="copy" /></button>
    <button class="btn btn-primary" disabled={!text || tooBig || !canSend} onclick={send}>Send to terminal</button>
  {/snippet}
</Sheet>
