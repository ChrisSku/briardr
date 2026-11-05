// Helpers shared by several screens: battery text, Herdr problems, stream context, seen markers,
// opening a stream, renaming and closing, and a stream's actions menu.
import { api, ApiError } from '../api.ts';
import { navigate } from '../router.ts';
import { listPane, live, mac, model, seen, seenKey, toast, visibleStreamId } from '../store.ts';
import type { Project, Stream, Workspace } from '../types.ts';
import type { IconName } from '../ui/icons/index.ts';
import { confirmDialog, duration, type MenuItem } from '../ui/kit.ts';
import { openRename } from './RenameSheet.svelte';
import { theHost } from '../host.ts';
import { caps } from '../host.ts';
import { canOpenWindows, openWindow } from '../windows.ts';

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

const plural = (n: number, noun: string) => `${n} ${noun}${n === 1 ? '' : 's'}`;

/** Asks, closes it on the host, and leaves a stream that closed with it. Closing stops what runs inside. */
async function close(what: string, name: string, body: string, path: string, streams: Stream[]) {
  if (!(await confirmDialog(`Close ${what} “${name}”?`, `${body} This can’t be undone.`, 'Close', true))) return;
  try {
    await api('POST', path);
    toast('Closed');
    if (streams.some((s) => s.id === visibleStreamId.value)) navigate({ name: listPane.value }, { replace: true });
  } catch (e) {
    toast(e instanceof ApiError ? e.message : 'Couldn’t close it.', 5000);
  }
}

/** Herdr closes a project with its last stream, and a workspace with its last project. */
function closesWith(projectId: string, closingStream: boolean): string {
  const m = model.value;
  const p = m?.projects.find((x) => x.id === projectId);
  if (!m || !p || (closingStream && m.streams.filter((x) => x.projectId === p.id).length > 1)) return '';
  const lastProject = m.projects.filter((x) => x.workspaceId === p.workspaceId).length === 1;
  if (lastProject) return ` Workspace “${m.workspaces.find((w) => w.id === p.workspaceId)?.name}” closes with it.`;
  return closingStream ? ` Project “${p.name}” closes with it.` : '';
}

/** Rename and Close for a menu, off while there's no connection. */
function manageItems(what: string, rename: () => void, closeIt: () => void): MenuItem[] {
  return [
    { label: 'Rename', disabled: !live.value, onSelect: rename },
    { label: `Close ${what}`, destructive: true, disabled: !live.value, onSelect: closeIt },
  ];
}

export function workspaceItems(ws: Workspace): MenuItem[] {
  const streams = model.value?.streams.filter((s) => s.workspaceId === ws.id) ?? [];
  return manageItems(
    'workspace',
    () => openRename({ noun: 'workspace', path: `/api/workspaces/${encodeURIComponent(ws.id)}/rename`, name: ws.name }),
    () => close('workspace', ws.name, `This stops ${plural(streams.length, 'stream')} and everything running in them on ${theHost()}.`, `/api/workspaces/${encodeURIComponent(ws.id)}/close`, streams),
  );
}

export function projectItems(p: Project): MenuItem[] {
  const streams = model.value?.streams.filter((s) => s.projectId === p.id) ?? [];
  return manageItems(
    'project',
    () => openRename({ noun: 'project', path: `/api/projects/${encodeURIComponent(p.id)}/rename`, name: p.name }),
    () => close('project', p.name, `This stops ${plural(streams.length, 'stream')} and everything running in them on ${theHost()}.${closesWith(p.id, false)}`, `/api/projects/${encodeURIComponent(p.id)}/close`, streams),
  );
}

export function streamItems(s: Stream): MenuItem[] {
  return manageItems(
    'stream',
    () => openRename({ noun: 'stream', path: `/api/streams/${encodeURIComponent(s.id)}/rename`, name: s.label ?? '', fallback: s.tool }),
    () => close('stream', s.name, `This stops ${s.kind === 'agent' ? 'the agent and ' : ''}everything running in its terminal on ${theHost()}.${closesWith(s.projectId, true)}`, `/api/streams/${encodeURIComponent(s.id)}/close`, [s]),
  );
}

export function streamMenu(s: Stream, peek: (s: Stream) => void): MenuItem[] {
  const items: MenuItem[] = [
    { label: 'Peek', hint: 'Read-only', onSelect: () => peek(s) },
    { label: 'Open', hint: 'Take control', onSelect: () => openStream(s) },
  ];
  if (canOpenWindows) items.push({ label: 'Open in New Window', onSelect: () => openWindow({ name: 'stream', id: s.id, surface: 'terminal' }) });
  if (s.state === 'done') items.push(isSeen(s) ? { label: 'Mark as not seen', onSelect: () => markSeen(s, false) } : { label: 'Mark as seen', onSelect: () => markSeen(s) });
  items.push(...streamItems(s));
  return items;
}
