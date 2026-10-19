<script lang="ts">
  import { api, ApiError, randomId } from '../../api.ts';
  import { mac, prefs } from '../../store.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import { AsOf, confirmDialog } from '../../ui/kit.ts';
  import { batteryText } from '../streams.ts';
  import Section from './Section.svelte';
  import type { Settings } from './settings.ts';

  let { s }: { s: Settings | null } = $props();
  const m = $derived(mac.value);
  const bt = $derived(batteryText());
  const ka = $derived(m?.keepAwake);
  const status = $derived(
    !ka?.wanted
      ? 'Off'
      : ka.lowBattery
        ? 'Paused at low battery · back above 10%'
        : ka.lidClosed
          ? 'On, lid closed too'
          : ka.setupNeeded
            ? 'On while the lid is open'
            : ka.briardr
              ? 'On'
              : 'Paused for sleep',
  );
  let sleepState = $state.raw<null | 'sending' | 'asleep' | 'maybe' | { error: string }>(null);

  async function sleep() {
    if (!(await confirmDialog('Sleep the Mac?', sleepBody, 'Sleep Mac', true))) return;
    sleepState = 'sending';
    // Sent exactly once; never retried. If the response is lost, the Mac may already be asleep.
    try {
      const r = await api<any>('POST', '/api/power/sleep', { requestId: randomId(16) }, { timeout: 10_000 });
      sleepState = r.ok ? 'asleep' : { error: r.error };
    } catch (e) {
      if (e instanceof ApiError && e.status === 409) sleepState = { error: e.code === 'sleep_disabled' ? 'Another app has disabled sleep on this Mac (pmset disablesleep). Briardr can’t override it.' : e.message };
      else sleepState = 'maybe';
    }
  }
</script>

{#snippet sleepBody()}
  <p>Briardr will be unreachable until the Mac is woken (for example by opening the lid or pressing a key). Agents pause while it sleeps.{ka?.briardr ? ' Always on resumes after it wakes.' : ''}</p>
{/snippet}

<Section id="power" title="Mac power">
  <dl class="info">
    <dt>Battery</dt>
    <dd>{bt ? `${bt.pct} · ${bt.detail}` : 'Unknown'}</dd>
    <dt>Always on</dt>
    <dd>{status}{ka?.others.length ? ` · also held by ${ka.others.join(', ')}` : ''}</dd>
  </dl>
  <AsOf at={m?.asOf} />
  <label class="switch-row">
    <input
      type="checkbox"
      role="switch"
      checked={!!s?.power.keepAwake}
      onchange={async (e) => {
        const r = await api('POST', '/api/power/keep-awake', { on: e.currentTarget.checked });
        mac.value = r.mac;
      }}
    />
    <span>
      Always on
      <small>Keeps the Mac and your agents reachable with the screen off, locked or the lid closed. At 20% battery you get a warning; at 10% the Mac goes to sleep, and Always on comes back once the battery is above 10%.</small>
    </span>
  </label>
  {#if ka?.setupNeeded}
    <p class="note" role="status">Staying awake with the lid closed needs a one-time setup on the Mac: run <code>mise run install-power</code> in the Briardr folder.</p>
  {/if}
  <label class="switch-row">
    <input type="checkbox" role="switch" checked={prefs.value.lowBatteryInApp} onchange={(e) => (prefs.value = { ...prefs.value, lowBatteryInApp: e.currentTarget.checked })} />
    <span>
      Low-battery banners in Briardr
      <small>Shown while Briardr is open. For alerts while it’s closed, turn on “Mac low battery” under Notifications.</small>
    </span>
  </label>
  <button class="btn btn-danger" onclick={sleep} disabled={sleepState === 'sending'}><Icon name="moon" size={18} /> Sleep Mac…</button>
  {#if sleepState === 'asleep'}<p class="note" role="status">The Mac is going to sleep. Briardr will be unreachable until it’s woken.</p>{/if}
  {#if sleepState === 'maybe'}<p class="note" role="status">No answer from the Mac. It may already be asleep. Briardr won’t send the command again.</p>{/if}
  {#if sleepState && typeof sleepState === 'object'}<p class="error" role="alert">{sleepState.error}</p>{/if}
</Section>
