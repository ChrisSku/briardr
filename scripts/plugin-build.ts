// Builds a plugin's client for this Briardr: `bun scripts/plugin-build.ts <plugin folder> [--watch]`.
// Compiles with Briardr's own Svelte, so the components match the runtime they share with the app;
// imports of svelte/* and @briardr/sdk become Briardr's shared copies under /sdk/v1/. The output goes to
// <plugin>/dist with a build.json saying which runtime it was built for (the server only serves a
// build that matches its own, and `mise run build` rebuilds installed plugins after an update).
import fs from 'node:fs';
import path from 'node:path';
import type { BunPlugin } from 'bun';
import { runtimeKey, SDK_MODULES, SDK_PATH } from './sdk-modules.ts';
import { sveltePlugin, type SvelteWarning } from './svelte-plugin.ts';

interface Manifest {
  id: string;
  client?: string;
}

/** svelte, svelte/* and @briardr/sdk resolve to the copies Briardr serves; anything else Svelte offers is refused. */
const shared: BunPlugin = {
  name: 'briardr-shared',
  setup(build) {
    build.onResolve({ filter: /^(svelte|@briardr\/sdk)(\/.*)?$/ }, ({ path: spec }) => {
      const name = SDK_MODULES[spec];
      if (!name) throw new Error(`Plugins can’t import ${spec}. Briardr shares: ${Object.keys(SDK_MODULES).join(', ')}.`);
      return { path: `${SDK_PATH}/${name}.js`, external: true };
    });
  },
};

export interface BuildResult {
  ok: boolean;
  entry: string | null;
  errors: string[];
  warnings: SvelteWarning[];
}

/** Builds into `<dir>/dist`, or `out` (the built-in plugins' test builds them elsewhere). */
export async function buildPlugin(dir: string, out = path.join(dir, 'dist')): Promise<BuildResult> {
  const warnings: SvelteWarning[] = [];
  let manifest: Manifest;
  try {
    manifest = JSON.parse(fs.readFileSync(path.join(dir, 'briardr-plugin.json'), 'utf8'));
  } catch (e) {
    return { ok: false, entry: null, errors: [`Couldn’t read briardr-plugin.json: ${(e as Error).message}`], warnings };
  }
  const stamp = (entry: string | null) => fs.writeFileSync(path.join(out, 'build.json'), JSON.stringify({ runtime: runtimeKey(), entry, builtAt: Date.now() }, null, 2) + '\n');
  fs.rmSync(out, { recursive: true, force: true });
  fs.mkdirSync(out, { recursive: true });
  if (!manifest.client) {
    stamp(null);
    return { ok: true, entry: null, errors: [], warnings };
  }
  const result = await Bun.build({
    entrypoints: [path.resolve(dir, manifest.client)],
    target: 'browser',
    format: 'esm',
    minify: true,
    splitting: true,
    conditions: ['production', 'browser'],
    plugins: [shared, sveltePlugin({ dev: false, css: 'injected', onWarning: (w) => warnings.push(w) })],
    define: { 'process.env.NODE_ENV': '"production"' },
    outdir: out,
    naming: { entry: '[name].[hash].[ext]', chunk: 'chunk.[hash].[ext]', asset: '[name].[hash].[ext]' },
    sourcemap: 'linked',
    throw: false,
  });
  if (!result.success) return { ok: false, entry: null, errors: result.logs.map(String), warnings };
  const entry = result.outputs.find((o) => o.kind === 'entry-point');
  const rel = entry ? path.relative(out, entry.path) : null;
  stamp(rel);
  return { ok: true, entry: rel, errors: [], warnings };
}

function report(dir: string, r: BuildResult) {
  for (const w of r.warnings) console.warn(`svelte: ${w.file}${w.line ? ':' + w.line : ''} ${w.code}: ${w.message}`);
  if (r.ok) console.log(`built ${path.basename(dir)}${r.entry ? ` (${r.entry})` : ' (no client)'}`);
  else console.error(`couldn’t build ${path.basename(dir)}:\n${r.errors.join('\n')}`);
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const watch = args.includes('--watch');
  const dir = path.resolve(args.find((a) => !a.startsWith('--')) || '.');
  const first = await buildPlugin(dir);
  report(dir, first);
  if (!watch) process.exit(first.ok ? 0 : 1);
  // Rebuild on changes to the plugin's sources; the server sees the new build.json and tells devices.
  let timer: ReturnType<typeof setTimeout> | null = null;
  let building = false;
  let again = false;
  const run = async () => {
    if (building) return void (again = true);
    building = true;
    report(dir, await buildPlugin(dir));
    building = false;
    if (again) (again = false), run();
  };
  fs.watch(dir, { recursive: true }, (_e, file) => {
    if (!file || /^(dist|node_modules|\.git)(\/|$)/.test(String(file))) return;
    if (timer) clearTimeout(timer);
    timer = setTimeout(run, 150);
  });
  console.log(`watching ${dir} (Ctrl-C to stop)`);
}
