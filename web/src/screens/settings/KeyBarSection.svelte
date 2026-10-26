<!--
  The key bar's top row: which keys it shows by default and, optionally, for an app (the program
  running in the pane), which then replace the default keys while it runs. Tap a key in the row to move or remove it; tap keys below to add or remove them.
-->
<script lang="ts">
  import { model } from '../../store.ts';
  import { appLabel, appOf, BAR_KEYS, barKeys, CATALOG, DEFAULT_KEYS, keyIds, MAX_KEYS } from '../../terminal/barkeys.ts';
  import { openMenu } from '../../ui/kit.ts';
  import Section from './Section.svelte';

  /** The app being edited; null for the default keys. */
  let app = $state<string | null>(null);
  const cfg = $derived(barKeys.value);
  const apps = $derived(Object.keys(cfg.apps ?? {}).sort((a, b) => appLabel(a).localeCompare(appLabel(b))));
  const ids = $derived(keyIds(app));
  const full = $derived(ids.length >= MAX_KEYS);

  function set(next: string[]) {
    barKeys.value = app ? { ...cfg, apps: { ...cfg.apps, [app]: next } } : { ...cfg, default: next };
  }
  const toggle = (id: string) => set(ids.includes(id) ? ids.filter((x) => x !== id) : [...ids, id]);
  function move(i: number, by: number) {
    const next = [...ids];
    [next[i], next[i + by]] = [next[i + by], next[i]];
    set(next);
  }

  function keyMenu(i: number, el: HTMLElement) {
    openMenu(
      [
        { label: 'Move Left', icon: 'back', disabled: i === 0, onSelect: () => move(i, -1) },
        { label: 'Move Right', icon: 'chevron', disabled: i === ids.length - 1, onSelect: () => move(i, 1) },
        { label: 'Remove', icon: 'trash', destructive: true, onSelect: () => toggle(ids[i]) },
      ],
      el,
      BAR_KEYS.get(ids[i])!.name,
    );
  }

  // Apps running now come first, then common ones.
  function addApp(el: HTMLElement) {
    const running = (model.value?.streams ?? []).map(appOf);
    const choices = [...new Set([...running, 'shell', 'claude', 'codex', 'nvim', 'vim'])].filter((a) => !apps.includes(a));
    openMenu(
      choices.map((a) => ({
        label: appLabel(a),
        hint: running.includes(a) ? 'Running now' : undefined,
        onSelect: () => {
          barKeys.value = { ...cfg, apps: { ...cfg.apps, [a]: keyIds(null) } };
          app = a;
        },
      })),
      el,
      'Keys for an app',
    );
  }

  function removeApp() {
    if (!app) return;
    const { [app]: _, ...rest } = cfg.apps;
    barKeys.value = { ...cfg, apps: rest };
    app = null;
  }
</script>

<Section id="keybar" title="Key bar">
  <p class="hint">The top row of keys above the on-screen keyboard. Give an app keys of its own to show them instead of the default keys while it runs in the pane.</p>
  <div class="segmented small keybar-apps" role="group" aria-label="Keys for">
    <button class={[!app && 'is-on']} aria-pressed={!app} onclick={() => (app = null)}>Default</button>
    {#each apps as a (a)}
      <button class={[app === a && 'is-on']} aria-pressed={app === a} onclick={() => (app = a)}>{appLabel(a)}</button>
    {/each}
    <button onclick={(e) => addApp(e.currentTarget)}>Add app…</button>
  </div>

  <div class="keybar-row" role="group" aria-label="Keys in the row{app ? ` for ${appLabel(app)}` : ''}">
    {#each ids as id, i (id)}
      <button class="key" aria-label="{BAR_KEYS.get(id)!.name}, key {i + 1} of {ids.length}" onclick={(e) => keyMenu(i, e.currentTarget)}>{BAR_KEYS.get(id)!.label}</button>
    {:else}
      <span class="muted">No keys: only Delete.</span>
    {/each}
  </div>
  <p class="hint">{ids.length} of {MAX_KEYS} keys. Delete always ends the row. Tap a key in the row to move or remove it.</p>

  {#each CATALOG as g (g.title)}
    <div class="keybar-catalog" role="group" aria-label={g.title}>
      <span class="field-label">{g.title}</span>
      <div class="keybar-keys">
        {#each g.keys as k (k.id)}
          {@const on = ids.includes(k.id)}
          <button class={['key', on && 'is-on']} aria-label={k.name} aria-pressed={on} disabled={!on && full} onclick={() => toggle(k.id)}>{k.label}</button>
        {/each}
      </div>
    </div>
  {/each}

  <div class="form-actions">
    {#if app}
      <button class="btn" onclick={removeApp}>Use the default keys</button>
    {:else}
      <button class="btn" disabled={ids.join() === DEFAULT_KEYS.join()} onclick={() => set(DEFAULT_KEYS)}>Reset to default</button>
    {/if}
  </div>
</Section>
