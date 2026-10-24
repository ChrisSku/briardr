// Briardr's version and releases. The version is set in package.json, by `changeset version` when the
// release workflow versions the changesets a merge brought to main; server/Cargo.toml (the server reads it
// when it's compiled), server/Cargo.lock, CHANGELOG.md and the install instructions, which clone the
// release tag, must agree with it. CI runs `check`; the release workflow runs `sync` after
// `changeset version`, and `publish` before versioning and after pushing a new version.
//   bun scripts/release.ts version          the current version
//   bun scripts/release.ts check [vX.Y.Z]   everything agrees, and with the tag when one is given
//   bun scripts/release.ts sync             copy package.json's version everywhere and tidy its CHANGELOG.md section
//   bun scripts/release.ts publish          the current version's GitHub release, unless it has one (needs gh and GH_TOKEN)
//   bun scripts/release.ts changeset [base]  the branch brings a changeset (base defaults to origin/main)
import fs from 'node:fs';
import path from 'node:path';

const root = path.resolve(import.meta.dir, '..');
const file = (rel: string) => path.join(root, rel);
const read = (rel: string) => fs.readFileSync(file(rel), 'utf8');
const write = (rel: string, change: (text: string) => string) => fs.writeFileSync(file(rel), change(read(rel)));

/** Files whose install instructions clone the current release (docs/development.md clones main on purpose). */
const INSTALL_DOCS = ['README.md', 'site/index.html', 'docs/macos-setup.md', 'docs/linux-deployment.md'];
const CLONE = /git clone (?:--branch (\S+) )?https:\/\/github\.com\/ChrisSku\/briardr\.git/g;
const SEMVER = /^\d+\.\d+\.\d+$/;
const CARGO_VERSION = /^(\[package\][^[]*?\nversion = )"([^"]*)"/m;
const LOCK_VERSION = /(name = "briardr-server"\nversion = )"([^"]*)"/;
const heading = (v: string) => new RegExp(`^## ${v.replace(/\./g, '\\.')}\\b.*$`, 'm');

function fail(message: string): never {
  console.error(`release: ${message}`);
  process.exit(1);
}

function version(): string {
  const v = JSON.parse(read('package.json')).version;
  if (typeof v !== 'string' || !SEMVER.test(v)) fail(`package.json has no X.Y.Z version (found ${v ?? 'none'})`);
  return v;
}

/** Where `v`'s CHANGELOG.md section starts (after its heading) and ends; null when there's none. */
function section(text: string, v: string): [number, number] | null {
  const start = text.search(heading(v));
  if (start < 0) return null;
  const body = text.indexOf('\n', start) + 1;
  const end = text.slice(body).search(/^## /m);
  return [body, end < 0 ? text.length : body + end];
}

/** The CHANGELOG.md section for `v`, without its heading; null when there's none. */
function notes(v: string): string | null {
  const text = read('CHANGELOG.md');
  const at = section(text, v);
  return at && text.slice(...at).trim();
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
  const cargo = read('server/Cargo.toml').match(CARGO_VERSION)?.[2];
  if (cargo !== v) problems.push(`server/Cargo.toml says ${cargo} (run bun scripts/release.ts sync)`);
  const lock = read('server/Cargo.lock').match(LOCK_VERSION)?.[2];
  if (lock !== v) problems.push(`server/Cargo.lock says ${lock} (run bun scripts/release.ts sync)`);
  if (!/\w/.test(notes(v) ?? '')) problems.push(`CHANGELOG.md has no notes under "## ${v}"`);
  for (const rel of INSTALL_DOCS) {
    const clones = [...read(rel).matchAll(CLONE)];
    if (!clones.length) problems.push(`${rel} has no git clone of the release`);
    for (const [line, branch] of clones) if (branch !== `v${v}`) problems.push(`${rel}: "${line}" should clone --branch v${v}`);
  }
  if (tag !== undefined && tag !== `v${v}`) problems.push(`the tag ${tag} doesn't match the version (v${v})`);
  if (problems.length) fail(`version ${v} isn't consistent:\n  ${problems.join('\n  ')}`);
  console.log(`version ${v}: Cargo.toml, Cargo.lock, CHANGELOG.md and the install instructions agree${tag ? `, and so does ${tag}` : ''}`);
}

/** After `changeset version`: package.json's version goes everywhere else, and its CHANGELOG.md section
 *  loses the "### Patch Changes" style headings Changesets groups entries under (the version number
 *  already says which kind it is), so it's one list like the sections written by hand. */
function sync() {
  const v = version();
  write('server/Cargo.toml', (t) => t.replace(CARGO_VERSION, `$1"${v}"`));
  write('server/Cargo.lock', (t) => t.replace(LOCK_VERSION, `$1"${v}"`));
  for (const rel of INSTALL_DOCS) write(rel, (t) => t.replace(CLONE, (line) => line.replace(/^git clone (--branch \S+ )?/, `git clone --branch v${v} `)));
  write('CHANGELOG.md', (t) => {
    const at = section(t, v);
    if (!at) return t;
    const body = t.slice(...at).replace(/^### (Major|Minor|Patch) Changes\n\n/gm, '').replace(/(\S)\n\n(?=- )/g, '$1\n');
    return t.slice(0, at[0]) + body + t.slice(at[1]);
  });
  console.log(`version ${v} copied to Cargo.toml, Cargo.lock and the install instructions`);
}

/** The current version's GitHub release, unless it has one: the tag goes on the commit that set the version
 *  (its "Version X.Y.Z" commit, also when main has moved on since), and the notes are its CHANGELOG.md
 *  section. The release workflow runs it after pushing a new version, and before versioning more
 *  changesets: a version whose release failed is released then, so a newer one never skips it. */
function publish() {
  const v = version();
  const tag = `v${v}`;
  if (Bun.spawnSync(['gh', 'release', 'view', tag], { cwd: root }).exitCode === 0) return console.log(`${tag} is already released`);
  check(tag);
  const log = Bun.spawnSync(['git', 'log', '-1', '--format=%H', `-S"version": "${v}"`, '--', 'package.json'], { cwd: root });
  const commit = log.stdout.toString().trim();
  if (log.exitCode !== 0 || !commit) fail(`no commit in the history sets package.json's version to ${v}`);
  const notesText = unwrap(notes(v) ?? fail(`CHANGELOG.md has no notes for ${v}`));
  const create = Bun.spawnSync(['gh', 'release', 'create', tag, '--target', commit, '--title', `Briardr ${v}`, '--notes', notesText], {
    cwd: root,
    stdout: 'inherit',
    stderr: 'inherit',
  });
  if (create.exitCode !== 0) fail(`gh release create ${tag} failed`);
  console.log(`released ${tag} at ${commit}`);
}

/** The branch brings a changeset compared with `base` (the pre-push hook and CI). Changesets' own message
 *  says to run `changeset add`, which asks questions an agent can't answer, so this says what to write. */
function changeset(base: string) {
  const status = Bun.spawnSync([process.execPath, '--bun', 'changeset', 'status', `--since=${base}`], { cwd: root });
  if (status.exitCode === 0) return console.log(`this branch brings a changeset (compared with ${base})`);
  const output = `${status.stdout}${status.stderr}`;
  if (!output.includes('no changesets were found')) fail(`changeset status --since=${base} failed:\n${output}`);
  fail(`this branch has no changeset (compared with ${base}). Write one by hand, like .changeset/pinned-streams.md:

  ---
  "briardr": patch
  ---

  **What changed, as one bold sentence.** Then the detail someone upgrading needs, wrapped at about 105
  columns. It becomes this pull request's CHANGELOG.md entry.

patch fixes something; minor adds something or changes setup or behavior. When nothing users notice
changes (docs, CI, tests), add an empty one instead: mise run changeset --empty
More in .changeset/README.md. Don't run \`changeset add\` without --empty: it asks questions.`);
}

const [command, arg] = process.argv.slice(2);
if (command === 'version') console.log(version());
else if (command === 'check') check(arg);
else if (command === 'sync') sync();
else if (command === 'publish') publish();
else if (command === 'changeset') changeset(arg ?? 'origin/main');
else fail('usage: bun scripts/release.ts version | check [vX.Y.Z] | sync | publish | changeset [base]');
