// Quiet hours: a span of this device's day without alerts, which the host keeps (push.ts). The host
// can't see this device's time zone, so it is told the clock: how far ahead of UTC it is, and when that
// changes next (summer time).

const MINUTE = 60_000;
const DAY = 24 * 60 * MINUTE;

/** This device's clock as the host keeps it: minutes ahead of UTC now, and the next change of that. */
export interface DeviceClock {
  utcOffset: number;
  next?: { at: number; utcOffset: number };
}

/** Minutes this device's clock is ahead of UTC at `t` (0 minus, so UTC is 0 and not -0). */
const offsetAt = (t: number) => 0 - new Date(t).getTimezoneOffset();

/** The clock at `now`, with its next change within a year, to the minute. */
export function deviceClock(now = Date.now()): DeviceClock {
  const utcOffset = offsetAt(now);
  for (let day = now; day < now + 366 * DAY; day += DAY) {
    if (offsetAt(day + DAY) === utcOffset) continue;
    // It changes within this day: find the first minute of the new offset.
    let [lo, hi] = [Math.floor(day / MINUTE), Math.ceil((day + DAY) / MINUTE)];
    while (hi - lo > 1) {
      const mid = Math.floor((lo + hi) / 2);
      if (offsetAt(mid * MINUTE) === utcOffset) lo = mid;
      else hi = mid;
    }
    return { utcOffset, next: { at: hi * MINUTE, utcOffset: offsetAt(hi * MINUTE) } };
  }
  return { utcOffset };
}

export const sameClock = (a: DeviceClock, b: DeviceClock) => a.utcOffset === b.utcOffset && a.next?.at === b.next?.at && a.next?.utcOffset === b.next?.utcOffset;

/** Minutes after midnight as a time input's value ("07:30"), and back. */
export const timeOfDay = (minutes: number) => `${String(Math.floor(minutes / 60)).padStart(2, '0')}:${String(minutes % 60).padStart(2, '0')}`;
export const minutesOf = (time: string) => Number(time.slice(0, 2)) * 60 + Number(time.slice(3, 5));
