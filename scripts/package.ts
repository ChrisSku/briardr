// Packages a Briardr release for one platform, so it runs without a checkout: the server built for it as
// bin/briardr, the web client in web/dist next to it (where the server looks), the macOS dictation helper's
// source, Briardr's license, and the notices of the third-party code built into both. It writes
// dist/briardr-<version>-<os>-<arch>/ and dist/briardr-<os>-<arch>.tar.gz, the release asset. The release
// workflow runs it for each platform; without --target it packages for this computer.
//   bun scripts/package.ts [--target <Rust target, e.g. x86_64-apple-darwin>]
// The server builds into server/target/<target>, so the checkout's own server/target/release/briardr (what
// a running Briardr may be) and web/dist stay as they are.
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dir, '..');

function fail(message: string): never {
  console.error(`package: ${message}`);
  process.exit(1);
}

function run(cmd: string[], env: Record<string, string> = {}): string {
  const p = Bun.spawnSync(cmd, { cwd: root, env: { ...process.env, ...env }, stdout: 'pipe', stderr: 'inherit' });
  if (p.exitCode !== 0) fail(`${cmd.join(' ')} failed`);
  return p.stdout.toString();
}

/** What the release calls a Rust target: macos-aarch64, linux-x86_64, … (Herdr's names). */
function platform(target: string): string {
  const os = target.endsWith('-apple-darwin') ? 'macos' : target.includes('-linux-') ? 'linux' : fail(`Briardr runs on macOS and Linux, not ${target}`);
  const arch = target.split('-')[0];
  if (arch !== 'aarch64' && arch !== 'x86_64') fail(`Briardr is released for aarch64 and x86_64, not ${arch}`);
  return `${os}-${arch}`;
}

type Package = { name: string; version: string; license: string; dir: string };

/** The crates built into the server for `target`: what it depends on to run, not to build or test. */
function crates(target: string): Package[] {
  const meta = JSON.parse(run(['cargo', 'metadata', '--format-version', '1', '--locked', '--manifest-path', 'server/Cargo.toml', '--filter-platform', target]));
  type Node = { id: string; deps: { pkg: string; dep_kinds: { kind: string | null }[] }[] };
  const nodes = new Map<string, Node>(meta.resolve.nodes.map((n: Node) => [n.id, n]));
  const seen = new Set<string>();
  const todo: string[] = [meta.resolve.root];
  while (todo.length)
    for (const dep of nodes.get(todo.pop()!)!.deps)
      if (dep.dep_kinds.some((k) => k.kind === null) && !seen.has(dep.pkg)) seen.add(dep.pkg), todo.push(dep.pkg);
  return meta.packages
    .filter((p: { id: string }) => seen.has(p.id))
    .map((p: { name: string; version: string; license: string | null; manifest_path: string }) => ({ name: p.name, version: p.version, license: p.license ?? 'unknown', dir: path.dirname(p.manifest_path) }));
}

/** The npm packages bundled into the client (the list scripts/build.ts --packages writes). */
function npmPackages(file: string): Package[] {
  return JSON.parse(fs.readFileSync(file, 'utf8')).map(({ name, version, dir }: Package) => {
    const license = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8')).license;
    return { name, version, dir, license: typeof license === 'string' ? license : 'unknown' };
  });
}

/** THIRD-PARTY-NOTICES.txt: each license file's text, once, after the packages that come with it. */
function notices(version: string, groups: [string, Package[]][]): string {
  const LICENSE_FILE = /^(licen[cs]e|copying|copyright|notice|unlicense)/i;
  const byText = new Map<string, { text: string; labels: string[] }>();
  const without: string[] = [];
  for (const [kind, packages] of groups)
    for (const p of packages) {
      const files = fs
        .readdirSync(p.dir)
        .filter((f) => LICENSE_FILE.test(f) && fs.statSync(path.join(p.dir, f)).isFile())
        .sort();
      const label = `${p.name} ${p.version} (${kind}, ${p.license})`;
      if (!files.length) {
        without.push(label);
        continue;
      }
      // The same text wrapped or indented differently is the same license.
      for (const f of files) {
        const text = fs.readFileSync(path.join(p.dir, f), 'utf8').trim();
        const key = text.replace(/\s+/g, ' ');
        const seen = byText.get(key);
        byText.set(key, { text: seen?.text ?? text, labels: [...(seen?.labels ?? []), label] });
      }
    }
  const rule = '='.repeat(80);
  const parts = [...byText.values()].sort((a, b) => a.labels[0].localeCompare(b.labels[0])).map(({ text, labels }) => `${rule}\n${labels.join('\n')}\n${'-'.repeat(80)}\n\n${text}\n`);
  if (without.length) parts.push(`${rule}\nThese packages come without a license file; their license is the one named:\n${without.join('\n')}\n`);
  return `Briardr ${version} is made with the third-party software below: the Rust crates built into bin/briardr
and the npm packages bundled into web/dist. Each part names packages, then a license text they come with.

${parts.join('\n')}`;
}

const at = process.argv.indexOf('--target');
const target = at > 0 ? (process.argv[at + 1] ?? fail('--target needs a Rust target')) : (run(['rustc', '-vV']).match(/^host: (\S+)$/m)?.[1] ?? fail('rustc -vV names no host'));
const version: string = JSON.parse(fs.readFileSync(path.join(root, 'package.json'), 'utf8')).version;
const name = `briardr-${platform(target)}`;
const out = path.join(root, 'dist');
const stage = path.join(out, `briardr-${version}-${platform(target)}`);
fs.rmSync(stage, { recursive: true, force: true });
fs.mkdirSync(path.join(stage, 'bin'), { recursive: true });

const packagesFile = path.join(out, `${name}-npm-packages.json`);
run([process.execPath, 'scripts/build.ts', '--packages', packagesFile], { BRIARDR_WEB_DIST: path.join(stage, 'web', 'dist') });
run(['cargo', 'build', '--release', '--locked', '--manifest-path', 'server/Cargo.toml', '--target', target]);
fs.copyFileSync(path.join(root, 'server', 'target', target, 'release', 'briardr'), path.join(stage, 'bin', 'briardr'));
fs.chmodSync(path.join(stage, 'bin', 'briardr'), 0o755);
if (target.endsWith('-apple-darwin')) fs.cpSync(path.join(root, 'native'), path.join(stage, 'native'), { recursive: true });
fs.copyFileSync(path.join(root, 'LICENSE'), path.join(stage, 'LICENSE'));
fs.writeFileSync(
  path.join(stage, 'THIRD-PARTY-NOTICES.txt'),
  notices(version, [
    ['Rust crate', crates(target)],
    ['npm package', npmPackages(packagesFile)],
  ]),
);
fs.rmSync(packagesFile);

const archive = path.join(out, `${name}.tar.gz`);
fs.rmSync(archive, { force: true });
// COPYFILE_DISABLE: macOS's tar would add ._ files with each file's extended attributes.
run(['tar', '-czf', archive, '-C', out, path.basename(stage)], { COPYFILE_DISABLE: '1' });
console.log(`packaged Briardr ${version} for ${target}: ${path.relative(root, archive)} (${(fs.statSync(archive).size / 1e6).toFixed(1)} MB)`);
