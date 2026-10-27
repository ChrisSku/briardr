<script lang="ts">
  import { api, ApiError } from '../../api.ts';
  import { caps, hostNoun, theHost } from '../../host.ts';
  import { ago, confirmDialog } from '../../ui/kit.ts';
  import Section from './Section.svelte';
  import type { Save, Settings, UpdateStatus } from './settings.ts';

  let { s, save }: { s: Settings | null; save: Save } = $props();

  // Updates: what the host's last check found, and installing it (only a release installation can).
  let u = $state.raw<UpdateStatus | null>(null);
  let busy = $state<null | 'checking' | 'installing'>(null);
  let installed = $state.raw<{ version: string; restarts: boolean } | null>(null);
  let failed = $state<string | null>(null);
  api<UpdateStatus>('GET', '/api/update').then((r) => (u = r), () => {});

  async function check() {
    busy = 'checking';
    failed = null;
    try {
      u = await api<UpdateStatus>('POST', '/api/update/check', {});
    } catch (e) {
      failed = e instanceof ApiError ? e.message : `Couldn’t reach ${theHost()}.`;
    } finally {
      busy = null;
    }
  }

  async function install() {
    const version = u?.latest;
    if (!version || !(await confirmDialog(`Update to Briardr ${version}?`, installBody, 'Update'))) return;
    busy = 'installing';
    failed = null;
    try {
      // Downloading can take a while on a slow connection; the host gives up after five minutes.
      installed = await api('POST', '/api/update/install', { version }, { timeout: 330_000 });
    } catch (e) {
      failed = e instanceof ApiError ? e.message : `Couldn’t reach ${theHost()}. The update may still be installing; check again in a minute.`;
    } finally {
      busy = null;
    }
  }

  async function setCheck(on: boolean) {
    if (await save({ updates: { check: on } })) u = u && { ...u, check: on };
  }

  // The build this page came from (written into index.html by scripts/build.ts): what this device runs,
  // which can be older than what the Mac serves until the app is opened again.
  const meta = (name: string) => document.querySelector<HTMLMetaElement>(`meta[name="${name}"]`)?.content || 'unknown';
  const commit = meta('briardr-commit');
</script>

<Section id="about" title="About">
  <p>Briardr {s?.version} — a remote window into Herdr on your {hostNoun()}. Work lives in Herdr; Briardr shows what Herdr reports, with how old it is.</p>
  <dl class="info build-info">
    <dt>Build</dt>
    <dd class="mono">{meta('briardr-version')}</dd>
    <dt>Branch</dt>
    <dd class="mono">{meta('briardr-branch')}</dd>
    <dt>Commit</dt>
    <dd class="mono">{commit.replace(/-dirty$/, '')}{#if commit.endsWith('-dirty')}{' '}<span class="muted">(with uncommitted changes)</span>{/if}</dd>
  </dl>
  {#if installed}
    <p class="note" role="status">
      Briardr {installed.version} is installed. {installed.restarts ? `Briardr is restarting on it; this page reconnects by itself and then offers the new version.` : `Restart Briardr on ${theHost()} to run it.`} Herdr and your agents keep running.
    </p>
  {:else if u?.available}
    <p class="note" role="status">Briardr {u.latest} is available; this is {u.current}. <a href={u.notes} target="_blank" rel="noopener">What’s new</a></p>
    {#if u.canInstall}
      <button class="btn btn-primary" onclick={install} disabled={!!busy}>{busy === 'installing' ? 'Updating…' : `Update to ${u.latest}`}</button>
    {:else}
      <p class="hint">{u.how} <code>{u.command}</code></p>
    {/if}
  {:else if u?.latest}
    <p class="hint">Briardr is up to date.</p>
  {/if}
  {#if failed}<p class="error" role="alert">{failed}</p>{/if}
  {#if u && !installed}
    <p class="hint">
      {u.checkedAt ? `Checked for updates ${ago(Date.now() - u.checkedAt)}${u.error ? ', but GitHub couldn’t be reached' : ''}.` : 'Not checked for updates yet.'}
      <button class="btn btn-small" onclick={check} disabled={!!busy}>{busy === 'checking' ? 'Checking…' : 'Check now'}</button>
    </p>
    <label class="switch-row">
      <input type="checkbox" role="switch" checked={u.check} onchange={(e) => setCheck(e.currentTarget.checked)} />
      <span>
        Check for updates daily
        <small>The {hostNoun()} asks GitHub once a day whether a newer Briardr is out. Updating is always your choice.</small>
      </span>
    </label>
  {/if}
  {#if s && s.tls.wanted === 'tailscale' && s.tls.effective !== 'tailscale'}
    <p class="note">
      Waiting for Tailscale HTTPS certificates. Turn on <strong>HTTPS Certificates</strong> for your tailnet in the Tailscale admin console (DNS page); Briardr checks every 30 minutes and switches over automatically. Until then it uses its own certificate authority.
    </p>
  {/if}
  {#if s?.tls.effective === 'tailscale'}<p class="hint">Connection secured with your tailnet’s HTTPS certificate.</p>{/if}
  {#if s?.tls.effective === 'proxy'}<p class="hint">Connection secured by the HTTPS proxy in front of Briardr{s.publicOrigin ? ` at ${s.publicOrigin}` : ''}.</p>{/if}
  {#if s?.caProfileAvailable && s.tls.effective !== 'tailscale'}
    <p class="hint">
      This {hostNoun()} uses Briardr’s own certificate authority. To trust it on another device, install <a href="/api/ca.mobileconfig">the Briardr profile</a>, then enable full trust in Settings → General → About → Certificate Trust Settings.
    </p>
  {/if}
  {#if caps().dictation || s?.dictation}
    <p class="hint">Dictation: {s?.dictation ? 'on-device transcription on the Mac is available.' : 'on-device transcription isn’t available on this Mac.'}</p>
  {:else}
    <p class="hint">Dictation: {theHost()} doesn’t transcribe speech. Use your keyboard’s dictation in text fields such as notes and Paste text.</p>
  {/if}
</Section>

{#snippet installBody()}
  <p>The {hostNoun()} downloads it from GitHub, checks it against the release’s checksums and replaces this version.{u?.restarts ? ' Briardr then restarts on it and devices reconnect.' : ` Then restart Briardr on ${theHost()}.`} Herdr and your agents keep running.</p>
{/snippet}
