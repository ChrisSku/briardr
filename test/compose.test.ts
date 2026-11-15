// Compose keeps a draft per stream and the messages inserted last, on the device.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { MAX_RECENT, remembered, withDraft } from '../web/src/screens/stream/insert.ts';

test('an inserted message goes to the front, once; empty and very long ones aren’t kept', () => {
  assert.deepEqual(remembered(['b', 'a'], ' a \n'), ['a', 'b']);
  assert.deepEqual(remembered(['a'], '   '), ['a']);
  assert.deepEqual(remembered(['a'], 'x'.repeat(4001)), ['a']);
  const many = Array.from({ length: MAX_RECENT }, (_, i) => `m${i}`);
  const next = remembered(many, 'new');
  assert.equal(next.length, MAX_RECENT);
  assert.equal(next[0], 'new');
  assert.ok(!next.includes(`m${MAX_RECENT - 1}`), 'the oldest one goes');
});

test('a stream has one draft; an empty one is removed and the oldest drafts go first', () => {
  assert.deepEqual(withDraft({ a: 'one' }, 'b', 'two'), { a: 'one', b: 'two' });
  assert.deepEqual(withDraft({ a: 'one', b: 'two' }, 'a', 'again'), { b: 'two', a: 'again' });
  assert.deepEqual(withDraft({ a: 'one', b: 'two' }, 'a', '  '), { b: 'two' });
  let drafts: Record<string, string> = {};
  for (let i = 0; i < 40; i++) drafts = withDraft(drafts, `s${i}`, 'text');
  assert.equal(Object.keys(drafts).length, 30);
  assert.ok(!('s0' in drafts) && 's39' in drafts);
});
