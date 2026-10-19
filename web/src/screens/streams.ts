// Helpers shared by several screens: battery text, Herdr problems, stream context, seen markers,
// opening a stream and its actions menu.
import { navigate } from '../router.ts';
import { mac, model, seen, seenKey } from '../store.ts';
import type { Stream } from '../types.ts';
import type { IconName } from '../ui/icons/index.ts';
import { duration, type MenuItem } from '../ui/kit.ts';
import { theHost } from '../host.ts';
import { caps } from '../host.ts';

export function batteryText(): { pct: string; detail: string; icon: IconName | null } | null {
  const b = mac.value?.battery;
  if (!b || !caps().battery) return null;
  if (!b.present) return { pct: 'No battery', detail: b.source === 'ac' ? 'Power adapter' : '', icon: 'plug' };
  const pct = `${b.percent}%`;
  const source = b.source === 'ac' ? (b.charged ? 'Charged' : b.charging ? 'Charging' : 'On power, not charging') : 'On battery';
  let time = '';
  if (b.timeKind === 'remaining' && b.minutes !== null) time = `${duration(b.minutes * 60000)} left`;
  else if (b.timeKind === 'until_full' && b.minutes !== null) time = `${duration(b.minutes * 60000)} until full`;
  else if (b.timeKind === 'calculating') time = 'Calculating…';
  else if (!b.charged) time = 'Time unavailable';
  return { pct, detail: [source, time].filter(Boolean).join(' · '), icon: b.charging ? 'bolt' : b.source === 'ac' ? 'plug' : null };
}

export function herdrProblem(p: string | null): string {
  switch (p) {
    case 'not_found':
      return 'Herdr isn’t installed at the configured path';
    case 'not_running':
      return `Herdr isn’t running on ${theHost()}`;
    case 'incompatible':
      return 'This Herdr version isn’t compatible';
    case 'timeout':
      return 'Herdr isn’t answering';
    default:
      return 'Herdr isn’t reachable';
  }
}

export function contextOf(s: Stream): string {
  const m = model.value;
  const ws = m?.workspaces.find((w) => w.id === s.workspaceId)?.name;
  const pr = m?.projects.find((p) => p.id === s.projectId)?.name;
  return [ws, pr].filter(Boolean).join(' · ');
}

export function isSeen(s: Stream): boolean {
  return seen.value[s.id] === seenKey(s);
}

export function markSeen(s: Stream, on = true) {
  const next = { ...seen.value };
  if (on) next[s.id] = seenKey(s);
  else delete next[s.id];
  seen.value = next;
}

export function openStream(s: Stream) {
  if (s.state === 'done') markSeen(s);
  navigate({ name: 'stream', id: s.id, surface: 'terminal' });
}

export function streamMenu(s: Stream, peek: (s: Stream) => void): MenuItem[] {
  const items: MenuItem[] = [
    { label: 'Peek', hint: 'Read-only', onSelect: () => peek(s) },
    { label: 'Open', hint: 'Take control', onSelect: () => openStream(s) },
  ];
  if (s.state === 'done') items.push(isSeen(s) ? { label: 'Mark as not seen', onSelect: () => markSeen(s, false) } : { label: 'Mark as seen', onSelect: () => markSeen(s) });
  return items;
}
