// Summarizes benchmark samples into Markdown tables.
//   bun bench/report.ts <run dir> [--base preact] [--cand svelte] [--out summary.md]
// For every scenario metric and configuration: sample count, median and p95 per implementation, the
// difference of medians and its percentage ((cand − base) / base × 100), and a bootstrap 95% interval
// for the difference of medians. A difference whose interval contains 0 is marked as within noise.
import fs from 'node:fs';
import path from 'node:path';

export interface Sample {
  impl: string;
  config: string;
  engine: string;
  size: number;
  round: number;
  iter: number;
  scenario: string;
  metric: string;
  value: number | null;
  unit: string;
  cycle?: number;
}

export function readSamples(dir: string): Sample[] {
  return fs
    .readFileSync(path.join(dir, 'samples.jsonl'), 'utf8')
    .split('\n')
    .filter(Boolean)
    .map((l) => JSON.parse(l));
}

export function quantile(sorted: number[], q: number): number {
  // Nearest-rank on the sorted samples (p95 of 20 samples is the 19th value).
  return sorted[Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1))];
}
export const median = (xs: number[]) => {
  const s = [...xs].sort((a, b) => a - b);
  const n = s.length;
  return n % 2 ? s[(n - 1) / 2] : (s[n / 2 - 1] + s[n / 2]) / 2;
};

/** Seeded bootstrap of the difference of medians (b − a); returns the 2.5% and 97.5% quantiles. */
export function bootstrapDiff(a: number[], b: number[], n = 2000, seed = 1): [number, number] {
  let s = seed >>> 0;
  const rand = () => {
    s = (s + 0x6d2b79f5) >>> 0;
    let t = s;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
  const pick = (xs: number[]) => Array.from(xs, () => xs[Math.floor(rand() * xs.length)]);
  const diffs: number[] = [];
  for (let i = 0; i < n; i++) diffs.push(median(pick(b)) - median(pick(a)));
  diffs.sort((x, y) => x - y);
  return [quantile(diffs, 0.025), quantile(diffs, 0.975)];
}

export interface Row {
  scenario: string;
  metric: string;
  config: string;
  size: number;
  unit: string;
  base: { n: number; median: number; p95: number } | null;
  cand: { n: number; median: number; p95: number } | null;
  diff: number | null;
  pct: number | null;
  ci: [number, number] | null;
  noise: boolean | null;
}

export function summarize(samples: Sample[], base: string, cand: string): Row[] {
  const groups = new Map<string, Sample[]>();
  for (const s of samples) {
    if (s.round < 0 || s.value === null || s.scenario === 'memory') continue;
    const k = [s.scenario, s.metric, s.config, s.size].join('|');
    (groups.get(k) || groups.set(k, []).get(k)!).push(s);
  }
  const rows: Row[] = [];
  for (const [k, list] of groups) {
    const [scenario, metric, config, size] = k.split('|');
    const stats = (impl: string) => {
      const v = list.filter((s) => s.impl === impl).map((s) => s.value!) as number[];
      if (!v.length) return null;
      const sorted = [...v].sort((a, b) => a - b);
      return { n: v.length, median: median(v), p95: quantile(sorted, 0.95), values: v };
    };
    const a = stats(base);
    const b = stats(cand);
    const diff = a && b ? b.median - a.median : null;
    const pct = a && b && a.median !== 0 ? (diff! / a.median) * 100 : null;
    const ci = a && b ? bootstrapDiff(a.values, b.values) : null;
    rows.push({
      scenario,
      metric,
      config,
      size: Number(size),
      unit: list[0].unit,
      base: a && { n: a.n, median: a.median, p95: a.p95 },
      cand: b && { n: b.n, median: b.median, p95: b.p95 },
      diff,
      pct,
      ci,
      noise: ci ? ci[0] <= 0 && ci[1] >= 0 : null,
    });
  }
  return rows.sort((x, y) => x.scenario.localeCompare(y.scenario) || x.metric.localeCompare(y.metric) || x.config.localeCompare(y.config) || x.size - y.size);
}

/** Memory trend per run: value after 30 open/close cycles minus the value before them. */
export function memoryTrend(samples: Sample[]) {
  const out: Record<string, { impl: string; config: string; metric: string; unit: string; first: number[]; last: number[]; growth: number[] }> = {};
  const runs = new Map<string, Sample[]>();
  for (const s of samples.filter((x) => x.scenario === 'memory' && x.value !== null)) {
    const k = [s.impl, s.config, s.size, s.round, s.metric].join('|');
    (runs.get(k) || runs.set(k, []).get(k)!).push(s);
  }
  for (const [k, list] of runs) {
    const [impl, config, , , metric] = k.split('|');
    const first = list.find((s) => s.cycle === 0)?.value;
    const last = list.find((s) => s.cycle === 30)?.value;
    if (first == null || last == null) continue;
    const key = [impl, config, metric].join('|');
    const e = (out[key] ||= { impl, config, metric, unit: list[0].unit, first: [], last: [], growth: [] });
    e.first.push(first);
    e.last.push(last);
    e.growth.push(last - first);
  }
  return Object.values(out);
}

const fmt = (v: number | null | undefined, unit: string) => {
  if (v === null || v === undefined || Number.isNaN(v)) return '—';
  if (unit === 'bytes') return `${(v / 1024 / 1024).toFixed(2)} MB`;
  if (unit === 'count') return Number.isInteger(v) ? String(v) : v.toFixed(1);
  return Math.abs(v) >= 100 ? v.toFixed(0) : Math.abs(v) >= 10 ? v.toFixed(1) : v.toFixed(2);
};
const signed = (v: number | null, unit: string) => (v === null ? '—' : (v > 0 ? '+' : v < 0 ? '−' : '±') + fmt(Math.abs(v), unit));
const pctText = (p: number | null) => (p === null ? '—' : `${p > 0 ? '+' : p < 0 ? '−' : '±'}${Math.abs(p).toFixed(1)}%`);

export function table(rows: Row[], base: string, cand: string): string {
  const head = `| Scenario | Metric | Config | Streams | n | ${base} median | ${base} p95 | ${cand} median | ${cand} p95 | Δ median | Δ % | 95% CI of Δ |\n|---|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---|`;
  const lines = rows.map((r) => {
    const n = `${r.base?.n ?? 0}/${r.cand?.n ?? 0}`;
    const ci = r.ci ? `${signed(r.ci[0], r.unit)} … ${signed(r.ci[1], r.unit)}${r.noise ? ' (noise)' : ''}` : '—';
    return `| ${r.scenario} | ${r.metric} | ${r.config} | ${r.size} | ${n} | ${fmt(r.base?.median, r.unit)} | ${fmt(r.base?.p95, r.unit)} | ${fmt(r.cand?.median, r.unit)} | ${fmt(r.cand?.p95, r.unit)} | ${signed(r.diff, r.unit)} | ${r.noise ? '' : pctText(r.pct)}${r.noise ? '(' + pctText(r.pct) + ')' : ''} | ${ci} |`;
  });
  return [head, ...lines].join('\n');
}

if (import.meta.main) {
  const argv = process.argv.slice(2);
  const dir = argv[0];
  const opt = (k: string, d: string) => (argv.includes('--' + k) ? argv[argv.indexOf('--' + k) + 1] : d);
  const base = opt('base', 'preact');
  const cand = opt('cand', 'svelte');
  const samples = readSamples(dir);
  const rows = summarize(samples, base, cand);
  const mem = memoryTrend(samples);
  const memTable = [
    `| Impl | Config | Metric | Runs | Before (median) | After 30 cycles (median) | Growth (median) |`,
    `|---|---|---|---:|---:|---:|---:|`,
    ...mem.sort((a, b) => a.metric.localeCompare(b.metric) || a.config.localeCompare(b.config) || a.impl.localeCompare(b.impl)).map((m) => `| ${m.impl} | ${m.config} | ${m.metric} | ${m.first.length} | ${fmt(median(m.first), m.unit)} | ${fmt(median(m.last), m.unit)} | ${signed(median(m.growth), m.unit)} |`),
  ].join('\n');
  const md = `# Benchmark summary: ${path.basename(dir)}\n\n${table(rows, base, cand)}\n\n## Memory after repeated navigation\n\n${memTable}\n`;
  const out = opt('out', path.join(dir, 'summary.md'));
  fs.writeFileSync(out, md);
  fs.writeFileSync(out.replace(/\.md$/, '.json'), JSON.stringify({ rows, memory: mem }, null, 1));
  console.log(`wrote ${out} (${rows.length} rows)`);
}
