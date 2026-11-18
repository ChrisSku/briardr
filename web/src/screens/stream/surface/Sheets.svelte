<!--
  The sheet open over the terminal, if any: Compose, Paste text, sending images and files, or Select Text.
  The key bar's + menu, a paste, a drop and the terminal's menu open them.
-->
<script module lang="ts">
  import type { UploadItem } from '../insert.ts';

  export type SurfaceSheet = { k: 'compose' } | { k: 'paste'; text: string } | { k: 'upload'; items: UploadItem[] } | { k: 'select'; text: string; start: number; end: number };
</script>

<script lang="ts">
  import type { TerminalController } from '../../../terminal/controller.ts';
  import type { Stream } from '../../../types.ts';
  import ComposeSheet from '../ComposeSheet.svelte';
  import PasteSheet from '../PasteSheet.svelte';
  import SelectTextSheet from '../SelectTextSheet.svelte';
  import UploadSheet from '../UploadSheet.svelte';

  let { ctl, stream, sheet, onClose }: { ctl: TerminalController; stream: Stream; sheet: SurfaceSheet | null; onClose: () => void } = $props();
</script>

{#if sheet?.k === 'compose'}<ComposeSheet {ctl} {stream} {onClose} />{/if}
{#if sheet?.k === 'paste'}<PasteSheet {ctl} {stream} initial={sheet.text} {onClose} />{/if}
{#if sheet?.k === 'upload'}<UploadSheet {ctl} {stream} items={sheet.items} {onClose} />{/if}
{#if sheet?.k === 'select'}<SelectTextSheet text={sheet.text} start={sheet.start} end={sheet.end} {onClose} />{/if}
