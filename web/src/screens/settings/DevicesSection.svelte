<script lang="ts">
  import { onMount } from 'svelte';
  import { api, errorMessage } from '../../api.ts';
  import { session, toast } from '../../store.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import { ago, confirmDialog } from '../../ui/kit.ts';
  import { openRename } from '../RenameSheet.svelte';
  import Section from './Section.svelte';

  interface Device {
    id: string;
    name: string;
    createdAt: number;
    lastSeenAt: number | null;
    /** The device showing this list. */
    current: boolean;
    /** It gets alerts. */
    push: boolean;
  }
  let list = $state.raw<Device[] | null>(null);
  let failed = $state<string | null>(null);
  /** A one-time code, the link that opens Briardr with it filled in (where this device reached it, or the
   *  host's address when it's the host itself), and that link as a QR code's rows of 0 and 1. */
  interface PairingCode {
    code: string;
    expiresAt: number;
    link: string;
    qr: string[] | null;
  }
  let code = $state.raw<PairingCode | null>(null);
  const opens = $derived(code && new URL(code.link).host);
  /** The dark modules as one path, a unit square each, inside a quiet zone of four. */
  const qrPath = (rows: string[]) => rows.flatMap((row, y) => Array.from(row, (m, x) => (m === '1' ? `M${x + 4} ${y + 4}h1v1h-1z` : ''))).join('');
  const load = () =>
    api<{ devices: Device[] }>('GET', '/api/devices').then(
      (r) => ((list = r.devices), (failed = null)),
      (e) => (failed = errorMessage(e, 'Couldn’t load the devices.')),
    );
  onMount(load);

  async function revoke(d: Device) {
    const ok = await confirmDialog(d.current ? 'Sign out this device?' : `Revoke ${d.name}?`, d.current ? signOutBody : revokeBody, d.current ? 'Sign Out' : 'Revoke', true);
    if (!ok) return;
    try {
      await api('DELETE', `/api/devices/${encodeURIComponent(d.id)}`);
    } catch (e) {
      // It still has access: say so, and show the list as the host has it.
      toast(errorMessage(e, d.current ? 'Couldn’t sign out.' : `Couldn’t revoke ${d.name}.`), 5000);
      return void load();
    }
    if (d.current) {
      session.value = { status: 'signed_out' };
      location.assign('/');
    } else load();
  }

  const rename = (d: Device) => openRename({ noun: 'device', path: `/api/devices/${encodeURIComponent(d.id)}/rename`, name: d.name, onRenamed: load });

  const addDevice = () =>
    api<PairingCode>('POST', '/api/devices/pairing-code').then(
      (r) => (code = r),
      (e) => toast(errorMessage(e, 'Couldn’t create a pairing code.'), 5000),
    );
</script>

{#snippet signOutBody()}<p>You’ll need a new pairing code to use Briardr here again.</p>{/snippet}
{#snippet revokeBody()}<p>It loses access immediately, including live terminals and alerts.</p>{/snippet}

<Section id="devices" title="Signed-in devices">
  <ul class="rows">
    {#each list ?? [] as d}
      <li class="row">
        <div class="row-main static">
          <Icon name="device" />
          <span class="row-text">
            <span class="row-title">{d.name}{#if d.current}<span class="tag">This device</span>{/if}{#if d.push}<span class="tag">Alerts</span>{/if}</span>
            <span class="row-sub">Paired {new Date(d.createdAt).toLocaleDateString()}{#if d.lastSeenAt} · last seen {ago(Date.now() - d.lastSeenAt)}{/if}</span>
          </span>
        </div>
        <button class="icon-btn" aria-label="Rename {d.name}" onclick={() => rename(d)}><Icon name="pencil" size={16} /></button>
        <button class="btn btn-small btn-danger-text" onclick={() => revoke(d)}>{d.current ? 'Sign out' : 'Revoke'}</button>
      </li>
    {/each}
  </ul>
  {#if failed}
    <p class="error" role="alert">{failed} <button class="btn btn-small" onclick={load}>Try again</button></p>
  {/if}
  <button class="btn" onclick={addDevice}>Add a device</button>
  {#if code}
    <div class="pair-code" role="status">
      {#if code.qr}
        <!-- Black on white whatever the theme: that's what a camera reads. -->
        <svg class="pair-qr" viewBox="0 0 {code.qr.length + 8} {code.qr.length + 8}" role="img" aria-label="QR code that opens Briardr on the new device with this code filled in">
          <rect width="100%" height="100%" fill="#fff" />
          <path fill="#000" d={qrPath(code.qr)} />
        </svg>
        <p>Scan this with the new device’s camera, or open <strong>{opens}</strong> there and enter:</p>
      {:else}
        <p>On the new device, open <strong>{opens}</strong> and enter:</p>
      {/if}
      <p class="code mono" aria-label="Pairing code {code.code.split('').join(' ')}">{code.code}</p>
      <p class="hint">Works once. Expires at {new Date(code.expiresAt).toLocaleTimeString()}.</p>
    </div>
  {/if}
</Section>

<style>
  .pair-qr { display: block; width: min(220px, 100%); margin: 0 auto 10px; border-radius: 8px; }
</style>
