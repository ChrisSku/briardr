<script lang="ts">
  import { AsOf, ago, duration, type PluginContext } from '@briardr/sdk';
  import { clockAt as clockAtSkew, type Status } from './usage.ts';

  let { briardr, name, s, onReset }: { briardr: PluginContext; name: string; s?: Status; onReset?: (available: number) => void } = $props();
  const clockAt = (at: number) => clockAtSkew(at, briardr.clockSkew());

  const t = $derived(briardr.now());
  // Prefer whichever report is newer: the direct read or the status-line tap.
  const rep = $derived(s?.report && s?.tapReport ? (s.tapReport.capturedAt > s.report.capturedAt ? s.tapReport : s.report) : s?.report || s?.tapReport || null);
</script>

<section class="usage-card" aria-label="{name} usage">
  <header class="usage-head">
    <h2>{name}</h2>
    {#if rep?.plan}<span class="tag">{rep.plan}</span>{/if}
    {#if rep}<AsOf at={rep.capturedAt} prefix={rep.source === 'statusline' ? 'from status line' : 'as of'} />{/if}
  </header>
  {#if !s?.enabled && !rep}
    <p class="hint">Not set up. {name === 'Claude' ? 'Allow the Keychain read or add the status-line tap in Settings.' : 'Turn it on in Settings.'}</p>
  {/if}
  {#if s?.enabled && !rep && !s.lastError}<p class="hint">No report yet.</p>{/if}
  {#each rep?.windows ?? [] as w}
    {@const passed = w.resetsAt !== null && w.resetsAt <= t}
    {@const pct = w.usedPercent}
    <div class="meter-row">
      <div class="meter-label">
        <span>{w.label}</span>
        <span class="meter-value">{passed ? 'Reset since last report' : pct === null ? 'Unknown' : `${Math.round(pct)}% used`}</span>
      </div>
      {#if !passed && pct !== null}
        <div class="meter" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(pct)} aria-label="{w.label} {Math.round(pct)} percent used">
          <span class={['meter-fill', pct >= 90 ? 'hot' : pct >= 70 && 'warm']} style:width="{Math.min(100, pct)}%"></span>
        </div>
      {/if}
      <div class="meter-sub">{w.resetsAt ? (passed ? `Reset ${ago(t - w.resetsAt)}` : `Resets in ${duration(w.resetsAt - t)} · ${clockAt(w.resetsAt)}`) : 'Reset time unknown'}</div>
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
    <div class="meter-row reset-credits">
      <div class="meter-label">
        <span>Limit resets</span>
        <span class="meter-value">{rc.available ? `${rc.available} available` : 'None available'}</span>
      </div>
      {#each rc.credits ?? [] as c}
        <div class="meter-sub">{c.title || 'Reset'}{c.expiresAt ? ` · expires in ${duration(c.expiresAt - t)} (${clockAt(c.expiresAt)})` : ''}</div>
      {/each}
      {#if rc.available > 0 && onReset}
        <button class="btn btn-small" onclick={() => onReset(rc.available)}>Use a reset…</button>
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
