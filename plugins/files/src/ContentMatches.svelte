<!-- Lines that contain what was searched for, under the file each is in. A line opens its file there. -->
<script lang="ts">
  import type { ContentMatch } from './files.ts';
  import FileIcon from './FileIcon.svelte';

  let { matches, selected, onOpen }: { matches: ContentMatch[]; selected: string | null; onOpen: (m: ContentMatch) => void } = $props();
  /** Each file once, with its lines, in the order they came. */
  const files = $derived.by(() => {
    const out: { path: string; changed: boolean; lines: ContentMatch[] }[] = [];
    for (const m of matches) {
      if (out.at(-1)?.path === m.path) out.at(-1)!.lines.push(m);
      else out.push({ path: m.path, changed: m.changed, lines: [m] });
    }
    return out;
  });
</script>

{#each files as f (f.path)}
  {@const slash = f.path.lastIndexOf('/') + 1}
  <li class="match-file">
    <div class={['match-head', selected === f.path && 'is-on']} title={f.path}>
      <FileIcon name={f.path.slice(slash)} folder={false} expanded={false} />
      <span class="match-name">{f.path.slice(slash)}{#if slash}<small class="ft-path">{f.path.slice(0, slash - 1)}</small>{/if}</span>
      {#if f.changed}<span class="ft-dot"></span>{/if}
    </div>
    <ul class="match-lines">
      {#each f.lines as m (m.line)}
        <li>
          <button class="ft-row match-line" aria-label={`${f.path}, line ${m.line}: ${m.text}`} onclick={() => onOpen(m)}>
            <span class="match-n">{m.line}</span>
            <span class="match-text mono">{m.text}</span>
          </button>
        </li>
      {/each}
    </ul>
  </li>
{/each}

<style>
  .match-file { margin-bottom: 4px; }
  .match-head { display: flex; align-items: center; gap: 8px; padding: 6px 8px; border-radius: 8px; font-weight: 600; }
  .match-head.is-on { background: color-mix(in srgb, var(--ink-default) 16%, transparent); }
  .match-name { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
  .match-lines { list-style: none; margin: 0; padding: 0; }
  .match-line { gap: 8px; font-weight: 400; }
  .match-n { flex: none; min-width: 4ch; text-align: right; color: var(--text-tertiary); font: var(--type-mono-small); font-variant-numeric: tabular-nums; }
  .match-text { flex: 1; min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-size: 0.8rem; color: var(--text-secondary); }
</style>
