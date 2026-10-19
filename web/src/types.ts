export type AgentState = 'blocked' | 'working' | 'done' | 'idle' | 'unknown';
export type StreamKind = 'agent' | 'shell' | 'editor' | 'program';

export interface Workspace {
  id: string;
  name: string;
  number: number;
  focused: boolean;
  activeProjectId: string | null;
  cwd: string | null;
}

export interface Project {
  id: string;
  workspaceId: string;
  name: string;
  number: number;
  focused: boolean;
}

export interface Stream {
  id: string;
  workspaceId: string;
  projectId: string;
  terminalId: string;
  name: string;
  label: string | null;
  kind: StreamKind;
  tool: string;
  state: AgentState | null;
  stateChangeSeq: number | null;
  stateSince: number | null;
  cwd: string | null;
  folder: string | null;
  focused: boolean;
  title: string | null;
  rows: number | null;
}

export interface Model {
  workspaces: Workspace[];
  projects: Project[];
  streams: Stream[];
  focusedStreamId: string | null;
  herdrVersion: string | null;
  asOf: number;
}

export interface HerdrHealth {
  ok: boolean;
  problem: string | null;
  version: string | null;
  lastOkAt: number | null;
}

export interface Battery {
  present: boolean;
  percent: number | null;
  source: 'ac' | 'battery' | 'unknown';
  charging: boolean;
  charged: boolean;
  minutes: number | null;
  timeKind: 'remaining' | 'until_full' | 'calculating' | 'unavailable';
}

export interface MacSummary {
  name: string;
  battery: Battery;
  keepAwake: { wanted: boolean; briardr: boolean; lidClosed: boolean; setupNeeded: boolean; lowBattery: boolean; others: string[]; sleepDisabled: boolean };
  asOf: number;
}

/** What the computer running Briardr supports (server/src/platform.rs). */
export interface HostCaps {
  platform: 'macos' | 'linux' | 'other';
  noun: string;
  battery: boolean;
  keepAwake: boolean;
  sleep: boolean;
  dictation: boolean;
  claudeKeychain: boolean;
}

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
  attention: 'Needs your attention',
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
