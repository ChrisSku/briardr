<script lang="ts">
  import { onMount } from 'svelte';
  import { api, ApiError } from '../../api.ts';
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
  let code = $state.raw<{ code: string; expiresAt: number } | null>(null);
  const message = (e: unknown, fallback: string) => (e instanceof ApiError ? e.message : fallback);
  const load = () =>
    api<{ devices: Device[] }>('GET', '/api/devices').then(
      (r) => ((list = r.devices), (failed = null)),
      (e) => (failed = message(e, 'Couldn’t load the devices.')),
    );
  onMount(load);

  async function revoke(d: Device) {
    const ok = await confirmDialog(d.current ? 'Sign out this device?' : `Revoke ${d.name}?`, d.current ? signOutBody : revokeBody, d.current ? 'Sign Out' : 'Revoke', true);
    if (!ok) return;
    try {
      await api('DELETE', `/api/devices/${encodeURIComponent(d.id)}`);
    } catch (e) {
      // It still has access: say so, and show the list as the host has it.
      toast(message(e, d.current ? 'Couldn’t sign out.' : `Couldn’t revoke ${d.name}.`), 5000);
      return void load();
    }
    if (d.current) {
      session.value = { status: 'signed_out' };
      location.assign('/');
    } else load();
  }

  const rename = (d: Device) => openRename({ noun: 'device', path: `/api/devices/${encodeURIComponent(d.id)}/rename`, name: d.name, onRenamed: load });

  const addDevice = () =>
    api<{ code: string; expiresAt: number }>('POST', '/api/devices/pairing-code').then(
      (r) => (code = r),
      (e) => toast(message(e, 'Couldn’t create a pairing code.'), 5000),
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
      <p>On the new device, open <strong>{location.host}</strong> and enter:</p>
      <p class="code mono" aria-label="Pairing code {code.code.split('').join(' ')}">{code.code}</p>
      <p class="hint">Works once. Expires at {new Date(code.expiresAt).toLocaleTimeString()}.</p>
    </div>
  {/if}
</Section>
