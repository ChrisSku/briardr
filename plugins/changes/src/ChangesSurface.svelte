<!--
  Changes: read-only review of uncommitted work against HEAD. Refreshes when opened, when the agent's
  state changes, and on pull-to-refresh; never polls in the background. Requests like "revert this
  file" belong to the agent, through the terminal.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, AsOf, Empty, Icon, PullToRefresh, type StreamTabProps } from '@briardr/sdk';
  import { LETTER, reviewed, type ChangedFile, type Report } from './changes.ts';
  import Diff from './Diff.svelte';

  let { stream, visible, briardr }: StreamTabProps = $props();
  const host = $derived(briardr.host());
  let report = $state.raw<Report | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let open = $state.raw<Record<string, boolean>>({});

  async function load() {
    loading = true;
    error = null;
    try {
      report = await briardr.core<Report>('GET', `/api/streams/${encodeURIComponent(stream.id)}/changes`, undefined, { timeout: 20_000 });
    } catch (e) {
      error = e instanceof ApiError ? e.message : 'Couldn’t load changes.';
    } finally {
      loading = false;
    }
  }

  // Refresh on open and whenever the agent's state changes while visible.
  const id = $derived(stream.id);
  const agentState = $derived(stream.state);
  $effect(() => {
    void [id, agentState];
    if (visible) untrack(load);
  });

  const ptr = new PullToRefresh(load);
  const marks = $derived(reviewed.value[stream.id] || {});
  function setMark(f: ChangedFile, on: boolean) {
    const cur = { ...(reviewed.value[stream.id] || {}) };
    if (on) cur[f.path] = f.signature;
    else delete cur[f.path];
    reviewed.value = { ...reviewed.value, [stream.id]: cur };
  }
</script>

{#if visible}
  <div class="surface changes" {...ptr.handlers}>
    {#if ptr.pull > 0}<div class="ptr" style:height="{ptr.pull}px">{ptr.pull > 64 ? 'Release to refresh' : 'Pull to refresh'}</div>{/if}
    <div class="surface-head">
      <div class="surface-title">
        {#if report?.state === 'ok' || report?.state === 'clean'}
          <strong>{report.noCommits ? 'No commits yet' : report.detached ? `Detached at ${report.headShort}` : report.branch}</strong>
          {#if report.state === 'ok'}<span class="muted">{` · ${report.totalFiles} changed file${report.totalFiles === 1 ? '' : 's'}`}</span>{/if}
        {:else}
          <strong>Changes</strong>
        {/if}
        <AsOf at={report?.asOf} />
      </div>
      <button class="icon-btn" aria-label="Refresh changes" onclick={load} disabled={loading}>
        <Icon name="refresh" class={loading ? 'spin' : ''} />
      </button>
    </div>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if !report && !error}<p class="hint padded">Loading…</p>{/if}
    {#if report?.state === 'not_repo'}<Empty title="Not a Git repository">This stream’s folder isn’t inside a Git repository.</Empty>{/if}
    {#if report?.state === 'git_missing'}<Empty title="Git isn’t installed on {host.the}">Install Git ({host.platform === 'macos' ? 'for example with Xcode Command Line Tools' : 'for example with sudo apt install git'}) to review changes.</Empty>{/if}
    {#if report?.state === 'error'}<Empty title="Git reported an error">{report.message}</Empty>{/if}
    {#if report?.state === 'clean'}<Empty title="No uncommitted changes">The working tree matches {report.noCommits ? 'an empty repository' : 'HEAD'}.</Empty>{/if}
    {#if report?.state === 'ok'}
      {#if report.truncated}<p class="note">Showing the first {report.files.length} of {report.totalFiles} files.</p>{/if}
      <p class="hint padded">Read-only. To revert or change a file, ask the agent in the terminal.</p>
      <ul class="files">
        {#each report.files as f}
          {@const isOpen = !!open[f.path]}
          {@const done = marks[f.path] === f.signature}
          <li class={['file', done && 'is-reviewed']}>
            <div class="file-row">
              <button class="file-toggle" aria-expanded={isOpen} onclick={() => (open = { ...open, [f.path]: !isOpen })}>
                <Icon name="chevron" class={['chev', isOpen && 'open']} size={16} />
                <span class="chg chg-{f.change}" title={f.change} aria-label={f.change}>{LETTER[f.change] || '?'}</span>
                <span class="file-path">{#if f.origPath}<span class="muted">{`${f.origPath} → `}</span>{/if}{f.path}</span>
                <span class="file-meta">
                  {#if f.binary}<span class="tag">binary</span>{:else if f.additions !== null}<span class="add">+{f.additions}</span> <span class="del">−{f.deletions}</span>{/if}
                  {#if f.modeChange}<span class="tag">mode {f.modeChange.from.slice(-3)}→{f.modeChange.to.slice(-3)}</span>{/if}
                  {#if f.staged && !f.untracked}<span class="tag">staged</span>{/if}
                  {#if f.unstaged && !f.untracked && f.staged}<span class="tag">+ unstaged</span>{/if}
                  {#if f.untracked}<span class="tag">untracked</span>{/if}
                </span>
              </button>
              <label class="review-check" title="Reviewed on this device">
                <input type="checkbox" checked={done} onchange={(e) => setMark(f, e.currentTarget.checked)} aria-label="Reviewed {f.path} on this device" />
                <Icon name="check" size={16} />
              </label>
            </div>
            {#if isOpen}<Diff {stream} {briardr} file={f} />{/if}
          </li>
        {/each}
      </ul>
    {/if}
  </div>
{/if}
