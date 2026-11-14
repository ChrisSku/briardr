// Files sent to a stream: what each one is, and how their paths go into the terminal.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { joinPaths, uploadItems } from '../web/src/screens/stream/insert.ts';

test('an agent gets each path on a line of its own, any other program gets them as arguments', () => {
  const quoted = ["'/up/a/one.png'", "'/up/b/two words.txt'"];
  assert.equal(joinPaths(quoted, { kind: 'agent' }), "'/up/a/one.png'\n'/up/b/two words.txt'");
  assert.equal(joinPaths(quoted, { kind: 'shell' }), "'/up/a/one.png' '/up/b/two words.txt'");
  assert.equal(joinPaths(quoted.slice(0, 1), { kind: 'agent' }), "'/up/a/one.png'");
});

test('images are told from other files by their type', () => {
  const files = [new File(['x'], 'photo.jpg', { type: 'image/jpeg' }), new File(['x'], 'notes.txt', { type: 'text/plain' }), new File(['x'], 'unknown')];
  assert.deepEqual(uploadItems(files).map((i) => i.kind), ['image', 'file', 'file']);
});
