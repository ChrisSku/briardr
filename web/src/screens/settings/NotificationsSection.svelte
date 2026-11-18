<script lang="ts">
  import { caps, theHost } from '../../host.ts';
  import { onMount } from 'svelte';
  import { api, errorMessage } from '../../api.ts';
  import { isIOS } from '../../device.ts';
  import { iosBrowserTab, standalone } from '../../install.ts';
  import { alertPrefs, checkPush, disablePush, enablePush, pushStatus, setAlertPrefs, type AlertChoice, type PushProblem } from '../../push.ts';
  import { minutesOf, timeOfDay } from '../../quiet-hours.ts';
  import { navigate } from '../../router.ts';
  import { toast } from '../../store.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import Section from './Section.svelte';
  import type { Settings } from './settings.ts';

  let { s }: { s: Settings | null } = $props();

  let test = $state.raw<{ phase: string; detail?: string } | null>(null);
  let busy = $state(false);
  const ios = isIOS();
  const st = $derived(pushStatus.value);
  onMount(() => void checkPush());

  const PROBLEMS: Record<PushProblem, string> = {
    blocked: ios ? 'Notifications are turned off for Briardr in Settings → Notifications on this device. Turn them on there; Briardr notices when you come back.' : 'Notifications are blocked for this site in the browser’s or system’s settings. Allow them there; Briardr notices when you come back.',
    permission: 'This device no longer allows Briardr’s notifications. Allow them again to get alerts.',
    expired: 'This device’s notification subscription expired or was removed, so alerts can’t reach it.',
    key_changed: `Briardr on ${theHost()} has a new notification key, so this device’s subscription no longer works.`,
  };

  async function turnOn() {
    busy = true;
    const r = await enablePush();
    busy = false;
    if (r === 'on') toast('Notifications are on for this device');
    else if (r === 'denied') toast('Notifications weren’t allowed. You can change this in system Settings.');
    else if (r === 'not_installed') toast('Briardr isn’t saved on this device yet, which notifications need. Try again in a moment, or use Repair offline app in Settings → Diagnostics.');
    else toast('Couldn’t turn on notifications on this device.');
  }

  async function turnOff() {
    busy = true;
    const ok = await disablePush();
    busy = false;
    if (!ok) toast(`Turned off on this device; ${theHost()} is told when it’s reachable.`);
  }

  /** An alert sent to this device, as the host recorded it (`DeliveryRecord` in server/src/push.rs). */
  interface Delivery {
    id: string;
    sentAt: number;
    accepted: boolean | null;
    statusCode: number | null;
    deliveredAt: number | null;
    error: string | null;
  }

  async function sendTest() {
    test = { phase: 'Sending…' };
    try {
      const r = await api<{ delivery: Delivery }>('POST', '/api/notifications/test');
      const d = r.delivery;
      if (!d.accepted) {
        void checkPush();
        return (test = { phase: 'Not sent', detail: d.error === 'subscription_gone' ? 'The push service says this device’s subscription expired. Use Repair above.' : `The push service refused it (${d.statusCode ?? d.error}).` });
      }
      test = { phase: 'Accepted by the push service', detail: 'Waiting for this device to confirm it arrived…' };
      for (let i = 0; i < 20; i++) {
        await new Promise((r) => setTimeout(r, 1000));
        const q = await api<{ delivery: Delivery | null }>('GET', `/api/notifications/deliveries/${d.id}`);
        if (q.delivery?.deliveredAt) return (test = { phase: 'Delivered to this device', detail: `Confirmed ${((q.delivery.deliveredAt - q.delivery.sentAt) / 1000).toFixed(1)} s after sending.` });
      }
      test = { phase: 'Accepted, not confirmed', detail: 'The push service accepted it, but this device hasn’t confirmed receiving it. Briardr won’t claim it was delivered.' };
    } catch (e) {
      test = { phase: 'Not sent', detail: errorMessage(e, `Couldn’t reach ${theHost()}.`) };
    }
  }

  // Which alerts this device gets is its own choice, kept on the host; a choice it never made follows the
  // host's setting (`notifications` in the configuration).
  const CHOICES: [AlertChoice, string, string][] = [
    ['needsYou', 'Needs you', 'Blocked on an approval or a question'],
    ['finished', 'Finished', 'Work completed'],
    ['ready', 'Ready for input', 'An agent became idle without finishing new work'],
    ['lowBattery', 'Mac low battery', 'At 20%, and at 10% when Always on lets the Mac sleep'],
    ['includeNames', 'Include names', 'Workspace and stream names in alerts'],
  ];
  const prefs = $derived(alertPrefs.value);
  const choosing = $derived(st.state === 'on' && prefs !== null);
  async function choose(patch: Parameters<typeof setAlertPrefs>[0]) {
    try {
      await setAlertPrefs(patch);
    } catch (e) {
      toast(errorMessage(e, 'Couldn’t save.'));
    }
  }
  /** Quiet hours as first turned on: the night. */
  const NIGHT = { from: 22 * 60, to: 7 * 60 };
</script>

<Section id="notifications" title="Notifications">
  <ol class="steps">
    <li class={iosBrowserTab ? '' : 'done'}>
      <strong>Add Briardr to the Home Screen.</strong>
      {#if !ios}
        Not required on this device.
      {:else if standalone}
        Done.
      {:else}
        iPhone and iPad only deliver web alerts to Home Screen apps. See <a href="/settings/install" onclick={(e) => (e.preventDefault(), navigate({ name: 'settings', section: 'install' }, { replace: true }))}>Install app</a>, then open Briardr from the Home Screen and turn notifications on there.
      {/if}
    </li>
    <li class={st.state === 'on' ? 'done' : ''}>
      <strong>Allow notifications on this device.</strong>
      {#if st.state === 'unsupported'}
        This browser doesn’t support web push here.
      {:else if st.state === 'needs_install'}
        Available once Briardr is open from the Home Screen.
      {:else if st.state === 'on'}
        On. <button class="btn btn-small" disabled={busy} onclick={turnOff}>Turn off</button>
      {:else if st.state === 'broken'}
        <span class="push-problem" role="status"><strong>Not working.</strong> {PROBLEMS[st.problem]}</span>
        {#if st.problem !== 'blocked'}
          <button class="btn btn-small btn-primary" disabled={busy} onclick={turnOn}><Icon name="bell" size={16} /> {st.problem === 'permission' ? 'Allow notifications' : 'Repair'}</button>
        {/if}
        <button class="btn btn-small" disabled={busy} onclick={turnOff}>Turn off</button>
      {:else if st.state === 'unknown'}
        Checking… <span class="hint">(needs {theHost()} to be reachable)</span>
      {:else}
        <button class="btn btn-small btn-primary" disabled={busy} onclick={turnOn}><Icon name="bell" size={16} /> Allow notifications</button>
      {/if}
    </li>
    <li>
      <strong>Send a test.</strong>
      <button class="btn btn-small" disabled={st.state !== 'on'} onclick={sendTest}>Send test</button>
      {#if test}
        <span class="test-result" role="status" aria-live="polite"><strong>{test.phase}.</strong> {test.detail}</span>
      {/if}
    </li>
  </ol>
  <p class="hint">Alerts come from {theHost()} even while Briardr is closed. They travel through your browser’s push service (Apple’s, on iPhone and iPad), encrypted end to end so the service can’t read them. They never contain terminal text.</p>
  {#if s}
    {@const n = s.notifications}
    <section class="alert-group" aria-labelledby="alerts-device-h">
      <h3 class="section-head"><span class="overline" id="alerts-device-h">This device</span></h3>
      {#if !choosing}<p class="hint">Allow notifications on this device to choose which alerts it gets.</p>{/if}
      <div class="switches">
        {#each CHOICES.filter(([k]) => k !== 'lowBattery' || caps().battery) as [k, label, hint]}
          <label class="switch-row">
            <input type="checkbox" role="switch" disabled={!choosing} checked={prefs?.[k] ?? n[k]} onchange={(e) => choose({ [k]: e.currentTarget.checked })} />
            <span>
              {label}
              <small>{hint}</small>
            </span>
          </label>
        {/each}
        <label class="switch-row">
          <input type="checkbox" role="switch" disabled={!choosing} checked={!!prefs?.quiet} onchange={(e) => choose({ quiet: e.currentTarget.checked ? NIGHT : null })} />
          <span>
            Quiet hours
            <small>No alerts on this device between two times of its day. They aren’t sent later.</small>
          </span>
        </label>
        {#if prefs?.quiet}
          {@const q = prefs.quiet}
          <div class="quiet-times">
            <label>From <input type="time" class="field" disabled={!choosing} value={timeOfDay(q.from)} onchange={(e) => e.currentTarget.value && choose({ quiet: { from: minutesOf(e.currentTarget.value), to: q.to } })} /></label>
            <label>To <input type="time" class="field" disabled={!choosing} value={timeOfDay(q.to)} onchange={(e) => e.currentTarget.value && choose({ quiet: { from: q.from, to: minutesOf(e.currentTarget.value) } })} /></label>
          </div>
        {/if}
      </div>
    </section>
  {/if}
</Section>

<style>
  .alert-group { margin-top: 14px; }
  .alert-group > .hint { margin: 4px 4px 0; }
  .quiet-times { display: flex; gap: 12px; padding: 4px 0 12px; }
  .quiet-times label { flex: 1; display: flex; flex-direction: column; gap: 6px; font: var(--type-footnote-strong); color: var(--text-secondary); }
</style>
