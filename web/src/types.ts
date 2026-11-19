// The model and the host as the server sends them (protocol.ts, made from the server's own types), by the
// names the client and plugins (@briardr/sdk) use, and the groups the client sorts agents into.
import type { AgentState, HostCapabilities, Stream } from './protocol.ts';

export type { AgentChoice, AgentState, Battery, HerdrHealth, MacSummary, Model, Project, Stream, StreamKind, Workspace } from './protocol.ts';

/** What the computer running Briardr supports (server/src/platform.rs). */
export type HostCaps = HostCapabilities;

export type Group = 'attention' | 'working' | 'finished' | 'ready' | 'unknown';

export function groupOf(s: Stream): Group | null {
  if (s.kind !== 'agent') return null;
  switch (s.state) {
    case 'blocked':
      return 'attention';
    case 'working':
      return 'working';
    case 'done':
      return 'finished';
    case 'idle':
      return 'ready';
    default:
      return 'unknown';
  }
}

export const GROUP_LABEL: Record<Group, string> = {
  attention: 'Needs you',
  working: 'Working',
  finished: 'Finished',
  ready: 'Ready',
  unknown: 'Unknown',
};

export const STATE_LABEL: Record<AgentState, string> = {
  blocked: 'Needs you',
  working: 'Working',
  done: 'Finished',
  idle: 'Ready',
  unknown: 'Unknown',
};
