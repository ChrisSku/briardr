<!--
  Who has the terminal. Being in control is the normal case and isn't shown. Watching (another device has
  control) floats a banner under the header, with Take over; it can be dismissed until the state changes.
  Reconnecting and errors go in the overlay above the header (App shows it) while the terminal is on
  screen. Take control stays in the stream's actions menu too.
-->
<script module lang="ts">
  import { record } from '../../../diag.ts';
  import type { TermStatus, TerminalController } from '../../../terminal/controller.ts';
  import { confirmDialog } from '../../../ui/kit.ts';

  /** Another window or device has control, or just took it: taking it from them needs confirming. */
  const contested = (st: TermStatus) => st.watchReason === 'busy' || st.watchReason === 'taken_over';

  /** Takes control: right away when no one has it, or from another client after confirming (it switches
   *  to watching). */
  export async function takeControl(ctl: TerminalController) {
    if (!contested(ctl.status.value)) return ctl.connect('control', false);
    const ok = await confirmDialog('Take over this stream?', takeOverBody, 'Take Over', true);
    if (ok) {
      record('term_takeover_confirmed', {});
      ctl.connect('control', true);
    }
  }
</script>

<script lang="ts">
  import { theHost } from '../../../host.ts';
  import { topOverlay } from '../../../store.ts';
  import { Icon } from '../../../ui/icons/index.ts';

  let { ctl, visible }: { ctl: TerminalController; visible: boolean } = $props();

  const st = $derived(ctl.status.value);
  const bannerKey = $derived(`${st.ownership}:${st.watchReason}`);
  let dismissed = $state<string | null>(null);
  $effect(() => {
    if (!visible) return;
    topOverlay.value = overlay;
    return () => {
      if (topOverlay.value === overlay) topOverlay.value = null;
    };
  });
  const watching = $derived(st.ownership === 'watching' && dismissed !== bannerKey);
  const watchText = $derived(
    st.watchReason === 'busy'
      ? 'Another window or device is in control'
      : st.watchReason === 'taken_over'
        ? 'Another window or device took control'
        : st.watchReason === 'released'
          ? `Handed back to ${theHost()}`
          : 'Read-only until you take control',
  );
</script>

{#snippet takeOverBody()}
  <p>Another window or device is controlling this pane. Taking over moves control here and resizes the pane to fit. The other one will switch to watching.</p>
{/snippet}

{#snippet overlay()}
  {#if st.ownership === 'reconnecting'}
    <div class="own-banner tone-warn" role="status">
      <span>{st.message || 'Reconnecting…'} Showing the last screen.</span>
      <button class="btn btn-small" onclick={() => ctl.retryNow()}>Retry now</button>
    </div>
  {:else if st.ownership === 'gone' || st.ownership === 'error'}
    <div class="own-banner tone-error" role="alert">
      <span>{st.message}</span>
      {#if st.ownership === 'error'}
        <button class="btn btn-small" onclick={() => location.reload()}>Reload</button>
      {/if}
    </div>
  {/if}
{/snippet}

{#if watching}
  <div class="own-banner watch-banner" role="status">
    <span class="watch-tile" aria-hidden="true"><Icon name="eye" size={18} /></span>
    <span class="watch-text"><strong>Watching</strong><span>{watchText}</span></span>
    <button class="btn btn-small btn-primary" onclick={() => takeControl(ctl)}>{contested(st) ? 'Take over' : 'Take control'}</button>
    <button class="icon-btn" aria-label="Dismiss" onclick={() => (dismissed = bannerKey)}>
      <Icon name="close" size={16} />
    </button>
  </div>
{/if}
