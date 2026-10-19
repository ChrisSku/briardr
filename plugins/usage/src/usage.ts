// Usage reports as the Mac sends them, and this device's copy of the last ones.
import { persisted } from '@briardr/sdk';

export interface Win {
  id: string;
  label: string;
  usedPercent: number | null;
  resetsAt: number | null;
}
export interface Report {
  provider: 'claude' | 'codex';
  source: string;
  windows: Win[];
  credits?: { hasCredits: boolean; unlimited: boolean; balance: string | null } | null;
  resetCredits?: { available: number; credits?: { title: string | null; description: string | null; grantedAt: number | null; expiresAt: number | null }[] | null } | null;
  spend?: { usedUsd: number | null; limitUsd: number | null; period: string | null } | null;
  plan?: string | null;
  limitReached?: string | null;
  capturedAt: number;
}
export interface Status {
  report: Report | null;
  tapReport?: Report | null;
  tapInstalled?: boolean;
  lastError: { category: string; message: string; at: number } | null;
  lastAttemptAt: number | null;
  cooldownUntil: number | null;
  rateLimitedUntil: number | null;
  enabled: boolean;
}
export type Usage = { claude: Status; codex: Status };

// Last reports are kept on this device too, so they survive relaunches while offline.
export const cached = persisted<Usage | null>('briardr.cache.usage', null);
export const REFRESH_MS = 3 * 60 * 1000;

/** Local clock time of a Mac-clock timestamp (`skew`: briardr.clockSkew()): "16:40", or "Tue 09:00" when it isn't today. */
export function clockAt(at: number, skew: number): string {
  const d = new Date(at - skew);
  const time = d.toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' });
  return d.toDateString() === new Date().toDateString() ? time : `${d.toLocaleDateString([], { weekday: 'short' })} ${time}`;
}
