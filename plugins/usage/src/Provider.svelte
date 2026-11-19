<!-- One provider's limits: a meter per window (ink under 70 %, then attention, danger from 90 %), credits,
     limit resets and the last error. -->
<script lang="ts">
  import { ago, AsOf, duration, Icon, type PluginContext } from '@briardr/sdk';
  import { clockAt as clockAtSkew, type Status } from './usage.ts';

  let { briardr, name, s, onReset }: { briardr: PluginContext; name: string; s?: Status; onReset?: (available: number) => void } = $props();
  const clockAt = (at: number) => clockAtSkew(at, briardr.clockSkew());

  const t = $derived(briardr.now());
  // Prefer whichever report is newer: the direct read or the status-line tap.
  const rep = $derived(s?.report && s?.tapReport ? (s.tapReport.capturedAt > s.report.capturedAt ? s.tapReport : s.report) : s?.report || s?.tapReport || null);
  const tone = (pct: number) => (pct >= 90 ? 'tone-danger' : pct >= 70 ? 'tone-attention' : '');
</script>

<section class="usage-card card" aria-label="{name} usage">
  <header class="usage-head">
    <span class="set-tile"><Icon name={name === 'Claude' ? 'claude' : 'codex'} size={17} /></span>
    <h2>{name}</h2>
    {#if rep?.plan}<span class="tag">{rep.plan}</span>{/if}
    {#if rep?.source === 'statusline'}<AsOf at={rep.capturedAt} prefix="from status line" />{/if}
  </header>
  {#if !s?.enabled && !rep}
    <p class="hint">Not set up. {name === 'Claude' ? 'Allow the Keychain read or add the status-line tap in Sources.' : 'Turn it on in Sources.'}</p>
  {/if}
  {#if s?.enabled && !rep && !s.lastError}<p class="hint">No report yet.</p>{/if}
  {#each rep?.windows ?? [] as w}
    {@const passed = w.resetsAt !== null && w.resetsAt <= t}
    {@const pct = w.usedPercent}
    {@const near = !passed && pct !== null && pct >= 90}
    {@const reset = w.resetsAt ? (passed ? `Reset ${ago(t - w.resetsAt)}` : `Resets in ${duration(w.resetsAt - t)} · ${clockAt(w.resetsAt)}`) : 'Reset time unknown'}
    <div class="meter-row">
      <div class="meter-label">
        <span>{w.label}</span>
        <span class={['meter-value', !passed && pct !== null && tone(pct)]}>{passed ? 'Reset since last report' : pct === null ? 'Unknown' : `${Math.round(pct)}%`}</span>
      </div>
      {#if !passed && pct !== null}
        <div class="meter" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label="{w.label} {Math.round(pct)} percent used">
          <span class={['meter-fill', tone(pct)]} style:width="{Math.min(100, pct)}%"></span>
        </div>
      {/if}
      <div class="meter-sub">{near ? `Almost at the limit · ${reset[0].toLowerCase()}${reset.slice(1)}` : reset}</div>
    </div>
  {/each}
  {#if rep?.credits}
    <div class="meter-row">
      <div class="meter-label">
        <span>Credits</span>
        <span class="meter-value">{rep.credits.unlimited ? 'Unlimited' : rep.credits.hasCredits ? (rep.credits.balance ? `${rep.credits.balance} left` : 'Available') : 'None'}</span>
      </div>
    </div>
  {/if}
  {#if rep?.resetCredits}
    {@const rc = rep.resetCredits}
    <div class="reset-credits">
      <div class="reset-credits-text">
        <strong>{rc.available ? `Limit reset${rc.available === 1 ? '' : 's'} available${rc.available > 1 ? ` · ${rc.available}` : ''}` : 'No limit resets available'}</strong>
        {#each rc.credits ?? [] as c}
          <span>{c.title || 'Reset'}{c.expiresAt ? ` · expires in ${duration(c.expiresAt - t)} (${clockAt(c.expiresAt)})` : ''}</span>
        {/each}
      </div>
      {#if rc.available > 0 && onReset}
        <button class="btn btn-small" onclick={() => onReset(rc.available)}>Use reset…</button>
      {/if}
    </div>
  {/if}
  {#if rep?.spend && rep.spend.limitUsd !== null}
    <p class="hint">Spend: ${rep.spend.usedUsd?.toFixed(2) ?? '?'} of ${rep.spend.limitUsd} {rep.spend.period || ''}</p>
  {/if}
  {#if rep?.limitReached}<p class="note">Limit reached ({rep.limitReached.replace(/_/g, ' ')}).</p>{/if}
  {#if s?.lastError}
    <p class={['note', !rep && 'tone-error']}>
      Last check failed {ago(t - s.lastError.at)}: {s.lastError.message}{s.rateLimitedUntil && s.rateLimitedUntil > t ? ` Next check allowed in ${duration(s.rateLimitedUntil - t)}.` : ''}
    </p>
  {/if}
</section>

<style>
  /* A provider's card: its limits as 6 pt meters (ink under 70 %, attention 70–89 %, danger from 90 %). */
  .usage-card { padding: 12px 14px 14px; display: flex; flex-direction: column; gap: 12px; }
  .usage-head { display: flex; align-items: center; gap: 10px; flex-wrap: wrap; }
  .usage-head h2 { font: var(--type-headline); letter-spacing: var(--track-headline); }
  .usage-head .tag { margin-left: 0; }
  .usage-head :global(.asof) { margin-left: auto; }
  .usage-card .hint, .usage-card .note { margin: 0; }
  .meter-row { display: flex; flex-direction: column; gap: 5px; }
  .meter-label { display: flex; justify-content: space-between; gap: 8px; font: var(--type-subhead-strong); letter-spacing: var(--track-subhead-strong); }
  .meter-value { font-variant-numeric: tabular-nums; }
  .meter-value.tone-attention { color: var(--state-attention); }
  .meter-value.tone-danger { color: var(--state-danger); }
  .meter-sub { font: var(--type-footnote); letter-spacing: var(--track-footnote); color: var(--text-tertiary); }
  .reset-credits { display: flex; align-items: center; gap: 10px; padding-top: 12px; border-top: 1px solid var(--line-subtle); }
  .reset-credits-text { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .reset-credits-text strong { font: var(--type-subhead-strong); }
  .reset-credits-text span { font: var(--type-footnote); color: var(--text-tertiary); }
</style>
