// Files' prompt: lines picked from files, files and folders, and a question, kept per stream until sent, and the
// text it is pasted as. @briardr/sdk needs a browser (Svelte's runes), so the plugin gets the two parts of it
// that prompt.ts uses: the real codeBlock, and a persisted value in memory standing in for this device's storage.
import assert from 'node:assert/strict';
import { mock, test } from 'bun:test';
import { codeBlock } from '../web/src/screens/stream/insert.ts';

// This device's storage, with a draft saved by an older Briardr, which kept whole files in the prompt.
const storage: Record<string, { value: unknown }> = {
  'briardr.files.prompt': {
    value: { old: { question: 'Why?', snippets: [{ path: 'a.ts', from: 1, to: 9, text: '…', whole: true }, { path: 'b.ts', from: 2, to: 2, text: 'x' }], references: [{ path: 'src', dir: true }] } },
  },
};
mock.module('@briardr/sdk', () => ({ codeBlock, persisted: (key: string, initial: unknown) => (storage[key] ??= { value: initial }) }));
const { addFileReference, addSnippet, clearPrompt, fileMention, formatPrompt, promptOf, rangeLabel, removeFileReference, removeSnippet, setQuestion } = await import('../plugins/files/src/prompt.ts');

const FILE = 'fn main() {\r\n    retry();\r\n}\r\n';

test('picked lines keep their text; picking the same lines of the same file again replaces them', () => {
  addSnippet('s1', 'src/main.rs', 1, 2, FILE);
  addSnippet('s1', 'src/main.rs', 2, 3, FILE);
  addSnippet('s1', 'src/main.rs', 1, 2, 'fn main() {\n    run();\n}');
  assert.deepEqual(promptOf('s1').snippets, [
    { path: 'src/main.rs', from: 2, to: 3, text: '    retry();\n}' },
    { path: 'src/main.rs', from: 1, to: 2, text: 'fn main() {\n    run();' },
  ]);
  assert.deepEqual(promptOf('s2'), { question: '', snippets: [] }, 'another stream’s prompt');
});

test('files and folders are added once each, and anything can be removed again', () => {
  addFileReference('s3', 'README.md', false);
  addFileReference('s3', 'src', true);
  addFileReference('s3', 'README.md', false);
  assert.deepEqual(promptOf('s3').references, [{ path: 'src', dir: true }, { path: 'README.md', dir: false }]);
  removeFileReference('s3', 0);
  assert.deepEqual(promptOf('s3').references, [{ path: 'README.md', dir: false }]);
  addSnippet('s3', 'a.txt', 1, 1, 'one\ntwo');
  addSnippet('s3', 'a.txt', 2, 2, 'one\ntwo');
  removeSnippet('s3', 0);
  assert.deepEqual(promptOf('s3').snippets.map((s) => s.text), ['two']);
});

test('a cleared prompt isn’t kept, and neither is one with nothing left in it', () => {
  const kept = () => Object.keys(storage['briardr.files.prompt'].value as object);
  setQuestion('s4', 'Explain this');
  addSnippet('s4', 'a.txt', 1, 1, 'x');
  setQuestion('s5', 'Hi');
  assert.ok(kept().includes('s4') && kept().includes('s5'));
  clearPrompt('s4');
  setQuestion('s5', '');
  assert.deepEqual(promptOf('s4'), { question: '', snippets: [] });
  assert.ok(!kept().includes('s4') && !kept().includes('s5'));
});

test('an older draft keeps its whole files as references, without their contents', () => {
  assert.deepEqual(promptOf('old'), {
    question: 'Why?',
    snippets: [{ path: 'b.ts', from: 2, to: 2, text: 'x' }],
    references: [{ path: 'src', dir: true }, { path: 'a.ts', dir: false }],
  });
});

test('ranges and mentions: one line or first-last, and a folder ends in a slash', () => {
  assert.equal(rangeLabel(7, 7), '7');
  assert.equal(rangeLabel(40, 52), '40-52');
  assert.equal(fileMention({ path: 'src/app.ts', dir: false }), '@src/app.ts');
  assert.equal(fileMention({ path: 'src', dir: true }), '@src/');
});

test('the pasted text: the question, each snippet under its path and lines in a block for its language, then the files', () => {
  const text = formatPrompt({
    question: '  Why does this retry forever?\n',
    snippets: [
      { path: 'server/src/files.rs', from: 40, to: 52, text: 'loop {\n    retry();\n}' },
      { path: 'docs/README.MD', from: 3, to: 3, text: 'Run ```mise run test```.' },
      { path: 'Makefile', from: 1, to: 2, text: 'all:\n\tbuild' },
      { path: '.env', from: 1, to: 1, text: 'A=1' },
    ],
    references: [{ path: 'server/src/app.rs', dir: false }, { path: 'web/src', dir: true }],
  });
  assert.equal(
    text,
    [
      'Why does this retry forever?',
      'server/src/files.rs 40-52\n```rs\nloop {\n    retry();\n}\n```',
      'docs/README.MD 3\n````md\nRun ```mise run test```.\n````',
      'Makefile 1-2\n```\nall:\n\tbuild\n```',
      '.env 1\n```\nA=1\n```',
      '@server/src/app.rs',
      '@web/src/',
    ].join('\n\n'),
  );
  assert.equal(formatPrompt({ question: ' ', snippets: [], references: [{ path: 'a', dir: false }] }), '@a', 'no question');
  assert.equal(formatPrompt({ question: '', snippets: [] }), '');
});
