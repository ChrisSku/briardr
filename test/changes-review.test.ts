// Changes: lines picked in a diff become review comments, pasted into the terminal as one prompt (the
// paste itself is checked in test-ui/interactions.test.ts).
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { lineId, parseDiff, whereOf } from '../plugins/changes/src/diff.ts';
import { codeBlock } from '../web/src/screens/stream/insert.ts';

const diff = ['diff --git a/src/a.rs b/src/a.rs', 'index 111..222 100644', '--- a/src/a.rs', '+++ b/src/a.rs', '@@ -10,4 +10,5 @@ fn main() {', ' let a = 1;', '-let b = 2;', '+let b = 3;', '+let c = 4;', ' done();', '\\ No newline at end of file'].join('\n');

test('diff lines are numbered from the hunk headers: removed lines in the old file, the rest in the new one', () => {
  const lines = parseDiff(diff);
  assert.deepEqual(lines.map((l) => [l.k, l.n]), [['dl-meta', null], ['dl-meta', null], ['dl-meta', null], ['dl-meta', null], ['dl-hunk', null], ['dl-ctx', 10], ['dl-del', 11], ['dl-add', 11], ['dl-add', 12], ['dl-ctx', 13], ['dl-ctx', null]]);
  assert.deepEqual(lines.map(lineId).filter(Boolean), ['+10', '-11', '+11', '+12', '+13']);
  // The line break a diff ends with is not a line of its own.
  assert.equal(parseDiff(diff + '\n').length, lines.length);
  assert.deepEqual(parseDiff(''), []);
});

test('picked lines are named by their numbers in the new file, or in the old one when all were removed', () => {
  const lines = parseDiff(diff);
  assert.equal(whereOf(lines.slice(7, 8)), '11');
  assert.equal(whereOf(lines.slice(5, 9)), '10-12');
  assert.equal(whereOf(lines.slice(6, 7)), '11 (removed)');
  assert.equal(whereOf(lines.slice(4, 5)), '');
});

test('a code block’s fence is longer than any run of backticks in the lines it holds', () => {
  assert.equal(codeBlock('-let b = 2;\n+let b = 3;', 'diff'), '```diff\n-let b = 2;\n+let b = 3;\n```');
  assert.equal(codeBlock('+```sh', 'diff'), '````diff\n+```sh\n````');
  assert.equal(codeBlock('a ````` b'), '``````\na ````` b\n``````');
});
