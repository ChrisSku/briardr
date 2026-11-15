<!--
  The prompt put together in Files: a question and the picked lines and files, looked over and sent to the
  terminal as one paste (the SDK's PromptSheet, which never presses Enter).
-->
<script lang="ts">
  import { Icon, PromptSheet, type Stream, type StreamContext } from '@briardr/sdk';
  import { clearPrompt, fileMention, formatPrompt, promptOf, rangeLabel, removeSnippet, removeFileReference, setQuestion } from './prompt.ts';

  let { stream, briardr, onClose, onSent }: { stream: Stream; briardr: StreamContext; onClose: () => void; onSent: () => void } = $props();

  const sid = $derived(stream.id);
  const prompt = $derived(promptOf(sid));

  const PREVIEW_LINES = 6;

  function sent(bytes: number) {
    briardr.record('prompt_sent', { bytes, snippets: prompt.snippets.length });
    clearPrompt(sid);
    onSent();
  }
  function clear() {
    clearPrompt(sid);
    onClose();
  }
</script>

<PromptSheet
  title="Ask about code"
  {briardr}
  text={formatPrompt(prompt)}
  bind:message={() => prompt.question, (q) => setQuestion(sid, q)}
  messageLabel="Question or instructions"
  placeholder="What should the agent do with this code?"
  {onClose}
  onSent={sent}
  onClear={clear}
>
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
</PromptSheet>
