// Fingers, the mouse and the wheel on the terminal: a finger is a tap until it moves past the slop, then
// a swipe; swipes and wheels scroll one row per row crossed, the rest of a row carrying over; and the
// mouse's buttons and modifiers are what Herdr gets with a mouse event.
import assert from 'node:assert/strict';
import { test } from 'bun:test';
import { modifiers, pressedButton } from '../web/src/screens/stream/surface/mouse.ts';
import { RowScroll, wheelPixels } from '../web/src/screens/stream/surface/scroll.ts';
import { SWIPE_SLOP, TouchGesture } from '../web/src/screens/stream/surface/touch.ts';

const H = 20; // a row's height in px

test('pixels moved become whole rows crossed, negative towards the top; the rest of a row carries over', () => {
  const s = new RowScroll();
  assert.equal(s.add(-15, H), 0);
  assert.equal(s.add(-15, H), -1, 'the two moves together cross a row');
  assert.equal(s.add(-50, H), -3, 'what was left of the row counts too');
  assert.equal(s.add(45, H), 2, 'back the other way');
  const steps = new RowScroll();
  let rows = 0;
  for (let i = 0; i < 10; i++) rows += steps.add(-(5.5 * H) / 10, H);
  assert.equal(rows, -5, 'five and a half rows in ten moves');
});

test('a wheel counting lines moves a row per line; one counting pixels, its pixels', () => {
  assert.equal(wheelPixels({ deltaY: -3, deltaMode: 1 }, H), -60);
  assert.equal(wheelPixels({ deltaY: -3, deltaMode: 0 }, H), -3);
});

test('a finger that stays within the slop is a tap and scrolls nothing', () => {
  const g = new TouchGesture(1, 100, 100, { col: 4, row: 2 });
  assert.equal(g.move(100 + SWIPE_SLOP * 0.6, 100 + SWIPE_SLOP * 0.6, 4), 0, 'though past a row, it hasn’t moved past the slop');
  assert.equal(g.swipe, false);
});

test('past the slop it’s a swipe, which is never a hold, and scrolls from where the finger went down', async () => {
  const g = new TouchGesture(1, 100, 100, { col: 4, row: 2 });
  let held = false;
  g.hold = setTimeout(() => (held = true), 0);
  assert.equal(g.move(100, 100 + 1.5 * H, H), -1, 'a finger moving down scrolls the content up, as a wheel turned up');
  assert.equal(g.swipe, true);
  assert.equal(g.hold, null);
  assert.equal(g.move(100, 100 + 3 * H, H), -2, 'the half row left over counts');
  assert.equal(g.move(100, 100 + H, H), 2, 'moving back scrolls back');
  assert.equal(g.move(100, 100 + 0.5 * H, H), 0, 'a swipe that crosses no row scrolls nothing, and stays a swipe');
  assert.equal(g.swipe, true);
  assert.deepEqual(g.cell, { col: 4, row: 2 }, 'anchored at the cell where it started');
  await new Promise((done) => setTimeout(done, 5));
  assert.equal(held, false, 'the hold was stopped');
});

test('the mouse’s buttons: the right one, and a Control-click on a Mac, open the menu instead of reaching the program', () => {
  assert.equal(pressedButton({ button: 0, ctrlKey: false }, true), 'left');
  assert.equal(pressedButton({ button: 1, ctrlKey: false }, true), 'middle');
  assert.equal(pressedButton({ button: 2, ctrlKey: false }, false), null);
  assert.equal(pressedButton({ button: 0, ctrlKey: true }, true), null, 'Control-click on a Mac');
  assert.equal(pressedButton({ button: 0, ctrlKey: true }, false), 'left', 'Control-click elsewhere');
});

test('the modifiers held go with a mouse event as Shift 1, Control 2 and Option 4', () => {
  const held = (shiftKey: boolean, ctrlKey: boolean, altKey: boolean) => modifiers({ shiftKey, ctrlKey, altKey });
  assert.equal(held(false, false, false), 0);
  assert.equal(held(true, false, false), 1);
  assert.equal(held(false, true, false), 2);
  assert.equal(held(false, false, true), 4);
  assert.equal(held(true, true, true), 7);
});
