// Lists show what an agent says it's doing (its terminal title), when that adds something.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { taskOf } from '../web/src/task.ts';

const agent = { kind: 'agent' as const, name: 'auth refactor', tool: 'claude', title: null as string | null, folder: 'app', cwd: '/srv/app' };

test('an agent’s terminal title is its task', () => {
  assert.equal(taskOf({ ...agent, title: ' Fix the flaky login test ' }), 'Fix the flaky login test');
});

test('a title that repeats the name, the tool or the folder isn’t shown', () => {
  for (const title of [null, '', '  ', 'auth refactor', 'Claude', 'app', '/srv/app', '~/code/app']) assert.equal(taskOf({ ...agent, title }), null, String(title));
});

test('shells and other programs have no task: their title is the folder or the command', () => {
  assert.equal(taskOf({ ...agent, kind: 'shell', tool: 'zsh', title: 'make test' }), null);
  assert.equal(taskOf({ ...agent, kind: 'program', tool: 'htop', title: 'htop' }), null);
});
