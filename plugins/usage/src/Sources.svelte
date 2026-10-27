<script lang="ts">
  import { onMount } from 'svelte';
  import { ApiError, type SettingsProps } from '@briardr/sdk';

  let { briardr, s, save }: SettingsProps = $props();
  const toast = (text: string) => briardr.toast(text);
  let tap = $state.raw<{ installed: boolean; preview: any } | null>(null);
  let review = $state(false);
  const load = () => briardr.core('GET', '/api/usage/statusline').then((r) => (tap = r), () => {});
  onMount(load);

  async function remove() {
    const r = await briardr.core('POST', '/api/usage/statusline', { action: 'remove' });
    toast(r.restored === 'backup' ? 'Removed. Claude’s settings were restored from the backup.' : 'Removed. The original status line was restored.');
    load();
  }

  async function install() {
    try {
      await briardr.core('POST', '/api/usage/statusline', { action: 'install', fingerprint: tap!.preview.fingerprint });
      toast('Status-line tap installed');
      review = false;
      load();
    } catch (e) {
      toast(e instanceof ApiError ? e.message : 'Couldn’t install the tap.');
      load();
    }
  }
</script>

  {#if briardr.host().claudeKeychain}
<label class="switch-row">
  <input type="checkbox" role="switch" checked={!!s?.usage.claudeKeychainOptIn} onchange={(e) => save({ usage: { claudeKeychainOptIn: e.currentTarget.checked } })} />
  <span>
    Claude: read limits with Claude Code’s sign-in
    <small>Briardr reads Claude Code’s sign-in from the Mac’s Keychain for a read-only usage request to Anthropic. macOS may ask you on the Mac to allow it. The token is never stored or sent anywhere else.</small>
  </span>
</label>
{/if}
<label class="switch-row">
  <input type="checkbox" role="switch" checked={!!s?.usage.codexEnabled} onchange={(e) => save({ usage: { codexEnabled: e.currentTarget.checked } })} />
  <span>
    Codex: read limits through the Codex app server
    <small>Uses Codex’s own sign-in on {briardr.host().the} and only asks for rate limits; it never starts a session or turn.</small>
  </span>
</label>
<div class="tap">
  <p>
    <strong>Optional Claude status-line tap.</strong> Records the rate limits Claude Code already gives status-line commands, and keeps your existing status line exactly as it is.
  </p>
  {#if tap?.installed}
    <button class="btn" onclick={remove}>Remove tap</button>
  {:else}
    <button class="btn" disabled={!tap?.preview} onclick={() => (review = true)}>Review change…</button>
  {/if}
  {#if review && tap?.preview}
    <div class="tap-review">
      <p>Briardr will change <code>statusLine</code> in <code>{tap.preview.settingsPath}</code>, after saving a backup next to it:</p>
      <div class="diff">
        <pre><span class="dl-del">{'- ' + JSON.stringify(tap.preview.before)}</span>{'\n'}<span class="dl-add">{'+ ' + JSON.stringify(tap.preview.after)}</span></pre>
      </div>
      <p class="hint">{tap.preview.chained ? 'Your current status-line command keeps running with the same input, and its output is shown unchanged.' : 'You have no status line now; the tap prints nothing.'} Removing the tap restores the original exactly.</p>
      <div class="form-actions">
        <button class="btn" onclick={() => (review = false)}>Cancel</button>
        <button class="btn btn-primary" onclick={install}>Install tap</button>
      </div>
    </div>
  {/if}
</div>
