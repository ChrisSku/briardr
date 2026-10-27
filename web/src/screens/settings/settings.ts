// Settings as the host reports them, and how sections save changes.
import type { HostCaps } from '../../types.ts';
export interface Settings {
  herdrPath: string;
  herdrSession: string;
  herdr: { running: boolean; version: string | null; problem: string | null };
  notifications: { includeNames: boolean; needsYou: boolean; finished: boolean; ready: boolean; lowBattery: boolean };
  usage: { claudeKeychainOptIn: boolean; codexEnabled: boolean };
  power: { keepAwake: boolean };
  tls: { wanted: string; effective: string };
  caProfileAvailable: boolean;
  dictation: boolean;
  previews: boolean;
  host: HostCaps;
  publicOrigin: string | null;
  version: string;
}

/** GET /api/update: what the host's last check for a newer release found. */
export interface UpdateStatus {
  current: string;
  latest: string | null;
  available: boolean;
  checkedAt: number | null;
  error: string | null;
  /** The daily check (`updates.check`). */
  check: boolean;
  /** A release installation, which replaces itself; otherwise `how` and `command` say what to run. */
  canInstall: boolean;
  how: string | null;
  command: string | null;
  /** launchd or systemd starts Briardr again after an update. */
  restarts: boolean;
  installing: boolean;
  notes: string | null;
}

export type Save = (patch: object) => Promise<boolean>;
