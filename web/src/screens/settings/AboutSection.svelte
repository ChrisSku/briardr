<script lang="ts">
  import { caps, hostNoun, theHost } from '../../host.ts';
  import Section from './Section.svelte';
  import type { Settings } from './settings.ts';

  let { s }: { s: Settings | null } = $props();

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
