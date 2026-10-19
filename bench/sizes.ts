// Bundle sizes for a built web client.
//   bun bench/sizes.ts <dist> [--maps <dist built with BRIARDR_SOURCEMAP=external>] [--out file.json]
// Every shipped file in <dist> is measured uncompressed, gzip (level 9) and Brotli (quality 11), each
// compressed on its own; totals are sums. Source maps are never counted. With --maps, the JavaScript is
// split by source package using the source map (raw bytes, plus each group's code gzipped on its own).
import fs from 'node:fs';
import path from 'node:path';
import zlib from 'node:zlib';

export interface FileSize {
  file: string;
  category: 'app-js' | 'service-worker' | 'css' | 'html' | 'manifest' | 'icon' | 'other';
  raw: number;
  gzip: number;
  brotli: number;
}

export function gzip(b: Buffer | Uint8Array): number {
  return zlib.gzipSync(b, { level: 9 }).length;
}
export function brotli(b: Buffer | Uint8Array): number {
  return zlib.brotliCompressSync(b, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11, [zlib.constants.BROTLI_PARAM_MODE]: zlib.constants.BROTLI_MODE_GENERIC } }).length;
}

function category(rel: string): FileSize['category'] {
  if (rel === 'sw.js') return 'service-worker';
  if (/^assets\/.*\.js$/.test(rel)) return 'app-js';
  if (rel.endsWith('.css')) return 'css';
  if (rel.endsWith('.html')) return 'html';
  if (rel.endsWith('.webmanifest')) return 'manifest';
  if (rel.startsWith('icons/')) return 'icon';
  return 'other';
}

export function measure(dist: string): FileSize[] {
  const out: FileSize[] = [];
  for (const rel of (fs.readdirSync(dist, { recursive: true }) as string[]).sort()) {
    const abs = path.join(dist, rel);
    // Builds before dotfiles were skipped shipped Finder's .DS_Store from web/public; it is not part of the app.
    // The build's .br/.gz copies are other encodings of files measured here.
    if (fs.statSync(abs).isDirectory() || /\.(map|br|gz)$/.test(rel) || path.basename(rel) === '.DS_Store') continue;
    const b = fs.readFileSync(abs);
    out.push({ file: rel, category: category(rel), raw: b.length, gzip: gzip(b), brotli: brotli(b) });
  }
  return out;
}

export function sum(files: FileSize[]) {
  return files.reduce((a, f) => ({ raw: a.raw + f.raw, gzip: a.gzip + f.gzip, brotli: a.brotli + f.brotli, files: a.files + 1 }), { raw: 0, gzip: 0, brotli: 0, files: 0 });
}

export function totals(files: FileSize[]) {
  const by = (c: FileSize['category'][]) => sum(files.filter((f) => c.includes(f.category)));
  return {
    appJs: by(['app-js']),
    serviceWorker: by(['service-worker']),
    shippedJs: by(['app-js', 'service-worker']),
    css: by(['css']),
    otherAssets: by(['html', 'manifest', 'icon', 'other']),
    icons: by(['icon']),
    total: sum(files),
  };
}

// ---------- composition (source map) ----------

const B64 = 'ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/';
function decodeVlq(seg: string): number[] {
  const out: number[] = [];
  let shift = 0;
  let value = 0;
  for (const ch of seg) {
    const d = B64.indexOf(ch);
    value += (d & 31) << shift;
    if (d & 32) shift += 5;
    else {
      out.push(value & 1 ? -(value >>> 1) : value >>> 1);
      value = 0;
      shift = 0;
    }
  }
  return out;
}

/** Package-level group of a source path from the map. */
export function groupOf(src: string): string {
  const nm = src.lastIndexOf('node_modules/');
  // The built-in plugins (plugins/<id>/src) are the app's too.
  if (nm < 0) return src.includes('web/src/') || /(^|\/)plugins\/[^/]+\/src\//.test(src) ? 'app' : 'other';
  const rest = src.slice(nm + 'node_modules/'.length).split('/');
  return rest[0].startsWith('@') ? `${rest[0]}/${rest[1]}` : rest[0];
}

export const FAMILY: Record<string, string> = {
  preact: 'Preact runtime',
  '@preact/signals': 'Preact runtime',
  '@preact/signals-core': 'Preact runtime',
  svelte: 'Svelte runtime',
  'esm-env': 'Svelte runtime',
  clsx: 'Svelte runtime',
  '@xterm/xterm': 'xterm.js',
  '@xterm/addon-webgl': 'xterm.js',
  '@xterm/addon-serialize': 'xterm.js',
  '@xterm/addon-unicode11': 'xterm.js',
  'markdown-it': 'Markdown (markdown-it)',
  mdurl: 'Markdown (markdown-it)',
  'uc.micro': 'Markdown (markdown-it)',
  entities: 'Markdown (markdown-it)',
  'linkify-it': 'Markdown (markdown-it)',
  'punycode.js': 'Markdown (markdown-it)',
  app: 'Application code',
};

export function composition(js: string, map: any) {
  const lines = js.split('\n');
  const groups = new Map<string, { raw: number; code: string[] }>();
  const add = (g: string, text: string) => {
    const e = groups.get(g) || { raw: 0, code: [] };
    e.raw += Buffer.byteLength(text);
    e.code.push(text);
    groups.set(g, e);
  };
  let src = 0;
  const mlines = String(map.mappings).split(';');
  for (let li = 0; li < lines.length; li++) {
    const line = lines[li];
    const segs = (mlines[li] || '').split(',').filter(Boolean);
    let col = 0;
    const marks: { col: number; src: number | null }[] = [];
    for (const s of segs) {
      const v = decodeVlq(s);
      col += v[0];
      if (v.length >= 4) {
        src += v[1];
        marks.push({ col, src });
      } else marks.push({ col, src: null });
    }
    if (!marks.length || marks[0].col > 0) add('unmapped', line.slice(0, marks[0]?.col ?? line.length));
    for (let i = 0; i < marks.length; i++) {
      const end = i + 1 < marks.length ? marks[i + 1].col : line.length;
      const text = line.slice(marks[i].col, end);
      add(marks[i].src === null ? 'unmapped' : groupOf(map.sources[marks[i].src!]), text);
    }
    if (li < lines.length - 1) add('unmapped', '\n');
  }
  const byPackage = [...groups.entries()].map(([pkg, e]) => ({ pkg, family: FAMILY[pkg] || (pkg === 'unmapped' ? 'Bundler glue (unmapped)' : 'Other'), raw: e.raw, gzipAlone: gzip(Buffer.from(e.code.join(''))) })).sort((a, b) => b.raw - a.raw);
  const families = new Map<string, { raw: number; code: string[] }>();
  for (const [pkg, e] of groups) {
    const f = FAMILY[pkg] || (pkg === 'unmapped' ? 'Bundler glue (unmapped)' : 'Other');
    const x = families.get(f) || { raw: 0, code: [] };
    x.raw += e.raw;
    x.code.push(...e.code);
    families.set(f, x);
  }
  const byFamily = [...families.entries()].map(([family, e]) => ({ family, raw: e.raw, gzipAlone: gzip(Buffer.from(e.code.join(''))), brotliAlone: brotli(Buffer.from(e.code.join(''))) })).sort((a, b) => b.raw - a.raw);
  return { byFamily, byPackage, sources: map.sources.length };
}

/** CSS split: xterm's stylesheet vs. the application's own, each minified on its own by Bun. */
export async function cssSplit(webSrc: string) {
  const one = async (entry: string) => {
    const r = await Bun.build({ entrypoints: [entry], minify: true, target: 'browser' });
    const text = await r.outputs[0].text();
    const b = Buffer.from(text);
    return { raw: b.length, gzip: gzip(b), brotli: brotli(b) };
  };
  const xtermCss = path.resolve('node_modules/@xterm/xterm/css/xterm.css');
  return { xterm: await one(xtermCss), app: await one(path.join(webSrc, 'styles.css')) };
}

if (import.meta.main) {
  const args = process.argv.slice(2);
  const dist = args[0];
  const mapsDir = args.includes('--maps') ? args[args.indexOf('--maps') + 1] : null;
  const outFile = args.includes('--out') ? args[args.indexOf('--out') + 1] : null;
  const files = measure(dist);
  const result: any = { dist, measuredAt: new Date().toISOString(), compression: { gzip: 'zlib level 9', brotli: 'quality 11, generic mode' }, files, totals: totals(files) };
  const version = fs.readFileSync(path.join(dist, 'index.html'), 'utf8').match(/briardr-version" content="([^"]+)"/)?.[1];
  result.version = version;
  if (mapsDir) {
    const jsRel = (fs.readdirSync(path.join(mapsDir, 'assets')) as string[]).find((f) => /^main\..*\.js$/.test(f))!;
    const js = fs.readFileSync(path.join(mapsDir, 'assets', jsRel), 'utf8').replace(/\n\/\/# sourceMappingURL=.*\s*$/, '');
    const map = JSON.parse(fs.readFileSync(path.join(mapsDir, 'assets', jsRel + '.map'), 'utf8'));
    result.composition = composition(js, map);
  }
  result.css = await cssSplit(path.resolve('web/src'));
  const text = JSON.stringify(result, null, 2);
  if (outFile) fs.writeFileSync(outFile, text + '\n');
  const kib = (n: number) => (n / 1024).toFixed(1) + ' KiB';
  for (const [k, v] of Object.entries(result.totals) as [string, any][]) console.log(k.padEnd(14), String(v.raw).padStart(9), kib(v.raw).padStart(11), 'gz', kib(v.gzip).padStart(10), 'br', kib(v.brotli).padStart(10));
  if (result.composition) for (const f of result.composition.byFamily) console.log('  ', f.family.padEnd(26), kib(f.raw).padStart(11), 'gz alone', kib(f.gzipAlone).padStart(10));
}
