<!--
  Changes: read-only review of uncommitted work against HEAD or, when the branch has commits of its
  own, of everything since the branch it started from. Refreshes when opened, when the agent's state
  changes, and on pull-to-refresh; never polls in the background. Requests like "revert this
  file" belong to the agent, through the terminal: lines picked in a diff take a comment each, and the
  comments go to the terminal together, as one paste.
-->
<script lang="ts">
  import { untrack } from 'svelte';
  import { ApiError, AsOf, Empty, Icon, PullToRefresh, type StreamTabProps } from '@briardr/sdk';
  import { LETTER, markKey, reviewed, type ChangedFile, type Report, type Since } from './changes.ts';
  import Diff from './Diff.svelte';
  import { reviewOf } from './review.ts';
  import ReviewSheet from './ReviewSheet.svelte';

  let { stream, visible, briardr }: StreamTabProps = $props();
  const host = $derived(briardr.host());
  let report = $state.raw<Report | null>(null);
  let error = $state<string | null>(null);
  let loading = $state(false);
  let open = $state.raw<Record<string, boolean>>({});
  /** What the files are compared with; back to uncommitted work when the branch has no commits of its own. */
  let since = $state<Since>('head');

  async function load() {
    loading = true;
    error = null;
    try {
      const r = await briardr.core<Report>('GET', `/api/streams/${encodeURIComponent(stream.id)}/changes${since === 'base' ? '?since=base' : ''}`, undefined, { timeout: 20_000 });
      if (!r.base) since = 'head';
      report = r;
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
    void [id, agentState, since];
    if (visible) untrack(load);
  });
  const base = $derived(report?.base ?? null);
  /** What the report on screen is of (the other view shows until the one asked for arrives). */
  const shown = $derived(report?.since ?? 'head');

  const ptr = new PullToRefresh(load);
  const totals = $derived.by(() => {
    const files = report?.state === 'ok' ? report.files : [];
    return { add: files.reduce((n, f) => n + (f.additions ?? 0), 0), del: files.reduce((n, f) => n + (f.deletions ?? 0), 0), lines: files.some((f) => f.additions !== null) };
  });
  const marks = $derived(reviewed.value[stream.id] || {});
  function setMark(f: ChangedFile, on: boolean) {
    const cur = { ...(reviewed.value[stream.id] || {}) };
    if (on) cur[markKey(f.path, shown)] = f.signature;
    else delete cur[markKey(f.path, shown)];
    reviewed.value = { ...reviewed.value, [stream.id]: cur };
  }

  const review = $derived(reviewOf(stream.id));
  let reviewing = $state(false);
  /** The review bar's height: a diff's selection bar sticks above it. */
  let reviewHeight = $state(0);
</script>

{#if visible}
  <div class="surface changes" style:--review-h="{review.comments.length || review.note ? reviewHeight + 8 : 0}px" {...ptr.handlers}>
    {#if ptr.pull > 0}<div class="ptr" style:height="{ptr.pull}px">{ptr.pull > 64 ? 'Release to refresh' : 'Pull to refresh'}</div>{/if}
    <div class="card branch-card">
      <div class="branch-row">
        <Icon name="branch" size={16} />
        <strong class="branch-name">
          {#if report?.state === 'ok' || report?.state === 'clean'}{report.noCommits ? 'No commits yet' : report.detached ? `Detached at ${report.headShort}` : report.branch}{:else}Changes{/if}
        </strong>
        <button class="icon-btn round tinted" aria-label="Refresh changes" onclick={load} disabled={loading}>
          <Icon name="refresh" size={17} class={loading ? 'spin' : ''} />
        </button>
      </div>
      <div class="branch-meta">
        {#if report?.state === 'ok'}
          <span>{report.totalFiles} file{report.totalFiles === 1 ? '' : 's'}</span>
          {#if totals.lines}<span class="sep">·</span><span class="add">+{totals.add}</span> <span class="del">−{totals.del}</span>{/if}
          <span class="sep">·</span>
        {/if}
        <AsOf at={report?.asOf} />
      </div>
      {#if base}
        <div class="segmented small changes-since" role="group" aria-label="Compare with">
          <button class={since === 'head' ? 'is-on' : ''} aria-pressed={since === 'head'} onclick={() => (since = 'head')}>Uncommitted</button>
          <button class={since === 'base' ? 'is-on' : ''} aria-pressed={since === 'base'} onclick={() => (since = 'base')} title="{base.ahead} {base.ahead === 1 ? 'commit' : 'commits'} since {base.name} ({base.short}), and the uncommitted work">Since {base.name}</button>
        </div>
      {/if}
    </div>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if !report && !error}<p class="hint padded">Loading…</p>{/if}
    {#if report?.state === 'not_repo'}<Empty title="Not a Git repository">This stream’s folder isn’t inside a Git repository.</Empty>{/if}
    {#if report?.state === 'git_missing'}<Empty title="Git isn’t installed on {host.the}">Install Git ({host.platform === 'macos' ? 'for example with Xcode Command Line Tools' : 'for example with sudo apt install git'}) to review changes.</Empty>{/if}
    {#if report?.state === 'error'}<Empty title="Git reported an error">{report.message}</Empty>{/if}
    {#if report?.state === 'clean'}
      {#if shown === 'base' && base}
        <Empty title="No changes since {base.name}">The working tree matches {base.name} at {base.short}.</Empty>
      {:else}
        <Empty title="No uncommitted changes">The working tree matches {report.noCommits ? 'an empty repository' : 'HEAD'}.{#if base} {base.ahead} {base.ahead === 1 ? 'commit is' : 'commits are'} ahead of {base.name}.{/if}</Empty>
      {/if}
    {/if}
    {#if report?.state === 'ok'}
      {#if report.truncated}<p class="note">Showing the first {report.files.length} of {report.totalFiles} files.</p>{/if}
      <p class="changes-hint"><Icon name="info" size={14} />Read-only. To have something changed, tap line numbers in a diff and comment for the agent.</p>
      <ul class="files card">
        {#each report.files as f}
          {@const isOpen = !!open[f.path]}
          {@const done = marks[markKey(f.path, shown)] === f.signature}
          {@const slash = f.path.lastIndexOf('/') + 1}
          <li class={['file', done && 'is-reviewed']}>
            <div class="file-row">
              <button class="file-toggle" aria-expanded={isOpen} onclick={() => (open = { ...open, [f.path]: !isOpen })}>
                <span class="chg chg-{f.change}" title={f.change} aria-label={f.change}>{LETTER[f.change] || '?'}</span>
                <span class="file-path">{#if f.origPath}<span class="file-dir">{`${f.origPath} → `}</span>{/if}<span class="file-dir">{f.path.slice(0, slash)}</span>{f.path.slice(slash)}</span>
                <span class="file-meta">
                  {#if f.binary}<span class="tag">binary</span>{:else if f.additions !== null}{#if f.additions}<span class="add">+{f.additions}</span>{/if}{#if f.additions && f.deletions}{' '}{/if}{#if f.deletions}<span class="del">−{f.deletions}</span>{/if}{/if}
                  {#if f.modeChange}<span class="tag">mode {f.modeChange.from.slice(-3)}→{f.modeChange.to.slice(-3)}</span>{/if}
                  {#if f.staged && !f.untracked}<span class="tag">staged</span>{/if}
                  {#if f.unstaged && !f.untracked && f.staged}<span class="tag">+ unstaged</span>{/if}
                  {#if f.untracked}<span class="tag">untracked</span>{/if}
                </span>
                <Icon name={isOpen ? 'chevron-down' : 'chevron'} size={16} class="file-chev" />
              </button>
              <label class="review-check" title="Reviewed on this device">
                <input type="checkbox" checked={done} onchange={(e) => setMark(f, e.currentTarget.checked)} aria-label="Reviewed {f.path} on this device" />
                <Icon name="check" size={16} />
              </label>
            </div>
            {#if isOpen}<Diff {stream} {briardr} file={f} since={shown} />{/if}
          </li>
        {/each}
      </ul>
    {/if}
    {#if review.comments.length || review.note}
      <div class="changes-review" role="region" aria-label="Review" bind:offsetHeight={reviewHeight}>
        <Icon name="notes" size={18} />
        <span class="changes-review-label">{review.comments.length ? `${review.comments.length} ${review.comments.length === 1 ? 'comment' : 'comments'} for the agent` : 'Note drafted'}</span>
        <button class="btn btn-small btn-primary" onclick={() => (reviewing = true)}>Review…</button>
      </div>
    {/if}
  </div>
  {#if reviewing}
    <ReviewSheet {stream} {briardr} onClose={() => (reviewing = false)} onSent={() => {
      reviewing = false;
      briardr.openTab('terminal');
    }} />
  {/if}
{/if}

<style>
  /* The branch and totals in a card, a quiet read-only hint, files with M/A/D chips and the open file's
     diff inline with line numbers. */
  .surface.changes { display: flex; flex-direction: column; gap: 10px; padding-top: 12px; }
  .surface.changes > :global(*) { flex: none; }
  .branch-card { padding: 12px 14px; display: flex; flex-direction: column; gap: 6px; }
  .branch-row { display: flex; align-items: center; gap: 8px; color: var(--text-secondary); min-height: 32px; }
  .branch-row .icon-btn { margin: -6px -8px -6px auto; }
  /* In full screen the way back floats in the top right corner: the row ends before it. */
  :global(.is-full .surfaces) .branch-row { margin-right: calc(var(--full-clear) - var(--gutter) - 6px); }
  .branch-name { font: var(--type-mono-strong); color: var(--text-primary); overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .branch-meta { display: flex; flex-wrap: wrap; align-items: baseline; gap: 4px 6px; font: var(--type-footnote); color: var(--text-secondary); font-variant-numeric: tabular-nums; }
  .branch-meta .sep, .branch-meta :global(.asof) { color: var(--text-tertiary); }
  .changes-since { margin-top: 4px; }
  .changes-hint { display: flex; align-items: center; gap: 6px; margin: 0; padding: 0 4px; font: var(--type-footnote); color: var(--text-tertiary); }
  .files { list-style: none; margin: 0; padding: 0; }
  .file + .file { border-top: 1px solid var(--line-subtle); }
  .file-row { display: flex; align-items: center; }
  .file-toggle { flex: 1; min-width: 0; display: flex; align-items: center; gap: 10px; border: 0; background: none; padding: 11px 4px 11px 12px; text-align: left; min-height: 48px; color: var(--text-primary); flex-wrap: wrap; }
  .file-path { font: var(--type-mono); word-break: break-all; flex: 1; min-width: 40%; }
  .file-dir { color: var(--text-tertiary); }
  .file-meta { display: flex; gap: 6px; align-items: center; font: var(--type-mono-small); }
  .file-toggle :global(.file-chev) { color: var(--text-tertiary); }
  .file.is-reviewed .file-path { color: var(--text-secondary); }
  .chg { font: var(--type-mono-strong); width: 22px; height: 22px; border-radius: 6px; display: inline-flex; align-items: center; justify-content: center; background: var(--bg-sunken); color: var(--text-secondary); flex: none; }
  .chg-modified, .chg-renamed, .chg-copied, .chg-type { color: var(--state-working); background: var(--state-working-bg); }
  .chg-added, .chg-untracked { color: var(--state-finished); background: var(--state-finished-bg); }
  .chg-deleted { color: var(--state-danger); background: var(--state-danger-bg); }
  .chg-conflict { color: var(--text-on-state); background: var(--state-danger); }
  .add { color: var(--diff-add); }
  .del { color: var(--diff-del); }
  .review-check { display: flex; align-items: center; justify-content: center; width: var(--size-tap); height: var(--size-tap); position: relative; color: var(--text-tertiary); cursor: pointer; flex: none; }
  .review-check input { position: absolute; inset: 0; opacity: 0; cursor: pointer; margin: 0; }
  .review-check :global(.icon) { border: 1.5px solid currentColor; border-radius: 6px; padding: 1px; }
  .review-check input:checked + :global(.icon) { color: var(--text-on-ink); background: var(--ink-default); border-color: var(--ink-default); }
  .review-check input:focus-visible + :global(.icon) { outline: 2px solid var(--ink-default); outline-offset: 2px; }
  /* The selection bar (in a diff) and the review bar stay along the bottom while the files scroll, one
     above the other (the card clips without being a scroller, so they can stick). */
  .changes .files.card { overflow: clip; }
  .changes-review {
    position: sticky; bottom: 0; z-index: 1; display: flex; align-items: center; gap: 10px; padding: 10px 12px;
    border: 1px solid var(--line-default); border-radius: var(--radius-lg); background: var(--bg-surface); box-shadow: var(--shadow-menu);
  }
  .changes-review-label { flex: 1; min-width: 0; font-weight: 600; font-size: 0.9rem; }
</style>
