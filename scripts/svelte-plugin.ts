// Bun.build plugin for Svelte 5: compiles .svelte components and .svelte.ts/.svelte.js rune modules
// with the official compiler. A component's <style> stays in its JavaScript, which adds it to the page
// when the component is first mounted, so it arrives with the code wherever that is: the app's entry, a
// chunk loaded on demand (a built-in plugin) or an installed plugin. (As CSS, a chunk's styles would go
// into a stylesheet of its own that nothing loads.) The Content-Security-Policy allows inline styles. Bun's
// source maps point at the compiled JavaScript (it doesn't chain a plugin's input source maps);
// composeSourceMap() maps them on into the components' own source.
import fs from 'node:fs';
import path from 'node:path';
import remapping, { type SourceMapInput } from '@jridgewell/remapping';
import type { BunPlugin } from 'bun';
import { compile, compileModule, type CompileOptions } from 'svelte/compiler';

/** Compiled output by file, reused while the source is unchanged (watch rebuilds compile only edits). */
const cache = new Map<string, { source: string; dev: boolean; code: string; map: SourceMapInput | null; warnings: SvelteWarning[] }>();

export interface SvelteWarning {
  file: string;
  code: string;
  message: string;
  line?: number;
}

/** `dev` turns on the compiler's development checks (ownership warnings, readable errors). */
export function sveltePlugin(opts: { dev: boolean; onWarning?: (w: SvelteWarning) => void }): BunPlugin {
  const ts = new Bun.Transpiler({ loader: 'ts', target: 'browser' });
  const common = { generate: 'client', dev: opts.dev } as const;
  const toWarnings = (file: string, warnings: { code: string; message: string; start?: { line: number } }[]) =>
    warnings.map((w) => ({ file: path.relative(process.cwd(), file), code: w.code, message: w.message, line: w.start?.line }));
  /** Compile `file` unless the same source was compiled before with the same options. */
  const cached = async (file: string, run: (source: string) => { code: string; map: SourceMapInput | null; warnings: SvelteWarning[] }) => {
    const source = await Bun.file(file).text();
    let hit = cache.get(file);
    if (!hit || hit.source !== source || hit.dev !== opts.dev) cache.set(file, (hit = { source, dev: opts.dev, ...run(source) }));
    for (const w of hit.warnings) opts.onWarning?.(w);
    return hit;
  };
  return {
    name: 'svelte',
    setup(build) {
      // Components: <script lang="ts"> is handled by the compiler itself (type syntax only).
      build.onLoad({ filter: /\.svelte$/ }, async ({ path: file }) => {
        const r = await cached(file, (source) => {
          const options: CompileOptions = { ...common, filename: file, css: 'injected', runes: true, discloseVersion: false };
          const out = compile(source, options);
          return { code: out.js.code, map: out.js.map as SourceMapInput, warnings: toWarnings(file, out.warnings) };
        });
        return { contents: r.code, loader: 'js' };
      });
      // Rune modules: strip TypeScript first, then let Svelte compile the runes. The TypeScript step has
      // no source map, so none is composed for these.
      build.onLoad({ filter: /\.svelte\.(ts|js)$/ }, async ({ path: file }) => {
        const r = await cached(file, (source) => {
          const out = compileModule(file.endsWith('.ts') ? ts.transformSync(source) : source, { ...common, filename: file });
          return { code: out.js.code, map: null, warnings: toWarnings(file, out.warnings) };
        });
        return { contents: r.code, loader: 'js' };
      });
    },
  };
}

/**
 * Rewrite a bundle's source map (Bun's, written next to the bundle) so that code from components maps
 * to the .svelte source as written, not to the compiler's output.
 */
export function composeSourceMap(mapFile: string) {
  const map = JSON.parse(fs.readFileSync(mapFile, 'utf8'));
  const dir = path.dirname(mapFile);
  const composed = remapping(map, (source, ctx) => {
    const file = path.resolve(dir, source);
    // Only the bundle's own sources; the compiler's map names the same file again.
    const hit = ctx.depth === 1 && file.endsWith('.svelte') ? cache.get(file) : undefined;
    // The compiler names its source by file name; give it the path the bundle's map uses.
    return hit?.map ? ({ ...(hit.map as object), sources: [source], sourcesContent: [hit.source] } as SourceMapInput) : null;
  });
  fs.writeFileSync(mapFile, JSON.stringify(composed));
}
