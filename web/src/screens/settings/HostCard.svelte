<!-- The computer Briardr runs on, at the top of Settings: its name, the connection and Herdr, and (with a
     battery or power controls) a Power row. The Mac summary lived on Agents before the redesign. -->
<script lang="ts">
  import { caps, HostLabel, theHost } from '../../host.ts';
  import { navigate } from '../../router.ts';
  import { herdr, mac, model, stale } from '../../store.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import { batteryText, herdrProblem } from '../streams.ts';

  const bt = $derived(batteryText());
  const batt = $derived(mac.value?.battery);
  const low = $derived(batt?.present && batt.source === 'battery' && (batt.percent ?? 100) < 20);
  const hasPower = $derived(caps().battery || caps().keepAwake || caps().sleep);
  const ok = $derived(!!herdr.value?.ok && !stale.value);
  const status = $derived(
    herdr.value?.ok ? `${stale.value ? 'Not connected' : 'Connected'} · Herdr ${herdr.value.version || ''}`.trim() : herdr.value ? herdrProblem(herdr.value.problem) : `Waiting for ${theHost()}…`,
  );
  const count = $derived(model.value?.streams.length ?? 0);
</script>

<section class={['card mac-summary', stale.value && 'is-stale']} aria-label="{HostLabel()} summary">
  <a class="host-row" href="/settings/herdr" onclick={(e) => (e.preventDefault(), navigate({ name: 'settings', section: 'herdr' }))}>
    <span class="host-tile" aria-hidden="true"><Icon name="laptop" size={22} /></span>
    <span class="host-text">
      <strong class="mac-name">{mac.value?.name || HostLabel()}</strong>
      <span class="host-status"><span class={['host-dot', ok ? 'st-finished' : 'st-unknown']} aria-hidden="true"></span>{status}{herdr.value?.ok ? ` · ${count} stream${count === 1 ? '' : 's'}` : ''}</span>
    </span>
    <Icon name="chevron" size={15} class="set-chev" />
  </a>
  {#if hasPower}
    <a class="set-row" href="/settings/power" onclick={(e) => (e.preventDefault(), navigate({ name: 'settings', section: 'power' }))}>
      <span class={['set-tile', low && 'tone-danger']}><Icon name="battery" size={17} /></span>
      <span class="set-label">Power</span>
      {#if bt}<span class={['set-value', 'mac-batt', low && 'is-low']}>{bt.pct}{bt.detail ? ` · ${bt.detail.split(' · ')[0].toLowerCase()}` : ''}</span>{/if}
      <Icon name="chevron" size={15} class="set-chev" />
    </a>
  {/if}
</section>
