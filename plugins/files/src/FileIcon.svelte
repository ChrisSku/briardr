<!-- Small, theme-aware file emblems. All artwork is local and decorative. -->
<script module lang="ts">
  import type { FileIconKind } from './files.ts';

  const SHAPES: Record<Exclude<FileIconKind, 'badge'> | 'folder' | 'folder-open', string> = {
    file: '<path d="M6 3h8l4 4v14H6z" fill="currentColor" fill-opacity=".08"/><path d="M14 3v5h4"/>',
    code: '<path d="m8 7-5 5 5 5m8-10 5 5-5 5m-3-12-2 14"/>',
    react: '<ellipse cx="12" cy="12" rx="10" ry="4"/><ellipse cx="12" cy="12" rx="10" ry="4" transform="rotate(60 12 12)"/><ellipse cx="12" cy="12" rx="10" ry="4" transform="rotate(120 12 12)"/><circle cx="12" cy="12" r="1.8" fill="currentColor" stroke="none"/>',
    vue: '<path d="M2 4h5l5 9 5-9h5L12 21z" fill="currentColor" fill-opacity=".15"/><path d="m7 4 5 9 5-9M9 4l3 5 3-5"/>',
    svelte: '<path d="m17 4-7 4a4 4 0 0 0 4 7l3-2a5 5 0 0 0-5-8L7 8m0 12 7-4a4 4 0 0 0-4-7l-3 2a5 5 0 0 0 5 8l5-3" stroke-width="2.5"/>',
    braces: '<path d="M9 3H7a2 2 0 0 0-2 2v4l-2 3 2 3v4a2 2 0 0 0 2 2h2m6-18h2a2 2 0 0 1 2 2v4l2 3-2 3v4a2 2 0 0 1-2 2h-2"/><circle cx="10" cy="12" r=".8" fill="currentColor"/><circle cx="14" cy="12" r=".8" fill="currentColor"/>',
    markdown: '<rect x="2" y="4" width="20" height="16" rx="2" fill="currentColor" fill-opacity=".08"/><path d="M5 16V8l3 4 3-4v8m6-8v8m-3-3 3 3 3-3"/>',
    text: '<path d="M6 3h8l4 4v14H6z" fill="currentColor" fill-opacity=".08"/><path d="M14 3v5h4M9 12h6m-6 4h6"/>',
    image: '<rect x="2.5" y="3.5" width="19" height="17" rx="2.5" fill="currentColor" fill-opacity=".1"/><circle cx="8" cy="9" r="2"/><path d="m3 18 5-5 4 4 4-6 5 6"/>',
    config: '<path d="M5 3v18M12 3v18M19 3v18"/><path d="M2 8h6m1 8h6m1-9h6" stroke-width="4"/>',
    git: '<rect x="4.5" y="4.5" width="15" height="15" rx="2" transform="rotate(45 12 12)" fill="currentColor" fill-opacity=".12"/><path d="m8 4 4 4v9m0-9 5 5"/><circle cx="12" cy="8" r="1.5" fill="currentColor"/><circle cx="12" cy="17" r="1.5" fill="currentColor"/><circle cx="17" cy="13" r="1.5" fill="currentColor"/>',
    lock: '<rect x="4" y="10" width="16" height="11" rx="2" fill="currentColor" fill-opacity=".12"/><path d="M7 10V7a5 5 0 0 1 10 0v3m-5 5v2"/>',
    package: '<path d="m12 2 9 5v10l-9 5-9-5V7z" fill="currentColor" fill-opacity=".1"/><path d="m3 7 9 5 9-5m-9 5v10M7.5 4.5l9 5V14"/>',
    terminal: '<rect x="2" y="4" width="20" height="16" rx="3" fill="currentColor" fill-opacity=".1"/><path d="m6 9 4 3-4 3m7 0h5"/>',
    database: '<ellipse cx="12" cy="5" rx="8" ry="3" fill="currentColor" fill-opacity=".12"/><path d="M4 5v14c0 4 16 4 16 0V5M4 12c0 4 16 4 16 0"/>',
    archive: '<path d="M5 3h14v18H5z" fill="currentColor" fill-opacity=".1"/><path d="M11 3h2m-2 3h2m-2 3h2m-2 3h2" stroke-width="2.5"/><rect x="10" y="15" width="4" height="4" rx="1"/>',
    audio: '<path d="M10 17V5l10-2v12M10 8l10-2"/><ellipse cx="6.5" cy="18" rx="3.5" ry="3" fill="currentColor" fill-opacity=".2"/><ellipse cx="16.5" cy="16" rx="3.5" ry="3" fill="currentColor" fill-opacity=".2"/>',
    video: '<rect x="2" y="4" width="20" height="16" rx="2" fill="currentColor" fill-opacity=".1"/><path d="m10 8 6 4-6 4z" fill="currentColor" stroke="none"/><path d="M5 4v16m14-16v16M2 9h3m-3 6h3m14-6h3m-3 6h3"/>',
    font: '<path d="m3 20 6-16 6 16M5 15h8m3-3c6-3 6 2 6 4v4m0-5c-8-1-7 7 0 3"/>',
    docker: '<path d="M2 12h17c2 0 3-2 3-3-2-1-3 0-4 1M2 12c0 7 13 11 18 0" fill="currentColor" fill-opacity=".12"/><path d="M4 8h4v4H4zm4 0h4v4H8zm4 0h4v4h-4zM8 4h4v4H8zm4 0h4v4h-4z" stroke-width="1.3"/>',
    folder: '<path d="M3 6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v10a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" fill="currentColor" fill-opacity=".16"/><path d="M3 10h18"/>',
    'folder-open': '<path d="M3 18V6a2 2 0 0 1 2-2h5l2 3h7a2 2 0 0 1 2 2v2"/><path d="M5 11h17l-3 10H3z" fill="currentColor" fill-opacity=".2"/>',
  };
</script>

<script lang="ts">
  import { fileIconOf } from './files.ts';

  let { name, folder = false, expanded = false, size = 19 }: { name: string; folder?: boolean; expanded?: boolean; size?: number } = $props();
  const spec = $derived(fileIconOf(name));
  const kind = $derived(folder ? (expanded ? 'folder-open' : 'folder') : spec.kind);
</script>

<svg
  class="file-icon"
  style:color={folder ? 'var(--text-secondary)' : spec.tint === 'plain' ? 'var(--text-tertiary)' : `var(--tint-${spec.tint})`}
  width={size}
  height={size}
  viewBox="0 0 24 24"
  fill="none"
  stroke="currentColor"
  stroke-width="1.7"
  stroke-linecap="round"
  stroke-linejoin="round"
  aria-hidden="true"
  focusable="false"
>
  {#if kind === 'badge'}
    <rect x="2" y="2" width="20" height="20" rx="4" fill="currentColor" fill-opacity=".12" stroke-opacity=".55" />
    <text x="12" y="13" text-anchor="middle" dominant-baseline="middle" font-family="ui-monospace, SFMono-Regular, Menlo, monospace" font-size={(spec.badge?.length ?? 0) > 2 ? 8 : 10} font-weight="800" letter-spacing="-.5" fill="currentColor" stroke="none">{spec.badge}</text>
  {:else}
    {@html SHAPES[kind]}
  {/if}
</svg>

<style>
  .file-icon { flex: none; display: block; }
</style>
