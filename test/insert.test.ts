// Files sent to a stream: what each one is, and how their paths go into the terminal.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { droppedFiles, dropRefusal, joinPaths, reviewsPaste, uploadItems } from '../web/src/screens/stream/insert.ts';

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

test('a drop’s folders are left out and counted: the browser can’t read one', () => {
  const file = new File(['x'], 'notes.txt');
  const item = (kind: string, isDirectory: boolean) => ({ kind, getAsFile: () => (kind === 'file' ? file : null), webkitGetAsEntry: () => ({ isDirectory }) as FileSystemEntry });
  assert.deepEqual(droppedFiles([item('file', false), item('file', true), item('string', false)]), { files: [file], folders: 1 });
});

test('files dropped on a terminal are offered only when it’s in your control and on this computer', () => {
  assert.equal(dropRefusal({}, true), null);
  assert.equal(dropRefusal({}, false), 'Take control to send files.', 'a terminal you watch gets nothing');
  assert.equal(dropRefusal({ machine: 'studio' }, true), 'Files can’t be sent to a terminal on studio.', 'an upload lands on this computer');
  assert.equal(dropRefusal({ machine: 'studio' }, false), 'Files can’t be sent to a terminal on studio.');
});

test('a paste from the keyboard is reviewed first when it’s long or has line breaks; a short line goes in as it is', () => {
  assert.equal(reviewsPaste('git status'), false);
  assert.equal(reviewsPaste('x'.repeat(2000)), false);
  assert.equal(reviewsPaste('x'.repeat(2001)), true);
  assert.equal(reviewsPaste('one\ntwo'), true);
  assert.equal(reviewsPaste('ends with a line break\n'), true);
});
