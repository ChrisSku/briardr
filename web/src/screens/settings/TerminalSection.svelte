<script lang="ts">
  import { prefs } from '../../store.ts';
  import Section from './Section.svelte';

  const p = $derived(prefs.value);
</script>

<Section id="terminal" title="Terminal">
  <label class="field-label">
    Font size: {p.fontSize} pt
    <input type="range" min={5} max={22} step={1} value={p.fontSize} oninput={(e) => (prefs.value = { ...p, fontSize: Number(e.currentTarget.value) })} aria-valuetext="{p.fontSize} points" />
  </label>
  <pre class="font-preview" style:font-size="{p.fontSize}px" aria-label="Font preview">{'$ npm test\n✓ 42 passed  ✗ 0 failed\n> claude: Ready for input'}</pre>
  <label class="switch-row">
    <input type="checkbox" role="switch" checked={p.bracketedPaste} onchange={(e) => (prefs.value = { ...p, bracketedPaste: e.currentTarget.checked })} />
    <span>
      Bracketed paste by default
      <small>Pastes arrive as one block in programs that support it. You can still change it per paste.</small>
    </span>
  </label>
  <label class="switch-row">
    <input type="checkbox" role="switch" checked={p.screenReaderTerminal} onchange={(e) => (prefs.value = { ...p, screenReaderTerminal: e.currentTarget.checked })} />
    <span>
      Screen reader terminal mode
      <small>Exposes terminal rows to VoiceOver. Takes effect when you next open a stream.</small>
    </span>
  </label>
</Section>
