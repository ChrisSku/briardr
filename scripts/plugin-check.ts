// Type-checks a plugin against @briardr/sdk and Svelte: `bun scripts/plugin-check.ts <plugin folder>`.
// Runs svelte-check with the plugin's tsconfig.json (made by `briardr plugin create`) and reports only
// problems in the plugin's own files: Briardr's sources are checked by Briardr.
import path from 'node:path';

const root = path.resolve(import.meta.dir, '..');
const dir = path.resolve(process.argv[2] || '.');
const run = Bun.spawnSync(
  [process.execPath, '--bun', path.join(root, 'node_modules', '.bin', 'svelte-check'), '--workspace', dir, '--tsconfig', path.join(dir, 'tsconfig.json'), '--output', 'machine-verbose'],
  { cwd: dir, stdout: 'pipe', stderr: 'pipe' },
);

interface Diagnostic {
  type: 'ERROR' | 'WARNING';
  filename: string;
  start?: { line: number; character: number };
  message: string;
  code?: string | number;
}

const mine: Diagnostic[] = [];
for (const line of run.stdout.toString().split('\n')) {
  const json = line.slice(line.indexOf(' ') + 1);
  if (!json.startsWith('{')) continue;
  const d = JSON.parse(json) as Diagnostic;
  const file = path.resolve(dir, d.filename);
  if (file.startsWith(dir + path.sep) && !file.includes(`${path.sep}node_modules${path.sep}`)) mine.push({ ...d, filename: path.relative(dir, file) });
}
for (const d of mine) {
  const at = d.start ? `:${d.start.line + 1}:${d.start.character + 1}` : '';
  console.log(`${d.filename}${at} ${d.type === 'ERROR' ? 'error' : 'warning'}: ${d.message.split('\n').join(' ')}`);
}
const errors = mine.filter((d) => d.type === 'ERROR').length;
if (run.exitCode !== 0 && !mine.length && !run.stdout.toString().includes('COMPLETED')) {
  console.error(run.stderr.toString() || 'svelte-check didn’t run.');
  process.exit(2);
}
console.log(errors || mine.length ? `${errors} errors, ${mine.length - errors} warnings` : 'No problems.');
process.exit(errors ? 1 : 0);
