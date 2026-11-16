// Reconnecting waits twice as long after each failed attempt, up to a cap, give or take a fifth so that
// devices dropped together don't come back together; a returning network starts the waits over.
import assert from 'node:assert/strict';
import { afterEach, beforeEach, spyOn, test, type Mock } from 'bun:test';
import { Backoff } from '../web/src/backoff.ts';

// Math.random gives the jitter: each test sets it, and every other test file gets the real one.
let random: Mock<() => number>;
beforeEach(() => (random = spyOn(Math, 'random')));
afterEach(() => random.mockRestore());

const waits = (b: Backoff, n: number) => Array.from({ length: n }, () => b.next());

test('each wait doubles the one before, up to the cap', () => {
  random.mockReturnValue(0.5); // no jitter
  const b = new Backoff();
  assert.deepEqual(waits(b, 9), [500, 1000, 2000, 4000, 8000, 16000, 30000, 30000, 30000]);
  assert.equal(b.attempts, 9);
  assert.deepEqual(waits(new Backoff(100, 1000), 6), [100, 200, 400, 800, 1000, 1000]);
});

test('jitter moves a wait by up to a fifth, never below the first wait', () => {
  random.mockReturnValue(0);
  assert.deepEqual(waits(new Backoff(), 3), [500, 800, 1600]);
  random.mockReturnValue(0.999999);
  assert.deepEqual(waits(new Backoff(), 3), [600, 1200, 2400]);
  assert.deepEqual(waits(new Backoff(500, 1000), 3).slice(2), [1200]);
});

test('reset starts again from the first wait', () => {
  random.mockReturnValue(0.5);
  const b = new Backoff();
  waits(b, 4);
  b.reset();
  assert.equal(b.attempts, 0);
  assert.equal(b.next(), 500);
});
