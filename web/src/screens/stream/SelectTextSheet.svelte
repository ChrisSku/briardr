<script lang="ts">
  import { onMount } from 'svelte';
  import { toast } from '../../store.ts';
  import { Sheet } from '../../ui/kit.ts';

  let { text, start, end, onClose }: { text: string; start: number; end: number; onClose: () => void } = $props();
  let area: HTMLTextAreaElement;

  onMount(() => {
    // Uncontrolled on purpose: re-applying `value` would move the caret. Select after the sheet's own
    // focus handling has run.
    area.value = text;
    requestAnimationFrame(() => {
      area.focus();
      area.setSelectionRange(start, end);
    });
  });

  function copy() {
    const sel = area.value.slice(area.selectionStart, area.selectionEnd) || area.value;
    navigator.clipboard.writeText(sel).then(() => toast('Copied'));
  }
</script>

<Sheet title="Select text" {onClose} wide class="select-sheet">
  <p class="hint">A frozen copy of the screen. Nothing you do here is sent to the terminal.</p>
  <textarea bind:this={area} class="field mono select-area" readonly aria-label="Screen text" spellcheck={false}></textarea>
  {#snippet footer()}
    <button class="btn" onclick={onClose}>Done</button>
    <button class="btn btn-primary" onclick={copy}>Copy</button>
  {/snippet}
</Sheet>
