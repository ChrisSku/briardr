<script lang="ts">
  import { HostLabel, theHost } from '../host.ts';
  import { herdr, mac, model, stale } from '../store.ts';
  import { Icon } from '../ui/icons/index.ts';
  import { AsOf } from '../ui/kit.ts';
  import { batteryText, herdrProblem } from './streams.ts';

  const bt = $derived(batteryText());
  const batt = $derived(mac.value?.battery);
  const pct = $derived(batt?.percent ?? 0);
  const low = $derived(batt?.present && batt.source === 'battery' && pct < 20);
  const count = $derived(model.value?.streams.length ?? 0);
</script>

<section class={['mac-summary', stale.value && 'is-stale']} aria-label="{HostLabel()} summary">
  <div class="mac-line">
    <strong class="mac-name">{mac.value?.name || HostLabel()}</strong>
    {#if bt}
      <span class="mac-batt" aria-label="Battery {bt.pct}, {bt.detail}">
        <span class={['batt', low && 'low']} aria-hidden="true">
          <span class="batt-fill" style:width="{Math.max(4, Math.min(100, pct))}%"></span>
        </span>
        <span>{bt.pct}</span>
        {#if bt.icon}<Icon name={bt.icon} size={16} />{/if}
      </span>
    {/if}
  </div>
  <div class="mac-sub">
    {#if bt?.detail}<span>{bt.detail}</span>{/if}
    <span>
      {#if herdr.value?.ok}
        Herdr {herdr.value.version || ''} · {count} stream{count === 1 ? '' : 's'}
      {:else if herdr.value}
        {herdrProblem(herdr.value.problem)}
      {:else}
        Waiting for {theHost()}…
      {/if}
    </span>
    <AsOf at={model.value?.asOf} stale={stale.value} />
  </div>
</section>
