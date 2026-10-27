<!-- Pairing: the only screen available before this device is signed in. The brand gets its moment here. -->
<script lang="ts">
  import { hostNoun, isMac, theHost } from '../host.ts';
  import { api, ApiError } from '../api.ts';
  import { record } from '../diag.ts';
  import { installSuggestion, standalone } from '../install.ts';
  import { navigate } from '../router.ts';
  import { acceptSession, startSession } from '../session.ts';
  import { Icon } from '../ui/icons/index.ts';

  let { reason }: { reason?: string } = $props();

  function defaultName(): string {
    const ua = navigator.userAgent;
    if (/iPad/.test(ua) || (navigator.platform === 'MacIntel' && navigator.maxTouchPoints > 1)) return 'iPad';
    if (/iPhone/.test(ua)) return 'iPhone';
    if (/Macintosh/.test(ua)) return 'Mac';
    return 'Device';
  }

  let code = $state('');
  let focused = $state(false);
  const chars = $derived(code.replace(/[^A-Z0-9]/g, '').slice(0, 8));
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
      acceptSession(r);
      // Start the live session in place; no reload needed.
      startSession();
      navigate({ name: 'agents' }, { replace: true });
    } catch (x) {
      error = x instanceof ApiError ? x.message : `Couldn’t reach ${theHost()}.`;
    } finally {
      busy = false;
    }
  }
</script>

<main class="pair">
  <div class="pair-card">
    <span class="brand-tile pair-tile" aria-hidden="true"><img src="/icons/briard.png" alt="" /></span>
    <h1>Sign in to Briardr</h1>
    <p class="lead">See and steer the agents running on your {hostNoun()}, from anywhere on your network.</p>
    {#if reason}<p class="note">{reason}</p>{/if}
    <form class="form" onsubmit={submit}>
      <div class="field-label">
        <label for="pair-code">Pairing code</label>
        <!-- One field (autofill and pasting work as usual), shown as eight cells in two groups of four, as
             briardr pair prints the code. -->
        <div class="code-cells">
          <!-- svelte-ignore a11y_autofocus -->
          <input
            id="pair-code"
            class="code-input mono"
            bind:value={() => code, (v) => (code = v.toUpperCase())}
            onfocus={() => (focused = true)}
            onblur={() => (focused = false)}
            autocomplete="one-time-code"
            autocapitalize="characters"
            autocorrect="off"
            spellcheck={false}
            inputmode="text"
            maxlength={9}
            autofocus
            required
          />
          <span class="cells" aria-hidden="true">
            {#each { length: 8 } as _, i}
              {#if i === 4}<span class="cell-dash">–</span>{/if}
              <span class={['cell', focused && i === Math.min(chars.length, 7) && 'is-active']}>{chars[i] ?? ''}</span>
            {/each}
          </span>
        </div>
      </div>
      <label class="field-label">
        Name for this device
        <input class="field" bind:value={name} maxlength={60} />
      </label>
      {#if error}<p class="error" role="alert">{error}</p>{/if}
      <button class="btn btn-primary btn-large btn-block" disabled={busy || chars.length < 8}>
        {busy ? 'Signing in…' : 'Sign in'}
      </button>
    </form>
    <div class="card pair-help">
      <span class="set-tile" aria-hidden="true"><Icon name="terminal" size={17} /></span>
      <div>
        <strong>Get a code on {theHost()}</strong>
        <p>Run <code>briardr pair</code> on {theHost()}{isMac() ? '' : ' (as the user Briardr runs as)'}, or open Settings → Devices → Add a device on a device that’s already signed in.</p>
        <p class="hint">This device stays signed in. You can revoke it any time from Settings on any signed-in device.</p>
      </div>
    </div>
  </div>
</main>
