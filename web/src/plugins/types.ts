// What a plugin is to the client: its manifest and the components it adds. Built-in plugins
// (plugins/<id>/ in this repository) and installed ones use the same shapes, through @briardr/sdk.
import type { Component } from 'svelte';
import type { Save, Settings } from '../screens/settings/settings.ts';
import type { HostCaps, Stream } from '../types.ts';
import type { MenuItem } from '../ui/overlays.ts';

export interface TabInfo {
  id: string;
  label: string;
  /** One of Briardr's icon names (ui/icons); unknown names show a generic icon. */
  icon?: string;
}

export interface SectionInfo {
  id: string;
  label: string;
}

export interface Contributes {
  streamTabs: TabInfo[];
  appTabs: TabInfo[];
  settings: SectionInfo[];
}

/** A plugin as the server lists it (server/src/plugins/mod.rs `info_json`). */
export interface PluginInfo {
  id: string;
  name: string;
  description: string;
  version: string;
  builtin: boolean;
  enabled: boolean;
  contributes: Partial<Contributes> | null;
  backend: { command: string[]; approved: boolean; hash: string } | null;
  /** Where the client loads from; null for built-ins, and while it isn't built for this Briardr. */
  client?: string | null;
  build?: 'ok' | 'missing' | 'stale';
  problem: string | null;
}

export type Navigation = { stream: string; tab?: string } | { app: string } | { settings: string } | { agents: true };

/** What every plugin component gets as `briardr`. */
export interface PluginContext {
  readonly plugin: { id: string };
  /** A request to this plugin's backend: `api('GET', '/items')` goes to /api/plugins/<id>/items. */
  api<T = any>(method: string, path: string, body?: unknown, opts?: { timeout?: number; signal?: AbortSignal }): Promise<T>;
  /** A request to Briardr's own API, e.g. `core('GET', '/api/streams/<id>/changes')`. Fails with an `ApiError`. */
  core<T = any>(method: string, path: string, body?: unknown, opts?: { timeout?: number; signal?: AbortSignal }): Promise<T>;
  /** Sends a file to Briardr's own API (POST, the file is the body), with progress. Fails with an `ApiError`. */
  upload<T = any>(path: string, file: Blob, opts?: { onProgress?: (sent: number, total: number) => void; signal?: AbortSignal }): Promise<T>;
  toast(text: string): void;
  confirm(opts: { title: string; message?: string; confirm: string; destructive?: boolean }): Promise<boolean>;
  /** A menu at an element, or at a point (a long press or right click). */
  menu(items: MenuItem[], anchor: HTMLElement | { x: number; y: number }, title?: string): void;
  navigate(to: Navigation): void;
  /** Messages the plugin's backend pushes (`{"type":"event","topic",...}`); returns an unsubscribe. */
  onEvent(topic: string, fn: (data: unknown) => void): () => void;
  /** Briardr's own live messages of one type (`t`), e.g. `notes` when a stream's notes change; returns an unsubscribe. */
  onCoreEvent(type: string, fn: (msg: Record<string, any>) => void): () => void;
  /** Whether this device reaches Briardr: "online", "connecting", "offline" or "stopped" (signed out). Reactive. */
  connection(): 'connecting' | 'online' | 'offline' | 'stopped';
  /** The time on the computer running Briardr, in ms; updates every second. Reactive. */
  now(): number;
  /** How far that clock is ahead of this device's (`now() - Date.now()`, give or take a second). Reactive. */
  clockSkew(): number;
  /** The terminal's font size in px, which code and text views can follow. Reactive. */
  fontSize(): number;
  /** Adds an event to the client log in Settings → Diagnostics. Names and keys are `snake_case`; only
   *  numbers and booleans are kept as they are (text is redacted), so nothing personal is recorded. */
  record(event: string, fields?: Record<string, number | boolean>): void;
  /** Small values kept on this device only, separate for each plugin. */
  prefs: { get<T>(key: string, fallback: T): T; set(key: string, value: unknown): void };
  /** What the computer running Briardr supports, and what to call it ("the Mac", "the server"). */
  host(): HostCaps & { the: string };
}

export interface TerminalAccess {
  /** True while this device controls the stream's terminal. Reactive. */
  readonly canSend: boolean;
  /** Changes when the terminal reconnects; pass it to `send` to refuse sending into a new connection. */
  readonly generation: number;
  /** Inserts text as one paste and never presses Enter. False when it wasn't sent. */
  send(text: string, opts?: { generation?: number }): boolean;
  /** The most `send` takes, in UTF-8 bytes. */
  readonly maxBytes: number;
}

export interface StreamContext extends PluginContext {
  readonly terminal: TerminalAccess;
  /** Shows one of this stream's tabs ("terminal" or a plugin's tab ID). */
  openTab(tab: string): void;
}

export interface StreamTabProps {
  stream: Stream;
  visible: boolean;
  compact: boolean;
  briardr: StreamContext;
}

export interface AppTabProps {
  compact: boolean;
  briardr: PluginContext;
}

export interface SettingsProps {
  briardr: PluginContext;
  /** Briardr's settings and a way to change them (every plugin gets them, as the built-in Usage does). */
  s: Settings | null;
  save: Save;
}

export interface PluginModule {
  streamTabs?: Record<string, Component<StreamTabProps>>;
  appTabs?: Record<string, Component<AppTabProps>>;
  settings?: Record<string, Component<SettingsProps>>;
  /** A link tapped in a stream's terminal. Return true when the plugin took it. */
  onTerminalLink?(url: string, stream: Stream, briardr: StreamContext): boolean;
}

export function definePlugin(m: PluginModule): PluginModule {
  return m;
}
