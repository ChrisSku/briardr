// Bun.build plugin for Svelte 5: compiles .svelte components and .svelte.ts/.svelte.js rune modules
// with the official compiler. Component styles (if any) become CSS modules that Bun bundles into the
// entry's stylesheet. Bun's source maps point at the compiled JavaScript (it doesn't chain a plugin's
// input source maps); composeSourceMap() maps them on into the components' own source.
import fs from 'node:fs';
import path from 'node:path';
import remapping, { type SourceMapInput } from '@jridgewell/remapping';
import type { BunPlugin } from 'bun';
import { compile, compileModule, type CompileOptions } from 'svelte/compiler';

/** Compiled output by file, reused while the source is unchanged (watch rebuilds compile only edits). */
const cache = new Map<string, { source: string; dev: boolean; cssMode?: string; code: string; map: SourceMapInput | null; css: string | null; warnings: SvelteWarning[] }>();

export interface SvelteWarning {
  file: string;
  code: string;
  message: string;
  line?: number;
}

/**
 * `dev` turns on the compiler's development checks (ownership warnings, readable errors). `css: 'injected'`
 * keeps component styles in the JavaScript, which adds them to the page when mounted (plugins: one file).
 */
export function sveltePlugin(opts: { dev: boolean; css?: 'external' | 'injected'; onWarning?: (w: SvelteWarning) => void }): BunPlugin {
  const ts = new Bun.Transpiler({ loader: 'ts', target: 'browser' });
  const css = new Map<string, string>();
  const common = { generate: 'client', dev: opts.dev } as const;
  const toWarnings = (file: string, warnings: { code: string; message: string; start?: { line: number } }[]) =>
    warnings.map((w) => ({ file: path.relative(process.cwd(), file), code: w.code, message: w.message, line: w.start?.line }));
  /** Compile `file` unless the same source was compiled before with the same options. */
  const cached = async (file: string, run: (source: string) => { code: string; map: SourceMapInput | null; css: string | null; warnings: SvelteWarning[] }) => {
    const source = await Bun.file(file).text();
    let hit = cache.get(file);
    if (!hit || hit.source !== source || hit.dev !== opts.dev || hit.cssMode !== opts.css) cache.set(file, (hit = { source, dev: opts.dev, cssMode: opts.css, ...run(source) }));
    for (const w of hit.warnings) opts.onWarning?.(w);
    return hit;
  };
  return {
    name: 'svelte',
    setup(build) {
      // Components: <script lang="ts"> is handled by the compiler itself (type syntax only).
      build.onLoad({ filter: /\.svelte$/ }, async ({ path: file }) => {
        const r = await cached(file, (source) => {
          const options: CompileOptions = { ...common, filename: file, css: opts.css || 'external', runes: true, discloseVersion: false };
          const out = compile(source, options);
          return { code: out.js.code, map: out.js.map as SourceMapInput, css: out.css?.code || null, warnings: toWarnings(file, out.warnings) };
        });
        let code = r.code;
        if (r.css) {
          css.set(file, r.css);
          code += `\nimport ${JSON.stringify('svelte-css:' + file)};`;
        }
        return { contents: code, loader: 'js' };
      });
      // Rune modules: strip TypeScript first, then let Svelte compile the runes. The TypeScript step has
      // no source map, so none is composed for these.
      build.onLoad({ filter: /\.svelte\.(ts|js)$/ }, async ({ path: file }) => {
        const r = await cached(file, (source) => {
          const out = compileModule(file.endsWith('.ts') ? ts.transformSync(source) : source, { ...common, filename: file });
          return { code: out.js.code, map: null, css: null, warnings: toWarnings(file, out.warnings) };
        });
        return { contents: r.code, loader: 'js' };
      });
      build.onResolve({ filter: /^svelte-css:/ }, ({ path: p }) => ({ path: p.slice('svelte-css:'.length), namespace: 'svelte-css' }));
      build.onLoad({ filter: /.*/, namespace: 'svelte-css' }, ({ path: p }) => ({ contents: css.get(p) || '', loader: 'css' }));
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
