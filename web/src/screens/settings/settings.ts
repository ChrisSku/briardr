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

export type Save = (patch: object) => Promise<boolean>;
