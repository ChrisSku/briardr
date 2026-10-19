// `briardr create plugin`: makes a working plugin from the template, installs the skill for Claude Code
// and Codex, installs and turns the plugin on, and starts the agent in the new folder with the job.
// Runs with a throwaway HOME and a stand-in `claude` that records how it was started.
import assert from 'node:assert/strict';
import { spawnSync } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'bun:test';
import { SERVER_BIN, tmpDir } from './helpers.ts';

test('creates a plugin, installs the skill and hands the job to the agent', () => {
  const home = tmpDir('briardr-home-');
  const data = tmpDir('briardr-data-');
  const bin = tmpDir('briardr-bin-');
  const record = path.join(bin, 'claude.json');
  fs.writeFileSync(path.join(bin, 'claude'), `#!/bin/sh\nprintf '{"cwd":"%s","prompt":"%s"}' "$PWD" "$(printf '%s' "$1" | tr '\\n' ' ' | sed 's/"/\\\\"/g')" > ${record}\n`);
  fs.chmodSync(path.join(bin, 'claude'), 0o755);
  const run = (...args: string[]) =>
    spawnSync(SERVER_BIN, args, {
      encoding: 'utf8',
      env: { ...process.env, HOME: home, BRIARDR_DATA_DIR: data, PATH: `${bin}:${path.dirname(process.execPath)}:/usr/bin:/bin` },
    });

  const r = run('create', 'plugin', 'Build lights', '--description', 'Show whether the last CI build passed.', '--backend', '--agent', 'claude', '--yes');
  assert.equal(r.status, 0, r.stderr + r.stdout);
  const dir = path.join(home, 'briardr-plugins', 'build-lights');
  const manifest = JSON.parse(fs.readFileSync(path.join(dir, 'briardr-plugin.json'), 'utf8'));
  assert.equal(manifest.id, 'build-lights');
  assert.equal(manifest.description, 'Show whether the last CI build passed.');
  assert.deepEqual(manifest.contributes.streamTabs, [{ id: 'build-lights', label: 'Build lights', icon: 'plug' }]);
  for (const f of ['src/index.ts', 'src/Tab.svelte', 'server/main.ts', 'server/briardr.ts', 'AGENTS.md', 'CLAUDE.md', 'tsconfig.json', '.gitignore', 'dist/build.json', '.git'])
    assert.ok(fs.existsSync(path.join(dir, f)), f);
  assert.doesNotMatch(fs.readFileSync(path.join(dir, 'src', 'Tab.svelte'), 'utf8'), /\{\{/);

  // The skill, for Claude Code and for Codex, pointing at this checkout.
  for (const d of ['.claude', '.agents']) {
    const skill = fs.readFileSync(path.join(home, d, 'skills', 'briardr-plugins', 'SKILL.md'), 'utf8');
    assert.match(skill, /^---\nname: briardr-plugins/);
    assert.ok(skill.includes(path.resolve('.')), 'names the Briardr checkout');
    assert.ok(fs.existsSync(path.join(home, d, 'skills', 'briardr-plugins', 'references', 'sdk.md')));
  }

  // Installed, on, its backend approved.
  assert.equal(fs.readlinkSync(path.join(data, 'plugins', 'build-lights')), dir);
  const state = JSON.parse(fs.readFileSync(path.join(data, 'state', 'plugins.json'), 'utf8'));
  assert.equal(state.enabled['build-lights'], true);
  assert.match(state.approved['build-lights'], /^[0-9a-f]{64}$/);
  assert.match(run('plugin', 'list').stdout, /on +build-lights +Build lights/);
  assert.match(run('plugin', 'check', dir).stdout, /No problems/);

  // The agent started in the plugin's folder with the job.
  const started = JSON.parse(fs.readFileSync(record, 'utf8'));
  assert.equal(fs.realpathSync(started.cwd), fs.realpathSync(dir));
  assert.match(started.prompt, /Show whether the last CI build passed\./);
  assert.match(started.prompt, /briardr-plugins skill/);
  assert.match(started.prompt, /plugin build \./);

  // The same name again is refused.
  const again = run('create', 'plugin', 'Build lights', '--yes', '--agent', 'none');
  assert.notEqual(again.status, 0);
  assert.match(again.stderr, /already a plugin/);
}, 120_000);
