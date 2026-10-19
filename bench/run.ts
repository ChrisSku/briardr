// One command to compare two builds of the web client: bundle sizes, route payloads, screen parity,
// behaviour checks, UI tests and interaction performance, ending in a combined report.
//
//   bun bench/run.ts --base <side> --cand <side> [--name <label>] [--mode quick|full]
//     [--stages sizes,routes,parity,validation,tests,perf,report] [--configs …] [--sizes …] [--scenarios …]
//     [--labels base,cand] [--rebuild] [--keep-worktrees]
//
// A side is a git ref (built in a temporary worktree from that commit's own build script), "." (the working
// tree, including uncommitted changes), or a directory that already holds a build (index.html + assets/).
// Builds go to bench/builds/, never to web/dist: the production server serves web/dist.
// Results go to bench/results/<name>/ (report.md first); full-size parity screenshots to bench/screens-full/<name>/.
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { readSamples, summarize, type Row } from './report.ts';

const ROOT = path.resolve(import.meta.dir, '..');
const BUN = process.execPath;
const argv = process.argv.slice(2);
const opt = (k: string, d?: string) => (argv.includes('--' + k) ? argv[argv.indexOf('--' + k) + 1] : d);
const flag = (k: string) => argv.includes('--' + k);

const baseArg = opt('base');
const candArg = opt('cand');
if (!baseArg || !candArg) {
  console.error('usage: bun bench/run.ts --base <git ref | . | build dir> --cand <git ref | . | build dir> [--mode quick|full] [--stages …]');
  process.exit(2);
}
const MODE = opt('mode', 'quick') as 'quick' | 'full';
const ALL = ['sizes', 'routes', 'parity', 'validation', 'tests', 'perf', 'report'];
const STAGES = opt('stages', MODE === 'quick' ? 'sizes,routes,parity,validation,perf,report' : ALL.join(','))!.split(',');
for (const s of STAGES) if (!ALL.includes(s)) throw new Error(`unknown stage ${s} (stages: ${ALL.join(', ')})`);

const log = (...a: unknown[]) => console.log(`[bench ${new Date().toISOString().slice(11, 19)}]`, ...a);
function run(cmd: string[], opts: { cwd?: string; env?: Record<string, string | undefined>; allowFail?: boolean; quiet?: boolean } = {}) {
  const r = spawnSync(cmd[0], cmd.slice(1), { cwd: opts.cwd || ROOT, env: { ...process.env, ...opts.env }, stdio: opts.quiet ? 'pipe' : 'inherit', encoding: 'utf8' });
  if (r.status !== 0 && !opts.allowFail) throw new Error(`failed (${r.status}): ${cmd.join(' ')}\n${r.stderr || ''}`);
  return r;
}
const git = (...a: string[]) => run(['git', ...a], { quiet: true }).stdout.trim();
const slug = (s: string) => s.replace(/[^A-Za-z0-9._-]+/g, '-').replace(/^-+|-+$/g, '').toLowerCase() || 'build';
const isBuild = (dir: string) => fs.existsSync(path.join(dir, 'index.html')) && fs.existsSync(path.join(dir, 'assets'));

// ---------- resolve and build both sides ----------
interface Side {
  role: 'base' | 'cand';
  arg: string;
  label: string;
  kind: 'ref' | 'worktree' | 'dir';
  sha: string | null;
  dist: string;
  maps: string | null;
}

const worktrees: string[] = [];
function build(src: string, out: string, maps: boolean) {
  fs.rmSync(out, { recursive: true, force: true });
  const env = { BRIARDR_WEB_DIST: out, ...(maps ? { BRIARDR_SOURCEMAP: 'external' } : {}) };
  run([BUN, 'scripts/build.ts'], { cwd: src, env, quiet: true });
  // Build scripts older than BRIARDR_WEB_DIST write to their own web/dist (a worktree's, never ours).
  if (!isBuild(out) && src !== ROOT && isBuild(path.join(src, 'web', 'dist'))) fs.cpSync(path.join(src, 'web', 'dist'), out, { recursive: true });
  if (!isBuild(out)) throw new Error(`no build appeared in ${out}`);
  const hasMaps = fs.readdirSync(path.join(out, 'assets')).some((f) => f.endsWith('.js.map'));
  return maps ? hasMaps : true;
}

function resolveSide(role: 'base' | 'cand', arg: string, label?: string): Side {
  const asDir = path.resolve(ROOT, arg);
  if (arg !== '.' && isBuild(asDir)) return { role, arg, label: label || slug(path.basename(asDir)), kind: 'dir', sha: null, dist: asDir, maps: null };
  const builds = path.join(ROOT, 'bench', 'builds');
  if (arg === '.') {
    const l = label || 'worktree';
    // A separate name, so a working-tree build never replaces another build in bench/builds.
    const dist = path.join(builds, l === 'worktree' ? l : `${l}-worktree`);
    const maps = path.join(builds, l === 'worktree' ? `${l}-maps` : `${l}-worktree-maps`);
    log(`${role}: building the working tree → ${path.relative(ROOT, dist)}`);
    build(ROOT, dist, false);
    const hasMaps = build(ROOT, maps, true);
    return { role, arg, label: l, kind: 'worktree', sha: git('rev-parse', '--short', 'HEAD') + (git('status', '--porcelain', '--', 'web', 'plugins', 'scripts', 'package.json', 'bun.lock') ? '+dirty' : ''), dist, maps: hasMaps ? maps : null };
  }
  const sha = git('rev-parse', '--short', `${arg}^{commit}`);
  const l = label || slug(arg);
  const dist = path.join(builds, `${l}-${sha}`);
  const maps = path.join(builds, `${l}-${sha}-maps`);
  if (!flag('rebuild') && isBuild(dist)) {
    log(`${role}: ${arg} (${sha}) already built in ${path.relative(ROOT, dist)}`);
    return { role, arg, label: l, kind: 'ref', sha, dist, maps: isBuild(maps) && fs.readdirSync(path.join(maps, 'assets')).some((f) => f.endsWith('.map')) ? maps : null };
  }
  const wt = path.join(ROOT, 'bench', 'worktrees', sha);
  if (!fs.existsSync(wt)) {
    log(`${role}: checking out ${arg} (${sha}) in ${path.relative(ROOT, wt)}`);
    git('worktree', 'add', '--detach', wt, sha);
  }
  worktrees.push(wt);
  log(`${role}: installing dependencies for ${sha}`);
  run([BUN, 'install', '--frozen-lockfile'], { cwd: wt, quiet: true });
  log(`${role}: building ${arg} (${sha}) → ${path.relative(ROOT, dist)}`);
  build(wt, dist, false);
  const hasMaps = build(wt, maps, true);
  return { role, arg, label: l, kind: 'ref', sha, dist, maps: hasMaps ? maps : null };
}

const [lb, lc] = (opt('labels') || '').split(',');
const base = resolveSide('base', baseArg, lb || undefined);
let cand = resolveSide('cand', candArg, lc || undefined);
if (cand.label === base.label) cand = { ...cand, label: cand.label + '-cand' };
const NAME = slug(opt('name') || `${base.label}-vs-${cand.label}`);
const OUT = path.join(ROOT, 'bench', 'results', NAME);
fs.mkdirSync(OUT, { recursive: true });
const meta: any = { name: NAME, mode: MODE, stages: STAGES, startedAt: new Date().toISOString(), base, cand: { ...cand }, results: {} };
const saveMeta = () => fs.writeFileSync(path.join(OUT, 'meta.json'), JSON.stringify(meta, null, 2) + '\n');
saveMeta();
log(`comparing ${base.label} (${base.sha ?? base.dist}) → ${cand.label} (${cand.sha ?? cand.dist}); results in ${path.relative(ROOT, OUT)}`);

const sections: string[] = [];
const kib = (n: number) => `${(n / 1024).toFixed(1)} KiB`;
const pct = (a: number, b: number) => (a ? `${b >= a ? '+' : '−'}${Math.abs(((b - a) / a) * 100).toFixed(1)}%` : '—');

// ---------- stages ----------
try {
  if (STAGES.includes('sizes')) {
    log('stage: sizes');
    for (const s of [base, cand]) run([BUN, 'bench/sizes.ts', s.dist, ...(s.maps ? ['--maps', s.maps] : []), '--out', path.join(OUT, `sizes-${s.label}.json`)], { quiet: true });
    const a = JSON.parse(fs.readFileSync(path.join(OUT, `sizes-${base.label}.json`), 'utf8'));
    const b = JSON.parse(fs.readFileSync(path.join(OUT, `sizes-${cand.label}.json`), 'utf8'));
    const rows: [string, string][] = [['App JavaScript', 'appJs'], ['Service worker', 'serviceWorker'], ['CSS', 'css'], ['Other assets', 'otherAssets'], ['Total distribution', 'total']];
    const lines = rows.map(([l, k]) => `| ${l} | ${kib(a.totals[k].raw)} | ${kib(b.totals[k].raw)} | ${pct(a.totals[k].raw, b.totals[k].raw)} | ${pct(a.totals[k].gzip, b.totals[k].gzip)} | ${pct(a.totals[k].brotli, b.totals[k].brotli)} |`);
    let comp = '';
    if (a.composition && b.composition) {
      const fam = (x: any) => Object.fromEntries(x.composition.byFamily.map((f: any) => [f.family, f.raw]));
      const fa = fam(a);
      const fb = fam(b);
      comp = '\n\n| Part of the app bundle | ' + base.label + ' | ' + cand.label + ' |\n|---|---:|---:|\n' + [...new Set([...Object.keys(fa), ...Object.keys(fb)])].map((f) => `| ${f} | ${fa[f] ? kib(fa[f]) : '—'} | ${fb[f] ? kib(fb[f]) : '—'} |`).join('\n');
    }
    sections.push(`## Bundle size\n\n| Files | ${base.label} | ${cand.label} | Δ raw | Δ gzip | Δ Brotli |\n|---|---:|---:|---:|---:|---:|\n${lines.join('\n')}${comp}\n\nFiles: \`sizes-${base.label}.json\`, \`sizes-${cand.label}.json\`.`);
    meta.results.sizes = { base: a.totals, cand: b.totals };
    saveMeta();
  }

  if (STAGES.includes('routes')) {
    log('stage: routes (needs the briardr-dev Herdr session for the real-server encoding check)');
    run([BUN, 'bench/routes.ts', `${base.label}=${base.dist}`, `${cand.label}=${cand.dist}`, '--out', path.join(OUT, 'routes.json')], { env: { BRIARDR_WEB_DIST: cand.dist }, quiet: true });
    const r = JSON.parse(fs.readFileSync(path.join(OUT, 'routes.json'), 'utf8'));
    const fresh = (l: string, e: string) => r.builds[l][e][0];
    const load = (l: string, e: string) => `${kib(fresh(l, e).raw)}${fresh(l, e).sent != null ? ` (${kib(fresh(l, e).sent)} sent)` : ''}`;
    const later = (l: string, e: string) => r.builds[l][e].slice(1).filter((s: any) => s.files.length).map((s: any) => `${s.step}: ${s.files.map((f: any) => f.file).join(', ')}`);
    sections.push(`## Route payloads\n\n| Fresh Agents load | ${base.label} | ${cand.label} |\n|---|---:|---:|\n| Chromium | ${load(base.label, 'chromium')} | ${load(cand.label, 'chromium')} |\n| WebKit | ${load(base.label, 'webkit')} | ${load(cand.label, 'webkit')} |\n\nFiles' sizes, and in parentheses the bytes sent (compressed, if the server compressed them; WebKit accepts Brotli only over HTTPS, so it gets gzip from the fixture server). Later screens fetch: ${[...later(base.label, 'chromium'), ...later(cand.label, 'chromium')].join('; ') || 'nothing more (single bundle)'}.\nServer encodings: ${r.realServer.checks.map((c: any) => `\`${c.path}\` ${c.contentEncoding ?? 'uncompressed'}`).join(', ')}. File: \`routes.json\`.`);
  }

  if (STAGES.includes('parity')) {
    log('stage: parity (20 screens × iPhone/iPad/Mac × light/dark, WebKit)');
    const screens = path.join(ROOT, 'bench', 'screens-full', NAME);
    run([BUN, 'bench/parity.ts', `${base.label}=${base.dist}`, `${cand.label}=${cand.dist}`, '--out', path.join(OUT, 'parity'), '--screens', screens], { quiet: true, allowFail: true });
    const p = JSON.parse(fs.readFileSync(path.join(OUT, 'parity', 'parity.json'), 'utf8'));
    const diffs = p.report.filter((x: any) => !(x.differing === 0 && x.textSame && x.ariaSame));
    const lines = diffs.slice(0, 40).map((x: any) => `| ${x.profile}-${x.scheme} | ${x.screen} | ${x.differing ?? '—'} | ${x.textSame ? 'same' : 'differs'} | ${x.ariaSame ? 'same' : 'differs'} | ${x.errorA || x.errorB || ''} |`);
    sections.push(`## Screen parity\n\n**${p.summary.identical} of ${p.summary.comparisons}** screen comparisons identical (pixels, visible text, accessibility tree).${diffs.length ? `\n\n| Profile | Screen | Differing pixels | Text | Accessibility tree | Error |\n|---|---|---:|---|---|---|\n${lines.join('\n')}` : ''}\n\nDetails (text and accessibility diffs): \`parity/parity.json\`. Screenshots and red-on-grey diff images: \`${path.relative(ROOT, screens)}/\`.`);
    meta.results.parity = p.summary;
    saveMeta();
  }

  const testRun = (label: string, file: string, env: Record<string, string>) => {
    const r = run([BUN, 'test', '--timeout', '120000', file], { env, quiet: true, allowFail: true });
    const out = (r.stdout + r.stderr).replace(/\x1b\[[0-9;]*m/g, '');
    fs.writeFileSync(path.join(OUT, `${label}.txt`), out);
    const pass = Number(out.match(/^\s*(\d+) pass/m)?.[1] ?? 0);
    const fail = Number(out.match(/^\s*(\d+) fail/m)?.[1] ?? 0);
    const failed = [...out.matchAll(/^\(fail\) (.*)$|^✗ (.*)$/gm)].map((m) => (m[1] || m[2]).trim());
    return { pass, fail, failed, ok: r.status === 0 };
  };

  if (STAGES.includes('validation')) {
    log('stage: validation (test-ui/validation.test.ts against each build and the base → cand update)');
    const res: Record<string, any> = {};
    res[base.label] = testRun(`validation-${base.label}`, './test-ui/validation.test.ts', { BRIARDR_WEB_DIST: base.dist });
    res[cand.label] = testRun(`validation-${cand.label}`, './test-ui/validation.test.ts', { BRIARDR_WEB_DIST: cand.dist });
    res[`${base.label} → ${cand.label}`] = testRun(`validation-update`, './test-ui/validation.test.ts', { BRIARDR_WEB_DIST: base.dist, BRIARDR_UPDATE_DIST: cand.dist });
    sections.push(`## Validation tests\n\n| Served | Pass | Fail | Failed tests |\n|---|---:|---:|---|\n${Object.entries(res).map(([k, v]) => `| ${k} | ${v.pass} | ${v.fail} | ${v.failed.join('; ')} |`).join('\n')}\n\nLogs: \`validation-*.txt\`.`);
    meta.results.validation = res;
    saveMeta();
  }

  if (STAGES.includes('tests')) {
    log('stage: tests (all of test-ui against each build; bun test ./test once)');
    const res: Record<string, any> = {};
    for (const s of [base, cand]) res[`test-ui, ${s.label}`] = testRun(`test-ui-${s.label}`, './test-ui', { BRIARDR_WEB_DIST: s.dist });
    const unit = run([BUN, 'test', '--timeout', '300000', './test'], { quiet: true, allowFail: true });
    const uo = (unit.stdout + unit.stderr).replace(/\x1b\[[0-9;]*m/g, '');
    fs.writeFileSync(path.join(OUT, 'test.txt'), uo);
    res['test (server, unit)'] = { pass: Number(uo.match(/^\s*(\d+) pass/m)?.[1] ?? 0), fail: Number(uo.match(/^\s*(\d+) fail/m)?.[1] ?? 0), failed: [] };
    sections.push(`## Test suites\n\n| Suite | Pass | Fail | Failed tests |\n|---|---:|---:|---|\n${Object.entries(res).map(([k, v]) => `| ${k} | ${v.pass} | ${v.fail} | ${v.failed.join('; ')} |`).join('\n')}\n\nThe tests come from the current checkout; logs: \`test-ui-*.txt\`, \`test.txt\`.`);
    meta.results.tests = res;
    saveMeta();
  }

  if (STAGES.includes('perf')) {
    const quick = ['--configs', 'chromium-desktop,webkit-iphone', '--sizes', '100', '--rounds', '2', '--per-round', '4', '--warmup', '1', '--launches', '8'];
    const args = [BUN, 'bench/perf.ts', '--impl', `${base.label}=${base.dist}`, '--impl', `${cand.label}=${cand.dist}`, '--out', path.join(OUT, 'perf'), ...(MODE === 'quick' ? quick : [])];
    for (const k of ['configs', 'sizes', 'scenarios', 'rounds', 'per-round', 'warmup', 'launches']) {
      const v = opt(k);
      if (v === undefined) continue;
      const i = args.indexOf('--' + k);
      if (i >= 0) args[i + 1] = v;
      else args.push('--' + k, v);
    }
    log(`stage: perf (${MODE}; keep the machine otherwise idle) — ${args.slice(2).join(' ')}`);
    fs.rmSync(path.join(OUT, 'perf'), { recursive: true, force: true });
    run(args);
  }

  if (STAGES.includes('report') && fs.existsSync(path.join(OUT, 'perf', 'samples.jsonl'))) {
    log('stage: report');
    run([BUN, 'bench/report.ts', path.join(OUT, 'perf'), '--base', base.label, '--cand', cand.label], { quiet: true });
    const samples = readSamples(path.join(OUT, 'perf'));
    const rows = summarize(samples, base.label, cand.label);
    const PRIMARY = new Set(['dom', 'dom:mounted', 'dom:live', 'populated', 'populated-frame', 'lcp', 'update-median', 'last-frame-shown', 'frames-over-50ms', 'cdp:ScriptDuration', 'js-heap-used']);
    const key = rows.filter((r) => PRIMARY.has(r.metric) && r.base && r.cand);
    const sig = key.filter((r) => !r.noise && r.pct !== null).sort((x, y) => Math.abs(y.pct!) - Math.abs(x.pct!));
    const fmt = (r: Row, v: number) => (r.unit === 'bytes' ? `${(v / 1048576).toFixed(2)} MB` : r.unit === 'count' ? String(v) : `${v.toFixed(v >= 100 ? 0 : 1)} ms`);
    const line = (r: Row) => `| ${r.scenario} | ${r.metric} | ${r.config} | ${r.size} | ${fmt(r, r.base!.median)} | ${fmt(r, r.cand!.median)} | ${r.pct! >= 0 ? '+' : '−'}${Math.abs(r.pct!).toFixed(0)}% |`;
    const checks = fs.existsSync(path.join(OUT, 'perf', 'checks.jsonl'))
      ? fs.readFileSync(path.join(OUT, 'perf', 'checks.jsonl'), 'utf8').split('\n').filter(Boolean).map((l) => JSON.parse(l))
      : [];
    const failed = new Map<string, number>();
    for (const c of checks.filter((c: any) => !c.ok)) failed.set(`${c.impl}: ${c.scenario} ${c.check.replace(/-after-\d+$/, '')}`, (failed.get(`${c.impl}: ${c.scenario} ${c.check.replace(/-after-\d+$/, '')}`) || 0) + 1);
    const n = rows[0]?.base?.n ?? 0;
    sections.push(
      `## Performance (${MODE}, ${n} samples per build and metric)\n\n` +
        `Of ${key.length} primary comparisons: ${cand.label} lower in ${key.filter((r) => !r.noise && r.diff! < 0).length}, higher in ${key.filter((r) => !r.noise && r.diff! > 0).length}, within noise in ${key.filter((r) => r.noise).length} ` +
        `(noise: the bootstrap 95% interval of the difference contains 0).\n\n` +
        (sig.length ? `Largest significant differences:\n\n| Scenario | Metric | Config | Streams | ${base.label} | ${cand.label} | Δ |\n|---|---|---|---:|---:|---:|---:|\n${sig.slice(0, 30).map(line).join('\n')}\n\n` : 'No significant differences.\n\n') +
        (checks.length
          ? `Behaviour checks: ${checks.length - [...failed.values()].reduce((a, b) => a + b, 0)} of ${checks.length} passed.${failed.size ? ' Failed: ' + [...failed.entries()].map(([k, v]) => `${k} (${v}×)`).join('; ') + '.' : ''}\n\n`
          : 'The scenarios run have no behaviour checks.\n\n') +
        `Every metric with p95 and intervals: \`perf/summary.md\`; raw samples: \`perf/samples.jsonl\`.`,
    );
  }
} finally {
  meta.finishedAt = new Date().toISOString();
  saveMeta();
  const header = `# ${base.label} → ${cand.label}\n\n- Base: \`${base.arg}\` (${base.kind}${base.sha ? `, ${base.sha}` : ''}) → \`${path.relative(ROOT, base.dist)}\`\n- Candidate: \`${cand.arg}\` (${cand.kind}${cand.sha ? `, ${cand.sha}` : ''}) → \`${path.relative(ROOT, cand.dist)}\`\n- Mode: ${MODE}; stages: ${STAGES.join(', ')}; ${meta.startedAt} → ${meta.finishedAt}`;
  fs.writeFileSync(path.join(OUT, 'report.md'), [header, ...sections].join('\n\n') + '\n');
  if (!flag('keep-worktrees'))
    for (const wt of worktrees) {
      run(['git', 'worktree', 'remove', '--force', wt], { allowFail: true, quiet: true });
    }
  log(`report: ${path.relative(ROOT, path.join(OUT, 'report.md'))}`);
}
