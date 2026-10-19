// Briardr's version and releases. The version is set in server/Cargo.toml (the server reads it when it's
// compiled); package.json, server/Cargo.lock, CHANGELOG.md and the install instructions, which clone the
// release tag, must agree with it. CI runs `check`; the release workflow runs `check` and `notes` for a tag.
//   bun scripts/release.ts check [vX.Y.Z]   everything agrees, and with the tag when one is given
//   bun scripts/release.ts notes vX.Y.Z     that version's CHANGELOG.md section, for the GitHub release
//   bun scripts/release.ts bump X.Y.Z       set a new version everywhere and start its CHANGELOG.md section
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dir, '..');
const file = (rel: string) => path.join(root, rel);
const read = (rel: string) => fs.readFileSync(file(rel), 'utf8');

/** Files whose install instructions clone the current release (docs/development.md clones main on purpose). */
const INSTALL_DOCS = ['README.md', 'site/index.html', 'docs/macos-setup.md', 'docs/linux-deployment.md'];
const CLONE = /git clone (?:--branch (\S+) )?https:\/\/github\.com\/ChrisSku\/briardr\.git/g;
const SEMVER = /^\d+\.\d+\.\d+$/;
const CARGO_VERSION = /^(\[package\][^[]*?\nversion = )"([^"]*)"/m;
const LOCK_VERSION = /(name = "briardr-server"\nversion = )"([^"]*)"/;

function fail(message: string): never {
  console.error(`release: ${message}`);
  process.exit(1);
}

function version(): string {
  const v = read('server/Cargo.toml').match(CARGO_VERSION)?.[2];
  if (!v || !SEMVER.test(v)) fail(`server/Cargo.toml has no X.Y.Z version (found ${v ?? 'none'})`);
  return v;
}

/** The CHANGELOG.md section for `v`, without its heading; null when there's none. */
function notes(v: string): string | null {
  const text = read('CHANGELOG.md');
  const start = text.search(new RegExp(`^## ${v.replace(/\./g, '\\.')}\\b.*$`, 'm'));
  if (start < 0) return null;
  const body = text.slice(text.indexOf('\n', start) + 1);
  const end = body.search(/^## /m);
  return (end < 0 ? body : body.slice(0, end)).trim();
}

/** GitHub shows every line break in release notes, so lines wrapped in CHANGELOG.md are joined again. */
function unwrap(md: string): string {
  const out: string[] = [];
  let code = false;
  for (const line of md.split('\n')) {
    const prev = out.at(-1);
    if (!code && line && prev && !/^\s*([-*+] |\d+\. |#|\||>|```)/.test(line)) out[out.length - 1] = `${prev} ${line.trim()}`;
    else out.push(line);
    if (line.trimStart().startsWith('```')) code = !code;
  }
  return out.join('\n');
}

function check(tag?: string) {
  const v = version();
  const problems: string[] = [];
  const pkg = JSON.parse(read('package.json')).version;
  if (pkg !== v) problems.push(`package.json says ${pkg}`);
  const lock = read('server/Cargo.lock').match(LOCK_VERSION)?.[2];
  if (lock !== v) problems.push(`server/Cargo.lock says ${lock} (run cargo build)`);
  if (!/\w/.test(notes(v) ?? '')) problems.push(`CHANGELOG.md has no notes under "## ${v}"`);
  for (const rel of INSTALL_DOCS) {
    const clones = [...read(rel).matchAll(CLONE)];
    if (!clones.length) problems.push(`${rel} has no git clone of the release`);
    for (const [line, branch] of clones) if (branch !== `v${v}`) problems.push(`${rel}: "${line}" should clone --branch v${v}`);
  }
  if (tag !== undefined && tag !== `v${v}`) problems.push(`the tag ${tag} doesn't match the version (v${v})`);
  if (problems.length) fail(`version ${v} isn't consistent:\n  ${problems.join('\n  ')}`);
  console.log(`version ${v}: package.json, Cargo.lock, CHANGELOG.md and the install instructions agree${tag ? `, and so does ${tag}` : ''}`);
}

function bump(next: string) {
  if (!SEMVER.test(next)) fail(`"${next}" isn't an X.Y.Z version`);
  const write = (rel: string, change: (text: string) => string) => fs.writeFileSync(file(rel), change(read(rel)));
  write('server/Cargo.toml', (t) => t.replace(CARGO_VERSION, `$1"${next}"`));
  write('server/Cargo.lock', (t) => t.replace(LOCK_VERSION, `$1"${next}"`));
  write('package.json', (t) => t.replace(/("version": )"[^"]*"/, `$1"${next}"`));
  for (const rel of INSTALL_DOCS) write(rel, (t) => t.replace(CLONE, (line) => line.replace(/^git clone (--branch \S+ )?/, `git clone --branch v${next} `)));
  if (notes(next) === null) write('CHANGELOG.md', (t) => t.replace(/^## /m, `## ${next}\n\n- \n\n## `));
  console.log(`version set to ${next}: write its notes in CHANGELOG.md, then commit, and tag v${next} once that's on main`);
}

const [command, arg] = process.argv.slice(2);
if (command === 'check') check(arg);
else if (command === 'notes' && arg) console.log(unwrap(notes(arg.replace(/^v/, '')) ?? fail(`CHANGELOG.md has no notes for ${arg}`)));
else if (command === 'bump' && arg) bump(arg);
else fail('usage: bun scripts/release.ts check [vX.Y.Z] | notes vX.Y.Z | bump X.Y.Z');
