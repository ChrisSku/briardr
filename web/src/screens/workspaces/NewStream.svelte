<script module lang="ts">
  import type { AgentChoice } from '../../types.ts';

  // The agents the server offered last time, so the sheet shows them at once; each opening asks again.
  let known = $state<AgentChoice[]>([]);
  let maxPrompt = $state(2000);
</script>

<script lang="ts">
  import { TheHost } from '../../host.ts';
  import { api } from '../../api.ts';
  import type { Agents, CreatedStream } from '../../protocol.ts';
  import { model, toast } from '../../store.ts';
  import type { Project } from '../../types.ts';
  import { ToolIcon } from '../../ui/icons/index.ts';
  import { Sheet } from '../../ui/kit.ts';
  import { onHerdrApp, openStream } from '../streams.ts';
  import { Submit } from '../submit.svelte.ts';

  let { project, onClose }: { project: Project; onClose: () => void } = $props();
  /** One tap away; the other agents Herdr starts are under Other. */
  const PINNED = ['claude', 'codex'];
  let name = $state('');
  let choice = $state('shell');
  let otherKind = $state('');
  let prompt = $state('');
  const pinned = $derived(known.filter((a) => PINNED.includes(a.kind)));
  const others = $derived(known.filter((a) => !PINNED.includes(a.kind)));
  const kind = $derived(choice === 'other' ? otherKind || others[0]?.kind || 'shell' : choice);
  const agent = $derived(known.find((a) => a.kind === kind));

  api<Agents>('GET', '/api/agents')
    .then((r) => {
      known = r.agents;
      maxPrompt = r.maxPrompt;
    })
    .catch(() => {});

  // The prompt is one line: Enter starts the stream, and pasted line breaks become spaces.
  function promptKey(e: KeyboardEvent & { currentTarget: HTMLTextAreaElement }) {
    if (e.key !== 'Enter' || e.isComposing) return;
    e.preventDefault();
    e.currentTarget.form?.requestSubmit();
  }

  const s = new Submit(async () => {
    const body = { projectId: project.id, kind, name, prompt: agent?.prompt ? prompt : undefined };
    const r = await api<CreatedStream>('POST', '/api/streams', body, { timeout: 45_000 });
    toast(r.note || 'Created stream', r.note ? 6000 : 2600);
    onClose();
    const st = model.value?.streams.find((x) => x.id === r.streamId);
    if (st) openStream(st);
  });
</script>

<Sheet title="New stream in {project.name}" {onClose}>
  <form onsubmit={s.run} class="form">
    <fieldset class="segmented" aria-label="Run">
      <label class={choice === 'shell' ? 'is-on' : ''}>
        <input type="radio" name="kind" value="shell" bind:group={choice} />
        <ToolIcon tool="zsh" kind="shell" size={18} />
        Shell
      </label>
      {#each pinned as a (a.kind)}
        <label class={choice === a.kind ? 'is-on' : ''}>
          <input type="radio" name="kind" value={a.kind} bind:group={choice} />
          <ToolIcon tool={a.kind} kind="agent" size={18} />
          {a.name}
        </label>
      {/each}
      {#if others.length}
        <label class={choice === 'other' ? 'is-on' : ''}>
          <input type="radio" name="kind" value="other" bind:group={choice} />
          <ToolIcon tool="agent" kind="agent" size={18} />
          Other
        </label>
      {/if}
    </fieldset>
    {#if choice === 'other'}
      <select class="field" aria-label="Agent" value={kind} onchange={(e) => (otherKind = e.currentTarget.value)}>
        {#each others as a (a.kind)}<option value={a.kind}>{a.name}</option>{/each}
      </select>
    {/if}
    {#if agent?.prompt}
      <label class="field-label">
        Prompt (optional)
        <textarea
          class="field new-stream-prompt"
          rows="3"
          maxlength={maxPrompt}
          enterkeyhint="go"
          value={prompt}
          oninput={(e) => (prompt = e.currentTarget.value.replace(/[\r\n\t]+/g, ' '))}
          onkeydown={promptKey}
          placeholder="What should {agent.name} do?"
        ></textarea>
      </label>
    {/if}
    {#if onHerdrApp()}
      <p class="hint">The Herdr app puts it beside this project’s last pane, and keeps its current focus.</p>
    {:else}
      <label class="field-label">
        Stream name (optional)
        <input class="field" bind:value={name} placeholder="e.g. refactor auth" />
      </label>
      <p class="hint">Briardr splits the largest pane in this project. {TheHost()} keeps its current focus.</p>
    {/if}
    {#if s.error}<p class="error" role="alert">{s.error}</p>{/if}
    <div class="form-actions">
      <button type="button" class="btn" onclick={onClose}>Cancel</button>
      <button class="btn btn-primary" disabled={s.busy}>{s.busy ? 'Starting…' : 'Start'}</button>
    </div>
  </form>
</Sheet>

<style>
  .new-stream-prompt { resize: vertical; line-height: 1.45; }
</style>
