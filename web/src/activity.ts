// Ordering by recent activity (Jump search): streams that need input first, then by the latest of
// when this device opened it, when it finished, started working or started needing input (Mac clock, ms).
import type { Stream } from './types.ts';

type Timed = Pick<Stream, 'id' | 'state' | 'stateSince'>;

export function lastActivity(s: Timed, opened: Record<string, number>): number {
  const changed = (s.state === 'done' || s.state === 'working' || s.state === 'blocked') && s.stateSince ? s.stateSince : 0;
  return Math.max(opened[s.id] || 0, changed);
}

export function byActivity<T extends Timed>(streams: T[], opened: Record<string, number>): T[] {
  const needs = (s: T) => (s.state === 'blocked' ? 0 : 1);
  return streams
    .map((s, i) => ({ s, i, at: lastActivity(s, opened) }))
    .sort((a, b) => needs(a.s) - needs(b.s) || b.at - a.at || a.i - b.i)
    .map((x) => x.s);
}
