<script lang="ts">
  import { setTheme, THEMES, themeChoice, type ThemeChoice } from '../../theme.ts';
  import Section from './Section.svelte';

  const CHOICES = Object.keys(THEMES) as ThemeChoice[];
  const choice = $derived(themeChoice());
</script>

<Section id="appearance" title="Appearance">
  <fieldset class="segmented" aria-label="Theme">
    {#each CHOICES as id}
      <label class={choice === id ? 'is-on' : ''}>
        <input type="radio" name="theme" value={id} checked={choice === id} onchange={() => setTheme(id)} />
        {THEMES[id]}
      </label>
    {/each}
  </fieldset>
  <p class="hint">{choice === 'system' ? 'Light or dark, as this device is set.' : `Always ${choice} on this device, in the terminal too.`}</p>
</Section>
