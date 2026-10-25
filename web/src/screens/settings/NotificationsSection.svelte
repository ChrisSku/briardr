<script lang="ts">
  import { caps, theHost } from '../../host.ts';
  import { onMount } from 'svelte';
  import { api, ApiError } from '../../api.ts';
  import { standalone } from '../../install.ts';
  import { checkPush, disablePush, enablePush, pushStatus, type PushProblem } from '../../push.ts';
  import { navigate } from '../../router.ts';
  import { toast } from '../../store.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import Section from './Section.svelte';
  import type { Save, Settings } from './settings.ts';

  let { s, save }: { s: Settings | null; save: Save } = $props();

  let test = $state.raw<{ phase: string; detail?: string } | null>(null);
  let busy = $state(false);
  const isIOS = /iPhone|iPad|iPod/.test(navigator.userAgent) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1);
  const st = $derived(pushStatus.value);
  onMount(() => void checkPush());

  const PROBLEMS: Record<PushProblem, string> = {
    blocked: isIOS ? 'Notifications are turned off for Briardr in Settings → Notifications on this device. Turn them on there; Briardr notices when you come back.' : 'Notifications are blocked for this site in the browser’s or system’s settings. Allow them there; Briardr notices when you come back.',
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

  async function sendTest() {
    test = { phase: 'Sending…' };
    try {
      const r = await api<{ delivery: any }>('POST', '/api/notifications/test');
      const d = r.delivery;
      if (!d.accepted) {
        void checkPush();
        return (test = { phase: 'Not sent', detail: d.error === 'subscription_gone' ? 'The push service says this device’s subscription expired. Use Repair above.' : `The push service refused it (${d.statusCode ?? d.error}).` });
      }
      test = { phase: 'Accepted by the push service', detail: 'Waiting for this device to confirm it arrived…' };
      for (let i = 0; i < 20; i++) {
        await new Promise((r) => setTimeout(r, 1000));
        const q = await api<{ delivery: any }>('GET', `/api/notifications/deliveries/${d.id}`);
        if (q.delivery?.deliveredAt) return (test = { phase: 'Delivered to this device', detail: `Confirmed ${((q.delivery.deliveredAt - q.delivery.sentAt) / 1000).toFixed(1)} s after sending.` });
      }
      test = { phase: 'Accepted, not confirmed', detail: 'The push service accepted it, but this device hasn’t confirmed receiving it. Briardr won’t claim it was delivered.' };
    } catch (e) {
      test = { phase: 'Not sent', detail: e instanceof ApiError ? e.message : `Couldn’t reach ${theHost()}.` };
    }
  }

  const SWITCHES = [
    ['needsYou', 'Needs you', 'Blocked on an approval or a question'],
    ['finished', 'Finished', 'Work completed'],
    ['ready', 'Ready for input', 'An agent became idle without finishing new work'],
    ['lowBattery', 'Mac low battery', 'At 20%, and at 10% when Always on lets the Mac sleep'],
    ['includeNames', 'Include names', 'Workspace and stream names in alerts'],
  ] as const;
</script>

<Section id="notifications" title="Notifications">
  <ol class="steps">
    <li class={standalone || !isIOS ? 'done' : ''}>
      <strong>Add Briardr to the Home Screen.</strong>
      {#if !isIOS}
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
    <div class="switches">
      {#each SWITCHES.filter(([k]) => k !== 'lowBattery' || caps().battery) as [k, label, hint]}
        <label class="switch-row">
          <input type="checkbox" role="switch" checked={n[k]} onchange={(e) => save({ notifications: { [k]: e.currentTarget.checked } })} />
          <span>
            {label}
            <small>{hint}</small>
          </span>
        </label>
      {/each}
    </div>
  {/if}
</Section>
