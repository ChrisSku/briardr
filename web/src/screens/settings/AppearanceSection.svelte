<script lang="ts">
  import { setTheme, themeChoice, type ThemeChoice } from '../../theme.ts';
  import Section from './Section.svelte';

  const CHOICES: { id: ThemeChoice; label: string }[] = [
    { id: 'system', label: 'System' },
    { id: 'light', label: 'Light' },
    { id: 'dark', label: 'Dark' },
  ];
  const choice = $derived(themeChoice());
</script>

<Section id="appearance" title="Appearance">
  <fieldset class="segmented" aria-label="Theme">
    {#each CHOICES as c}
      <label class={choice === c.id ? 'is-on' : ''}>
        <input type="radio" name="theme" value={c.id} checked={choice === c.id} onchange={() => setTheme(c.id)} />
        {c.label}
      </label>
    {/each}
  </fieldset>
  <p class="hint">{choice === 'system' ? 'Light or dark, as this device is set.' : `Always ${choice} on this device, in the terminal too.`}</p>
</Section>
