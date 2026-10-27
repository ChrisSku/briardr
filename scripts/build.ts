// Builds the web client into web/dist: hashed JS/CSS bundles, the service worker, static files,
// and index.html wired to the hashed names.
import { execFileSync } from 'node:child_process';
import crypto from 'node:crypto';
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';
import { buildPlugin } from './plugin-build.ts';
import { runtimeKey, SDK_MODULES, SDK_PATH, SDK_SOURCES } from './sdk-modules.ts';
import { COMPRESSED_TYPES } from './static.ts';
import { composeSourceMap, sveltePlugin, type SvelteWarning } from './svelte-plugin.ts';

const root = path.resolve(import.meta.dir, '..');
const web = path.join(root, 'web');
// BRIARDR_WEB_DIST builds somewhere else, e.g. so experiments don't replace what a running server serves.
const dist = process.env.BRIARDR_WEB_DIST ? path.resolve(process.env.BRIARDR_WEB_DIST) : path.join(web, 'dist');
const watch = process.argv.includes('--watch');
// --packages <file> also writes the npm packages bundled into the client there, for a release's
// third-party notices (scripts/package.ts).
const packagesAt = process.argv.indexOf('--packages');
const packagesFile = packagesAt > 0 && process.argv[packagesAt + 1] ? path.resolve(process.argv[packagesAt + 1]) : null;
/** Every file the bundles were made from (with --packages). */
const inputs = new Set<string>();

/** The git branch (or release tag) and commit the build comes from, shown in Settings → About ("unknown" outside git). */
function gitInfo(): { branch: string; commit: string } {
  const git = (...args: string[]) => execFileSync('git', args, { cwd: root, encoding: 'utf8', stdio: ['ignore', 'pipe', 'ignore'] }).trim();
  try {
    const commit = git('rev-parse', '--short', 'HEAD');
    let branch = 'detached HEAD';
    try {
      branch = git('symbolic-ref', '--short', '-q', 'HEAD') || branch;
    } catch {
      try {
        branch = git('describe', '--tags', '--exact-match', 'HEAD'); // a checkout of a release
      } catch {}
    }
    // Uncommitted changes to what goes into the client (other files, like bench results, don't count).
    const dirty = git('status', '--porcelain', '--', 'web', 'plugins', 'scripts', 'package.json', 'bun.lock');
    return { branch, commit: dirty ? `${commit}-dirty` : commit };
  } catch {
    return { branch: 'unknown', commit: 'unknown' };
  }
}
const attr = (s: string) => s.replace(/&/g, '&amp;').replace(/"/g, '&quot;').replace(/</g, '&lt;');

// Dotfiles in web/public (Finder's .DS_Store) are not part of the app.
const shipped = (name: string) => !name.startsWith('.');

async function bundle(config: Bun.BuildConfig): Promise<Bun.BuildArtifact[]> {
  const result = await Bun.build({ target: 'browser', minify: !watch, tsconfig: path.join(web, 'tsconfig.json'), metafile: !!packagesFile, ...config, throw: false });
  if (!result.success) throw new AggregateError(result.logs, result.logs.map(String).join('\n'));
  for (const input of Object.keys(result.metafile?.inputs ?? {})) inputs.add(path.resolve(input));
  return result.outputs;
}

/** The npm packages the bundles were made from: the folder after the last node_modules in each input's path. */
function bundledPackages(): { name: string; version: string; dir: string }[] {
  const dirs = new Set([...inputs].map((f) => f.match(/^.*[/\\]node_modules[/\\](?:@[^/\\]+[/\\])?[^/\\]+/)?.[0]).filter((d): d is string => !!d));
  return [...dirs]
    .map((dir) => {
      const { name, version } = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'));
      return { name, version, dir };
    })
    .sort((a, b) => a.name.localeCompare(b.name));
}

/** Data directories that may have plugins: this build's server's, the default one, and the dev server's. */
function dataDirs(): string[] {
  const home = process.env.HOME || '';
  const xdg = process.env.XDG_DATA_HOME && path.isAbsolute(process.env.XDG_DATA_HOME) ? process.env.XDG_DATA_HOME : path.join(home, '.local', 'share');
  const os = process.platform === 'darwin' ? path.join(home, 'Library', 'Application Support', 'Briardr') : path.join(xdg, 'briardr');
  return [...new Set([process.env.BRIARDR_DATA_DIR, os, path.join(root, '.dev-data')].filter((d): d is string => !!d && fs.existsSync(d)))];
}

/** Installed plugins built for another runtime (a Svelte update) or never built: build them for this one. */
async function rebuildPlugins() {
  const runtime = runtimeKey();
  for (const data of dataDirs()) {
    const plugins = path.join(data, 'plugins');
    if (!fs.existsSync(plugins)) continue;
    for (const id of fs.readdirSync(plugins).filter(shipped)) {
      const dir = path.join(plugins, id);
      let stamp: { runtime?: string } = {};
      try {
        stamp = JSON.parse(fs.readFileSync(path.join(dir, 'dist', 'build.json'), 'utf8'));
      } catch {}
      if (stamp.runtime === runtime || !fs.existsSync(path.join(dir, 'briardr-plugin.json'))) continue;
      const r = await buildPlugin(dir);
      console.log(r.ok ? `rebuilt plugin ${id} for ${runtime}` : `plugin ${id} didn’t build: ${r.errors.join('; ')}`);
    }
  }
}

/** One entry per module shared with plugins (scripts/sdk-modules.ts), bundled with the app so both use one copy. */
function runtimeEntries(): string[] {
  const dir = path.join(web, 'src', 'sdk', 'runtime');
  fs.mkdirSync(dir, { recursive: true });
  return Object.entries(SDK_MODULES).map(([spec, name]) => {
    const file = path.join(dir, `${name}.ts`);
    const text = `// Generated by scripts/build.ts: ${spec} for plugins.\nexport * from '${SDK_SOURCES[spec] ? `../${SDK_SOURCES[spec]}` : spec}';\n`;
    if (!fs.existsSync(file) || fs.readFileSync(file, 'utf8') !== text) fs.writeFileSync(file, text);
    return file;
  });
}

/** The built-in plugins (plugins/<id>) import @briardr/sdk like any other plugin; in the app it's the source. */
const sdkSource: Bun.BunPlugin = {
  name: 'briardr-sdk-source',
  setup(b) {
    b.onResolve({ filter: /^@briardr\/sdk(\/.*)?$/ }, ({ path: spec }) => {
      if (!SDK_SOURCES[spec]) throw new Error(`There’s no ${spec}. Briardr has: ${Object.keys(SDK_SOURCES).join(', ')}.`);
      return { path: path.join(web, 'src', 'sdk', SDK_SOURCES[spec]) };
    });
  },
};

async function build() {
  const warnings: SvelteWarning[] = [];
  fs.rmSync(dist, { recursive: true, force: true });
  fs.mkdirSync(path.join(dist, 'assets'), { recursive: true });
  const artifacts = (
    await bundle({
      entrypoints: [path.join(web, 'src', 'main.ts'), ...runtimeEntries()],
      // Svelte components and rune modules; the production condition drops Svelte's dev-only checks.
      plugins: [sdkSource, sveltePlugin({ dev: watch, onWarning: (w) => warnings.push(w) })],
      conditions: [watch ? 'development' : 'production'],
      format: 'esm',
      // Code loaded on demand with import() (the Files highlighter) goes into its own chunks.
      splitting: true,
      outdir: path.join(dist, 'assets'),
      naming: { entry: '[name].[hash].[ext]', chunk: '[name].[hash].[ext]', asset: '[name].[hash].[ext]' },
      // Watch builds link a map that points into the .svelte files (composed below). BRIARDR_SOURCEMAP=external
      // writes Bun's own maps next to the bundles, for bundle analysis (bench/sizes.ts); never served.
      sourcemap: watch ? 'linked' : process.env.BRIARDR_SOURCEMAP === 'external' ? 'external' : 'none',
      loader: { '.woff2': 'file', '.svg': 'text' },
      define: { 'process.env.NODE_ENV': watch ? '"development"' : '"production"' },
    })
  );
  const outputs = artifacts.map((o) => path.relative(dist, o.path));
  // Shared chunks take the entry's name too, so entries are found by kind.
  const entries = artifacts.filter((o) => o.kind === 'entry-point').map((o) => path.relative(dist, o.path));
  if (watch) for (const f of outputs) if (f.endsWith('.js') && fs.existsSync(path.join(dist, f + '.map'))) composeSourceMap(path.join(dist, f + '.map'));
  const js = entries.find((p) => /^assets\/main\.[a-z0-9]+\.js$/i.test(p))!;
  // Stable names for the shared modules, each re-exporting this build's hashed copy. Each page also maps
  // the stable names to its own build's copies (an import map), so a page still running the previous
  // build (until it reloads) never loads a plugin against the new build's Svelte.
  fs.mkdirSync(path.join(dist, SDK_PATH), { recursive: true });
  const runtime: Record<string, string> = {};
  for (const name of Object.values(SDK_MODULES)) {
    const out = entries.find((p) => new RegExp(`^assets/${name}\\.[a-z0-9]+\\.js$`, 'i').test(p));
    if (!out) throw new Error(`no bundle for the plugin runtime module ${name}`);
    fs.writeFileSync(path.join(dist, SDK_PATH, `${name}.js`), `export * from "/${out}";\n`);
    runtime[`${SDK_PATH}/${name}.js`] = `/${out}`;
  }
  fs.writeFileSync(path.join(dist, SDK_PATH, 'runtime.json'), JSON.stringify({ runtime: runtimeKey() }) + '\n');
  // Chunks the entry imports, directly or not: preloaded from index.html so they load side by side.
  const preload = new Set<string>();
  const visit = (file: string) => {
    for (const [, rel] of fs.readFileSync(path.join(dist, file), 'utf8').matchAll(/(?:from|import)\s*"\.\/([\w.-]+\.js)"/g)) {
      const dep = `assets/${rel}`;
      if (!preload.has(dep) && outputs.includes(dep)) preload.add(dep), visit(dep);
    }
  };
  visit(js);
  const css = outputs.find((p) => /main\.[a-z0-9]+\.css$/i.test(p));
  // Icons get content-hashed names too: iOS keeps a home-screen icon by URL, so a redrawn icon under
  // the old name would never be picked up. The plain names stay for anything that links them directly.
  const icons: Record<string, string> = {};
  for (const f of fs.readdirSync(path.join(web, 'public', 'icons')).filter(shipped)) {
    const hash = crypto.createHash('sha256').update(fs.readFileSync(path.join(web, 'public', 'icons', f))).digest('hex').slice(0, 10);
    icons[`/icons/${f}`] = `/icons/${f.replace(/(\.[a-z]+)$/, `.${hash}$1`)}`;
  }
  const withIcons = (text: string) => text.replace(/\/icons\/[\w.-]+\.png/g, (u) => icons[u] || u);
  const source = gitInfo();
  // The build's version names its offline shell and tells open pages to reload. It covers everything the
  // shell is made of: the bundles' names (content hashes) and the main bundle, the icons, the service
  // worker, the page and the other public files. Written in last, as a placeholder until then; the branch
  // and commit are left out, so a commit that doesn't change the client doesn't reload anyone.
  const VERSION = '%BRIARDR_VERSION%';
  await bundle({
    entrypoints: [path.join(web, 'src', 'sw.ts')],
    format: 'iife',
    outdir: dist,
    naming: 'sw.js',
    define: {
      __BRIARDR_VERSION__: JSON.stringify(VERSION),
      __BRIARDR_ICON__: JSON.stringify(icons['/icons/icon-192.png']),
      // Chunks too, so a client still on this version can load them after the next build replaced them.
      // The brand tile's dog is linked from the app by its plain name.
      __BRIARDR_ASSETS__: JSON.stringify(['/', `/${js}`, ...Object.values(SDK_MODULES).map((n) => `${SDK_PATH}/${n}.js`), ...(css ? [`/${css}`] : []), ...outputs.filter((p) => p.endsWith('.js') && p !== js).map((p) => `/${p}`), '/manifest.webmanifest', ...['icon-192.png', 'icon-512.png', 'apple-touch-icon.png'].map((f) => icons[`/icons/${f}`]), '/icons/briard.png']),
    },
  });

  const identity = crypto.createHash('sha256').update(outputs.join('|') + Object.values(icons).join('|')).update(fs.readFileSync(path.join(dist, js))).update(fs.readFileSync(path.join(dist, 'sw.js')));
  for (const f of (fs.readdirSync(path.join(web, 'public'), { recursive: true }) as string[]).sort()) {
    const src = path.join(web, 'public', f);
    if (!f.split(path.sep).every(shipped) || fs.statSync(src).isDirectory()) continue;
    fs.mkdirSync(path.dirname(path.join(dist, f)), { recursive: true });
    fs.copyFileSync(src, path.join(dist, f));
    if (icons[`/${f}`]) fs.copyFileSync(src, path.join(dist, icons[`/${f}`]));
    if (f !== 'manifest.webmanifest') identity.update(f).update(fs.readFileSync(src));
  }
  const manifest = path.join(dist, 'manifest.webmanifest');
  const manifestText = withIcons(fs.readFileSync(manifest, 'utf8'));
  fs.writeFileSync(manifest, manifestText);
  // The server's Content-Security-Policy allows the page's one inline script, the import map, by its hash.
  const importMap = JSON.stringify({ imports: runtime });
  fs.writeFileSync(path.join(dist, 'index.html.csp'), `'sha256-${crypto.createHash('sha256').update(importMap).digest('base64')}'\n`);
  const page = withIcons(fs.readFileSync(path.join(web, 'index.html'), 'utf8'))
    .replace('%IMPORT_MAP%', `<script type="importmap">${importMap}</script>`)
    .replace('%APP_JS%', `/${js}`)
    .replace('%APP_CSS%', (css ? `<link rel="stylesheet" href="/${css}">` : '') + [...preload].map((p) => `\n    <link rel="modulepreload" href="/${p}">`).join(''));
  const version = identity.update(manifestText).update(page).digest('hex').slice(0, 12);
  const release = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;
  const html = page.replace('%VERSION%', version).replace('%RELEASE%', attr(release)).replace('%BRANCH%', attr(source.branch)).replace('%COMMIT%', attr(source.commit));
  fs.writeFileSync(path.join(dist, 'index.html'), html);
  fs.writeFileSync(path.join(dist, 'sw.js'), fs.readFileSync(path.join(dist, 'sw.js'), 'utf8').replaceAll(VERSION, version));
  // Brotli and gzip copies of the text files, which the server sends to browsers that accept them
  // (server/src/static_files.rs). Not while watching: development builds should be quick.
  if (!watch)
    for (const f of fs.readdirSync(dist, { recursive: true }) as string[]) {
      const file = path.join(dist, f);
      if (!COMPRESSED_TYPES.has(path.extname(f)) || !fs.statSync(file).isFile()) continue;
      const raw = fs.readFileSync(file);
      const br = zlib.brotliCompressSync(raw, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: raw.length } });
      const gz = zlib.gzipSync(raw, { level: 9 });
      if (br.length < raw.length) fs.writeFileSync(file + '.br', br);
      if (gz.length < raw.length) fs.writeFileSync(file + '.gz', gz);
    }
  if (!watch) await rebuildPlugins();
  if (packagesFile) fs.writeFileSync(packagesFile, JSON.stringify(bundledPackages(), null, 2) + '\n');
  for (const w of warnings) console.warn(`svelte: ${w.file}${w.line ? ':' + w.line : ''} ${w.code}: ${w.message}`);
  console.log(`built ${js}${css ? ' + ' + css : ''} (version ${version}, ${source.branch} at ${source.commit})`);
}

if (watch) {
  // Rebuild when a file's contents change. macOS also reports files the build only reads (it copies
  // web/public/icons), which used to start a rebuild after every build; content hashes filter those
  // out. Builds run one at a time; edits made during a build start one more afterwards.
  const digests = new Map<string, string>();
  const digest = (file: string): string | null => {
    try {
      return fs.statSync(file).isDirectory() ? null : crypto.createHash('sha256').update(fs.readFileSync(file)).digest('hex');
    } catch {
      return 'deleted';
    }
  };
  const inDist = (file: string) => file === dist || file.startsWith(dist + path.sep);
  // The web client and the built-in plugins in plugins/ (whose builds, if any, aren't sources).
  const sources = [web, path.join(root, 'plugins')];
  const ignored = (file: string) => inDist(file) || /[/\\](dist|node_modules)([/\\]|$)/.test(path.relative(root, file));
  for (const dir of sources)
    for (const f of fs.readdirSync(dir, { recursive: true }) as string[]) {
      const file = path.join(dir, f);
      const d = ignored(file) ? null : digest(file);
      if (d) digests.set(file, d);
    }
  let building: Promise<void> | null = null;
  let again = false;
  const rebuild = async () => {
    if (building) return void (again = true);
    building = build().catch((e) => console.error(e.message));
    await building;
    building = null;
    if (again) {
      again = false;
      await rebuild();
    }
  };
  await rebuild();
  let timer: Timer | null = null;
  for (const dir of sources)
    fs.watch(dir, { recursive: true }, (_e, f) => {
      if (!f) return;
      const file = path.resolve(dir, String(f));
      const d = ignored(file) ? null : digest(file);
      if (!d || d === digests.get(file)) return;
      digests.set(file, d);
      if (timer) clearTimeout(timer);
      timer = setTimeout(rebuild, 150);
    });
} else {
  await build();
}
