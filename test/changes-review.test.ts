// Changes: lines picked in a diff become review comments, pasted into the terminal as one prompt.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { formatReview, lineId, parseDiff, whereOf } from '../plugins/changes/src/review.ts';

const diff = ['diff --git a/src/a.rs b/src/a.rs', 'index 111..222 100644', '--- a/src/a.rs', '+++ b/src/a.rs', '@@ -10,4 +10,5 @@ fn main() {', ' let a = 1;', '-let b = 2;', '+let b = 3;', '+let c = 4;', ' done();', '\\ No newline at end of file'].join('\n');

test('diff lines are numbered from the hunk headers: removed lines in the old file, the rest in the new one', () => {
  const lines = parseDiff(diff);
  assert.deepEqual(lines.map((l) => [l.k, l.n]), [['dl-meta', null], ['dl-meta', null], ['dl-meta', null], ['dl-meta', null], ['dl-hunk', null], ['dl-ctx', 10], ['dl-del', 11], ['dl-add', 11], ['dl-add', 12], ['dl-ctx', 13], ['dl-ctx', null]]);
  assert.deepEqual(lines.map(lineId).filter(Boolean), ['+10', '-11', '+11', '+12', '+13']);
});

test('picked lines are named by their numbers in the new file, or in the old one when all were removed', () => {
  const lines = parseDiff(diff);
  assert.equal(whereOf(lines.slice(7, 8)), '11');
  assert.equal(whereOf(lines.slice(5, 9)), '10-12');
  assert.equal(whereOf(lines.slice(6, 7)), '11 (removed)');
  assert.equal(whereOf(lines.slice(4, 5)), '');
});

test('the pasted review is the note, then each comment under its file, lines and a fence that holds them', () => {
  const text = formatReview({
    note: ' Keep the public API. ',
    comments: [
      { path: 'src/a.rs', where: '11-12', lines: '-let b = 2;\n+let b = 3;', ids: ['-11', '+11'], text: 'Why 3?\n' },
      { path: 'README.md', where: '4', lines: '+```sh', ids: ['+4'], text: 'Close the fence.' },
    ],
  });
  assert.equal(text, ['Keep the public API.', 'src/a.rs 11-12\n```diff\n-let b = 2;\n+let b = 3;\n```\nWhy 3?', 'README.md 4\n````diff\n+```sh\n````\nClose the fence.'].join('\n\n'));
  assert.equal(formatReview({ note: '', comments: [] }), '');
});
