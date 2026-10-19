// Notification taps: which open window takes the tap, and where a new one opens.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { alertPath, pickWindow } from '../web/src/alert-window.ts';

const w = (path: string, focused = false, visibilityState = 'hidden') => ({ url: `https://briardr.test${path}`, focused, visibilityState });

test('a tap goes to the window already showing that stream, then the focused, a visible, or the most recent one', () => {
  const a = w('/agents', false, 'visible');
  const b = w('/stream/w1%3Ap2/notes');
  const c = w('/settings', true, 'visible');
  assert.equal(pickWindow([a, b, c], 'w1:p2'), b, 'showing the stream (any of its tabs)');
  assert.equal(pickWindow([a, w('/stream/w1%3Ap22'), c], 'w1:p2'), c, 'a different stream with a longer ID does not count');
  assert.equal(pickWindow([a, c], 'w1:p2'), c, 'focused');
  assert.equal(pickWindow([w('/agents'), a], null), a, 'visible');
  const first = w('/workspaces');
  assert.equal(pickWindow([first, w('/agents')], 'x'), first, 'most recent');
  assert.equal(pickWindow([], 'x'), undefined, 'none: a new window opens');
});

test('a new window opens at the alert’s stream, marked as opened from an alert', () => {
  assert.equal(alertPath('w1:p2'), '/stream/w1%3Ap2?from=alert');
  assert.equal(alertPath(null), '/');
});
