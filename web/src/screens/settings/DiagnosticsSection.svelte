<script lang="ts">
  import { onMount } from 'svelte';
  import { theHost, TheHost } from '../../host.ts';
  import { exportClientLog } from '../../diag.ts';
  import { checkOffline, keepStorage, offline, persistent, repairOffline, storageEstimate, type OfflineFailure } from '../../offline.ts';
  import { toast } from '../../store.ts';
  import Section from './Section.svelte';

  let usage = $state.raw<{ usage: number; quota: number } | null>(null);
  let repairing = $state(false);
  onMount(() => {
    void checkOffline();
    void storageEstimate().then((u) => (usage = u));
  });

  const FAILURES: Record<OfflineFailure, string> = {
    untrusted: 'The browser refused Briardr’s service worker. On iPhone and iPad this usually means Briardr’s certificate isn’t fully trusted yet (see About).',
    register: 'The browser couldn’t register Briardr’s service worker.',
    install: 'Saving the app for offline use didn’t finish (a file didn’t download, or Briardr was updated meanwhile).',
    incomplete: 'Some of the files Briardr needs offline were cleared from this device.',
  };
  const mb = (n: number) => (n < 1e9 ? `${Math.max(1, Math.round(n / 1e6))} MB` : `${(n / 1e9).toFixed(1)} GB`);

  async function repair() {
    repairing = true;
    const r = await repairOffline();
    repairing = false;
    toast(r.state === 'ready' ? 'Briardr is saved for offline use again.' : 'Repair didn’t finish. Try again when the connection is better.');
  }

  async function keep() {
    const kept = await keepStorage();
    toast(kept ? 'The browser will keep Briardr’s data.' : 'The browser didn’t agree to keep Briardr’s data. Briardr works the same either way.');
  }

  async function exportLogs() {
    let server = '';
    try {
      const r = await fetch('/api/diagnostics', { credentials: 'same-origin' });
      server = r.ok ? await r.text() : '';
    } catch {}
    const blob = new Blob([`# Briardr diagnostics\n# exported ${new Date().toISOString()}\n# This device\n`, exportClientLog(), `# ${TheHost()}\n`, server], { type: 'text/plain' });
    const a = document.createElement('a');
    a.href = URL.createObjectURL(blob);
    a.download = `briardr-diagnostics-${new Date().toISOString().slice(0, 19).replace(/:/g, '')}.txt`;
    a.click();
    setTimeout(() => URL.revokeObjectURL(a.href), 5000);
  }
</script>

<Section id="diagnostics" title="Diagnostics">
  <dl class="info offline-info">
    <dt>Offline app</dt>
    <dd>
      <span role="status">
        {#if offline.value.state === 'ready'}
          Ready. Briardr opens without a connection.
        {:else if offline.value.state === 'preparing'}
          Saving for offline use…
        {:else if offline.value.state === 'unsupported'}
          Not available in this browser.
        {:else}
          Not ready. {FAILURES[offline.value.failure]}
        {/if}
      </span>
    </dd>
    <dt>Storage</dt>
    <dd>
      {persistent.value === true ? 'Kept by the browser.' : persistent.value === false ? 'The browser may clear it when this device runs low on space.' : 'The browser doesn’t say whether it keeps it.'}
      {#if usage}{' '}{mb(usage.usage)} in use.{/if}
    </dd>
  </dl>
  <div class="form-actions">
    <button class="btn" disabled={repairing || offline.value.state === 'unsupported'} onclick={repair}>{repairing ? 'Repairing…' : 'Repair offline app'}</button>
    {#if persistent.value === false}<button class="btn" onclick={keep}>Ask to keep data</button>{/if}
  </div>
  <p class="hint">Repair saves the app for offline use again. Notes, drafts, pairing and settings on this device stay.</p>
  <p class="hint">Briardr records connection events on this device and on {theHost()}: timing, network changes, retry delays and failure categories, with anonymized IDs. It never records addresses, names, commands, terminal content or credentials. Logs rotate at about 1 MB.</p>
  <button class="btn" onclick={exportLogs}>Export logs</button>
</Section>
