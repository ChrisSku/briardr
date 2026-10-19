<script lang="ts">
  import { onMount } from 'svelte';
  import { api } from '../../api.ts';
  import { session } from '../../store.ts';
  import { Icon } from '../../ui/icons/index.ts';
  import { ago, confirmDialog } from '../../ui/kit.ts';
  import Section from './Section.svelte';

  let list = $state.raw<any[] | null>(null);
  let code = $state.raw<{ code: string; expiresAt: number } | null>(null);
  const load = () => api('GET', '/api/devices').then((r) => (list = r.devices), () => {});
  onMount(load);

  async function revoke(d: any) {
    const ok = await confirmDialog(d.current ? 'Sign out this device?' : `Revoke ${d.name}?`, d.current ? signOutBody : revokeBody, d.current ? 'Sign Out' : 'Revoke', true);
    if (!ok) return;
    await api('DELETE', `/api/devices/${encodeURIComponent(d.id)}`);
    if (d.current) {
      session.value = { status: 'signed_out' };
      location.assign('/');
    } else load();
  }
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
            <span class="row-sub">Paired {new Date(d.createdAt).toLocaleDateString()} · last seen {ago(Date.now() - d.lastSeenAt)}</span>
          </span>
        </div>
        <button class="btn btn-small btn-danger-text" onclick={() => revoke(d)}>{d.current ? 'Sign out' : 'Revoke'}</button>
      </li>
    {/each}
  </ul>
  <button class="btn" onclick={() => api('POST', '/api/devices/pairing-code').then((r) => (code = r))}>Add a device</button>
  {#if code}
    <div class="pair-code" role="status">
      <p>On the new device, open <strong>{location.host}</strong> and enter:</p>
      <p class="code mono" aria-label="Pairing code {code.code.split('').join(' ')}">{code.code}</p>
      <p class="hint">Works once. Expires at {new Date(code.expiresAt).toLocaleTimeString()}.</p>
    </div>
  {/if}
</Section>
