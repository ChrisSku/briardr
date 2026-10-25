<!-- Pairing: the only screen available before this device is signed in. -->
<script lang="ts">
  import { hostNoun, isMac, theHost } from '../host.ts';
  import { api, ApiError } from '../api.ts';
  import { record } from '../diag.ts';
  import { showAttentionBadge } from '../badge.svelte.ts';
  import { installSuggestion, standalone } from '../install.ts';
  import { watchPush } from '../push.ts';
  import { liveConn } from '../live.ts';
  import { navigate } from '../router.ts';
  import { host, mac, session } from '../store.ts';

  let { reason }: { reason?: string } = $props();

  function defaultName(): string {
    const ua = navigator.userAgent;
    if (/iPad/.test(ua) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1)) return 'iPad';
    if (/iPhone/.test(ua)) return 'iPhone';
    if (/Macintosh/.test(ua)) return 'Mac';
    return 'Device';
  }

  let code = $state('');
  let name = $state(defaultName());
  let busy = $state(false);
  let error = $state<string | null>(null);

  async function submit(e: Event) {
    e.preventDefault();
    busy = true;
    error = null;
    try {
      const r = await api('POST', '/api/pair', { code, name });
      record('paired', {});
      if (!standalone && installSuggestion.value !== 'dismissed') installSuggestion.value = 'pending';
      session.value = { status: 'signed_in', device: r.device };
      // Start the live session in place; no reload needed.
      navigate({ name: 'agents' }, { replace: true });
      liveConn.start();
      watchPush();
      showAttentionBadge();
      api('GET', '/api/mac').then((m) => {
        mac.value = m.mac;
        if (m.host) host.value = m.host;
      }, () => {});
    } catch (x) {
      error = x instanceof ApiError ? x.message : `Couldn’t reach ${theHost()}.`;
    } finally {
      busy = false;
    }
  }
</script>

<main class="pair">
  <div class="pair-card">
    <h1>Briardr</h1>
    <p class="lead">Sign in this device to see and steer the work running on your {hostNoun()}.</p>
    {#if reason}<p class="note">{reason}</p>{/if}
    <form class="form" onsubmit={submit}>
      <label class="field-label">
        Pairing code
        <!-- svelte-ignore a11y_autofocus -->
        <input
          class="field mono code-input"
          bind:value={() => code, (v) => (code = v.toUpperCase())}
          placeholder="XXXX-XXXX"
          autocomplete="one-time-code"
          autocapitalize="characters"
          autocorrect="off"
          spellcheck={false}
          inputmode="text"
          maxlength={9}
          autofocus
          required
        />
      </label>
      <label class="field-label">
        Name for this device
        <input class="field" bind:value={name} maxlength={60} />
      </label>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <button class="btn btn-primary btn-block" disabled={busy || code.replace(/[^A-Z0-9]/g, '').length < 8}>
        {busy ? 'Signing in…' : 'Sign in'}
      </button>
    </form>
    <div class="pair-help">
      <p>
        <strong>Get a code</strong> on {theHost()} with <code>briardr pair</code>{isMac() ? '' : ' (as the user Briardr runs as)'}, or from Settings → Devices → Add a device on a device that’s already signed in.
      </p>
      <p class="hint">This device stays signed in. You can revoke it any time from Settings on any signed-in device.</p>
    </div>
  </div>
</main>
