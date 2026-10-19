// The built-in plugins (plugins/<id>) are made like any installed plugin: a valid manifest, and a client
// that uses only its own files, Svelte, @briardr/sdk and npm packages. Anything a built-in needs from
// Briardr goes into the SDK, for every plugin, so each of them must also build as an installed plugin.
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'bun:test';
import { buildPlugin } from '../scripts/plugin-build.ts';
import { SDK_MODULES } from '../scripts/sdk-modules.ts';
import { BUILTIN_MANIFESTS } from '../web/src/plugins/builtin.ts';
import { tmpDir } from './helpers.ts';

const root = path.resolve(import.meta.dir, '..');
const plugins = path.join(root, 'plugins');
const ids = fs.readdirSync(plugins).filter((id) => id !== 'template' && fs.existsSync(path.join(plugins, id, 'briardr-plugin.json')));

function sources(dir: string): string[] {
  return (fs.readdirSync(dir, { recursive: true }) as string[]).filter((f) => /\.(ts|svelte)$/.test(f)).map((f) => path.join(dir, f));
}

/** Every module a file imports: static, dynamic and type-only. */
function imports(file: string): string[] {
  const text = fs.readFileSync(file, 'utf8');
  return [...text.matchAll(/(?:^|[\s;])(?:import|export)\s[^'"]*?from\s*['"]([^'"]+)['"]|import\(\s*['"]([^'"]+)['"]\s*\)|^\s*import\s+['"]([^'"]+)['"]/gm)].map((m) => m[1] || m[2] || m[3]);
}

test('every built-in plugin is in plugins/ and loaded by the app', () => {
  assert.deepEqual(ids.toSorted(), BUILTIN_MANIFESTS.map((m) => m.id).toSorted());
  for (const id of ids) {
    const manifest = JSON.parse(fs.readFileSync(path.join(plugins, id, 'briardr-plugin.json'), 'utf8'));
    assert.equal(manifest.id, id);
    assert.equal(manifest.briardr, 1);
    assert.equal(manifest.client, 'src/index.ts', `${id}: the client entry`);
    assert.ok(fs.existsSync(path.join(plugins, id, manifest.client)));
  }
});

test('built-in plugins import only their own files, Svelte, @briardr/sdk and npm packages', () => {
  const problems: string[] = [];
  for (const id of ids) {
    const dir = path.join(plugins, id);
    for (const file of sources(path.join(dir, 'src'))) {
      for (const spec of imports(file)) {
        const where = `${path.relative(root, file)}: ${spec}`;
        if (spec.startsWith('.')) {
          const target = path.resolve(path.dirname(file), spec);
          if (!target.startsWith(dir + path.sep)) problems.push(`${where} (outside the plugin; use @briardr/sdk, or add what's missing to it)`);
        } else if (spec.startsWith('/')) problems.push(`${where} (an absolute path)`);
        else if (/^(svelte|@briardr\/sdk)(\/|$)/.test(spec) && !SDK_MODULES[spec]) problems.push(`${where} (not shared with plugins: ${Object.keys(SDK_MODULES).join(', ')})`);
      }
    }
  }
  assert.deepEqual(problems, []);
});

test('each built-in plugin also builds as an installed plugin', async () => {
  const out = tmpDir('briardr-builtin-build-');
  for (const id of ids) {
    const r = await buildPlugin(path.join(plugins, id), path.join(out, id));
    assert.ok(r.ok, `${id}: ${r.errors.join('\n')}`);
    assert.ok(r.entry, `${id}: no client entry`);
    // Svelte and the SDK stay Briardr's shared copies.
    const entry = fs.readFileSync(path.join(out, id, r.entry!), 'utf8');
    assert.match(entry, /\/sdk\/v1\/briardr\.js/, `${id}: imports @briardr/sdk from /sdk/v1`);
  }
}, 120_000);
