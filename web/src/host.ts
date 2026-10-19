// The computer running Briardr: a Mac, or a (Linux) server. What it supports decides which controls
// exist; its name for itself is used in messages ("Can’t reach the server."). Until the server has
// said, Briardr assumes a Mac, which is what every earlier version ran on.
import { host } from './store.ts';
import type { HostCaps } from './types.ts';

const MAC: HostCaps = { platform: 'macos', noun: 'Mac', battery: true, keepAwake: true, sleep: true, dictation: true, claudeKeychain: true };

export function caps(): HostCaps {
  return { ...MAC, ...host.value };
}

/** "Mac" or "server". */
export function hostNoun(): string {
  return caps().noun;
}

/** "the Mac" or "the server". */
export function theHost(): string {
  return `the ${caps().noun}`;
}

/** "The Mac" or "The server". */
export function TheHost(): string {
  return `The ${caps().noun}`;
}

/** "Mac" or "Server", for labels. */
export function HostLabel(): string {
  const n = caps().noun;
  return n[0].toUpperCase() + n.slice(1);
}

export function isMac(): boolean {
  return caps().platform === 'macos';
}
