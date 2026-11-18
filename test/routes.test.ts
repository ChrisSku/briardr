// Addresses of the app's places: each place's path opens that place again, and old or odd addresses open the
// nearest place there is.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { parse, pathOf, type Route } from '../web/src/routes.ts';

test('each place has a path that opens it again', () => {
  const places: [Route, string][] = [
    [{ name: 'agents' }, '/agents'],
    [{ name: 'workspaces' }, '/workspaces'],
    [{ name: 'settings', section: 'plugins' }, '/settings/plugins'],
    [{ name: 'stream', id: 'w1:p2', surface: 'terminal' }, '/stream/w1%3Ap2'],
    [{ name: 'stream', id: 'w1:p2', surface: 'files' }, '/stream/w1%3Ap2/files'],
    [{ name: 'app', tab: 'board' }, '/board'],
  ];
  for (const [route, path] of places) {
    assert.equal(pathOf(route), path);
    assert.deepEqual(parse(path), route, path);
  }
  assert.equal(pathOf({ name: 'settings' }), '/settings');
  assert.deepEqual(parse('/settings'), { name: 'settings', section: undefined });
});

test('any stream ID survives its path', () => {
  for (const id of ['w1:p2', 'a/b', 'with space', '100%', 'q?x=1#top', 'ünï-cødé', '..', '%3A']) {
    const path = pathOf({ name: 'stream', id, surface: 'notes' });
    assert.match(path, /^\/stream\/[^/]+\/notes$/, id);
    assert.deepEqual(parse(path), { name: 'stream', id, surface: 'notes' }, id);
  }
});

test('the bare address, extra slashes and old paths open a place', () => {
  assert.deepEqual(parse('/'), { name: 'agents' });
  assert.deepEqual(parse(''), { name: 'agents' });
  assert.deepEqual(parse('//workspaces/'), { name: 'workspaces' });
  assert.deepEqual(parse('/stream//w1%3Ap2//'), { name: 'stream', id: 'w1:p2', surface: 'terminal' });
  assert.deepEqual(parse('/usage'), { name: 'settings', section: 'usage' }, 'Usage was a tab');
  assert.deepEqual(parse('/stream'), { name: 'agents' }, 'a stream without an ID');
  assert.deepEqual(parse('/agents/extra'), { name: 'agents' });
});

test('an address that can’t be decoded opens Agents', () => {
  for (const path of ['/stream/100%', '/stream/%E0%A4%A', '/settings/%zz', '/%']) assert.deepEqual(parse(path), { name: 'agents' }, path);
});
