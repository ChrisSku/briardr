// Quiet hours are kept on the host by the device's clock: its offset from UTC and when that changes next.
import assert from 'node:assert/strict';
import { afterEach, test } from 'bun:test';
import { deviceClock, minutesOf, sameClock, timeOfDay } from '../web/src/quiet-hours.ts';

// Bun follows TZ as it changes; each test puts it back.
const zone = process.env.TZ;
afterEach(() => {
  if (zone === undefined) delete process.env.TZ;
  else process.env.TZ = zone;
});

test('the clock says when summer time starts and ends, to the minute', () => {
  process.env.TZ = 'Europe/Berlin';
  assert.deepEqual(deviceClock(Date.UTC(2026, 0, 15, 12)), { utcOffset: 60, next: { at: Date.UTC(2026, 2, 29, 1), utcOffset: 120 } });
  assert.deepEqual(deviceClock(Date.UTC(2026, 5, 1)), { utcOffset: 120, next: { at: Date.UTC(2026, 9, 25, 1), utcOffset: 60 } });
  // Half an hour, south of the equator.
  process.env.TZ = 'Australia/Lord_Howe';
  assert.deepEqual(deviceClock(Date.UTC(2026, 6, 1)), { utcOffset: 630, next: { at: Date.UTC(2026, 9, 3, 15, 30), utcOffset: 660 } });
});

test('a clock without summer time has no next change', () => {
  process.env.TZ = 'Asia/Kolkata';
  assert.deepEqual(deviceClock(Date.UTC(2026, 0, 1)), { utcOffset: 330 });
  process.env.TZ = 'UTC';
  assert.deepEqual(deviceClock(Date.UTC(2026, 0, 1)), { utcOffset: 0 });
});

test('the host is told again only when the clock changed', () => {
  const winter = { utcOffset: 60, next: { at: 1, utcOffset: 120 } };
  assert.ok(sameClock(winter, { ...winter, next: { ...winter.next } }));
  assert.ok(!sameClock(winter, { utcOffset: 120 }));
  assert.ok(!sameClock(winter, { utcOffset: 60, next: { at: 2, utcOffset: 120 } }));
  assert.ok(sameClock({ utcOffset: 0 }, { utcOffset: 0 }));
});

test('times of day are minutes after midnight', () => {
  assert.equal(timeOfDay(22 * 60), '22:00');
  assert.equal(timeOfDay(7 * 60 + 5), '07:05');
  assert.equal(minutesOf('06:30'), 390);
  assert.equal(minutesOf(timeOfDay(1439)), 1439);
});
