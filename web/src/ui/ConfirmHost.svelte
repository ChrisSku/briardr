<script lang="ts">
  import { confirmReq } from './overlays.ts';
  import Sheet from './Sheet.svelte';

  const r = $derived(confirmReq.value);
  function done(ok: boolean) {
    const req = confirmReq.value;
    confirmReq.value = null;
    req?.resolve(ok);
  }
</script>

{#if r}
  <Sheet title={r.title} onClose={() => done(false)} class="confirm" initialFocus=".btn-cancel">
    <div class="confirm-body">
      {#if typeof r.body === 'string'}{r.body}{:else}{@render r.body()}{/if}
    </div>
    {#snippet footer()}
      <button class="btn btn-cancel" onclick={() => done(false)}>Cancel</button>
      <button class="btn {r.destructive ? 'btn-danger' : 'btn-primary'}" onclick={() => done(true)}>{r.confirm}</button>
    {/snippet}
  </Sheet>
{/if}
