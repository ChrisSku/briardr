// Jump search order: needs input first, then the most recent of opened / finished / started working.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { byActivity } from '../web/src/activity.ts';

test('streams that need input come first; the rest by latest activity', () => {
  const s = (id: string, state: any, stateSince: number | null) => ({ id, state, stateSince });
  const streams = [
    s('idle-old', 'idle', 100), // idle time doesn't count as activity
    s('finished', 'done', 500),
    s('needs-old', 'blocked', 10),
    s('working', 'working', 400),
    s('opened', 'idle', null),
    s('needs-new', 'blocked', 900),
    s('never', null, null),
  ];
  const order = byActivity(streams, { opened: 700, 'idle-old': 50 }).map((x) => x.id);
  assert.deepEqual(order, ['needs-new', 'needs-old', 'opened', 'finished', 'working', 'idle-old', 'never']);
});

test('opening a stream moves it ahead of older activity', () => {
  const streams = [{ id: 'a', state: 'done' as const, stateSince: 500 }, { id: 'b', state: 'done' as const, stateSince: 300 }];
  assert.deepEqual(byActivity(streams, {}).map((x) => x.id), ['a', 'b']);
  assert.deepEqual(byActivity(streams, { b: 600 }).map((x) => x.id), ['b', 'a']);
});
