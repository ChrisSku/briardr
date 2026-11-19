// What Briardr's API answers, and what its live and terminal sockets send and take: made from the
// server's types by server/src/app/protocol.rs. Don't edit it: change those types and run
// `mise run protocol`.

/**
 * An agent a new stream can start.
 */
export type AgentChoice = {
  kind: string;
  name: string;
  /**
   * Whether it takes a first message when it starts.
   */
  prompt: boolean;
};

export type AgentInfo = {
  name: string | null;
  /**
   * The agent's kind.
   */
  agent?: string;
  status?: string;
  stateChangeSeq: number | null;
  /**
   * "present" when Herdr knows the agent's session, never the session itself.
   */
  session: string | null;
};

export type AgentState = "blocked" | "working" | "done" | "idle" | "unknown";

/**
 * `GET /api/agents`: what a new stream can start, and how long its prompt can be.
 */
export type Agents = {
  agents: Array<AgentChoice>;
  maxPrompt: number;
};

/**
 * A plugin's backend command, and whether the user approved this one (its hash) to run.
 */
export type BackendInfo = {
  command: Array<string>;
  approved: boolean;
  hash: string;
};

export type Battery = {
  present: boolean;
  percent: number | null;
  /**
   * Where its power comes from.
   */
  source: "ac" | "battery" | "unknown";
  charging: boolean;
  charged: boolean;
  /**
   * Minutes remaining (on battery) or until full (charging); None when unknown.
   */
  minutes: number | null;
  /**
   * What `minutes` counts.
   */
  timeKind: "remaining" | "until_full" | "calculating" | "unavailable";
};

/**
 * Why a stream ended (snake_case in the terminal's messages).
 */
export type CloseCategory = "busy" | "taken_over" | "released" | "pane_gone" | "exited" | "error";

/**
 * What a plugin adds to Briardr (`contributes` in its manifest).
 */
export type Contributes = {
  streamTabs: Array<Tab>;
  appTabs: Array<Tab>;
  settings: Array<Section>;
};

/**
 * `POST /api/projects`: the new project and its first stream.
 */
export type CreatedProject = {
  projectId?: string;
  streamId?: string;
  focusUnchanged: boolean;
};

/**
 * `POST /api/streams`: the new stream, and what to tell the person when its agent didn't start as asked.
 */
export type CreatedStream = {
  streamId?: string;
  note: string | null;
  focusUnchanged: boolean;
};

/**
 * `POST /api/workspaces`: the new workspace, its project and its first stream.
 */
export type CreatedWorkspace = {
  workspaceId?: string;
  projectId?: string;
  streamId?: string;
  /**
   * Whether the host's focused pane is still the one it was.
   */
  focusUnchanged: boolean;
};

/**
 * `GET /api/notifications/deliveries/:id`: the delivery, None when it isn't known.
 */
export type DeliveryBody = {
  delivery: DeliveryRecord | null;
};

export type DeliveryRecord = {
  id: string;
  kind: string;
  deviceId: string;
  sentAt: number;
  accepted: boolean | null;
  statusCode: number | null;
  deliveredAt: number | null;
  error: string | null;
};

export type DeviceEntry = {
  /**
   * The device asking.
   */
  current: boolean;
  /**
   * It gets alerts.
   */
  push: boolean;
  id: string;
  name: string;
  createdAt: number;
  lastSeenAt: number | null;
};

/**
 * A device's own choices for its alerts. A choice it didn't make follows the host's setting
 * (`notifications` in the configuration). Kept by device apart from its subscription, so they outlast
 * a repair, an expired subscription and alerts turned off and on, until the device is revoked.
 */
export type DevicePrefs = {
  needsYou?: boolean;
  finished?: boolean;
  ready?: boolean;
  lowBattery?: boolean;
  /**
   * Workspace and stream names in its alerts.
   */
  includeNames?: boolean;
  /**
   * Hours of the day when the device gets no alerts.
   */
  quiet?: QuietHours;
};

/**
 * A device as answers name it.
 */
export type DeviceRef = {
  id: string;
  name: string;
};

/**
 * `GET /api/devices`: every paired device, which of them this is, and which take alerts.
 */
export type Devices = {
  devices: Array<DeviceEntry>;
};

/**
 * `POST /api/dictation`: what was said.
 */
export type Dictated = {
  text: string;
};

/**
 * Which way a scroll goes (lowercase in the client's messages, as in Herdr's).
 */
export type Direction = "up" | "down";

/**
 * `{ok}`: whether it was done, from a route that has nothing else to say.
 */
export type Done = {
  ok: boolean;
};

/**
 * What a refused request says: its code and, for people, what went wrong.
 */
export type ErrorBody = {
  error: string;
  message?: string;
};

export type HerdrHealth = {
  /**
   * Which Herdr this is: "tui" (Herdr TUI) or "core" (herdr-core, under the Herdr app).
   */
  backend: "tui" | "core";
  ok: boolean;
  problem: string | null;
  version: string | null;
  lastOkAt: number | null;
};

/**
 * Whether the configured Herdr runs, its version, and what's wrong with it (`HerdrStatus::problem`).
 */
export type HerdrState = {
  running: boolean;
  version: string | null;
  problem: string | null;
};

/**
 * `GET /api/streams/:id/history`: the stream's last lines as text.
 */
export type History = {
  text: string;
  /**
   * More lines exist than were read.
   */
  truncated: boolean;
  lines: number;
  asOf: number;
};

/**
 * What the client sees as `host` (in /api/session, /api/mac, settings and the live `mac` message).
 */
export type HostCapabilities = {
  platform: HostPlatform;
  /**
   * "Mac" or "server".
   */
  noun: string;
  battery: boolean;
  keepAwake: boolean;
  sleep: boolean;
  dictation: boolean;
  claudeKeychain: boolean;
};

export type HostPlatform = "macos" | "linux" | "other";

/**
 * One `input`: the first of `text`, `keys`, `key` and `paste` that is valid is sent.
 */
export type InputRequest = {
  /**
   * Echoed in `input_ok` or `input_rejected`.
   */
  id?: number | null;
  text?: string | null;
  /**
   * Named keys with modifiers (`valid_key`).
   */
  keys?: Array<string> | null;
  /**
   * A key sent as its standard sequence (`raw_key`).
   */
  key?: string | null;
  paste?: PasteRequest | null;
};

/**
 * `POST /api/update/install`: the version installed, and whether Briardr restarts on it by itself.
 */
export type Installed = {
  ok: boolean;
  version: string;
  restarts: boolean;
};

export type JsonValue =
  | number
  | string
  | boolean
  | Array<JsonValue>
  | { [key in string]: JsonValue }
  | null;

export type KeepAwake = {
  /**
   * Always on is turned on in Settings.
   */
  wanted: boolean;
  /**
   * Briardr is holding the Mac awake right now (idle sleep).
   */
  briardr: boolean;
  /**
   * Briardr has disabled system sleep, so the Mac also stays awake with the lid closed.
   */
  lidClosed: boolean;
  /**
   * Always on is wanted but the sudoers rule for `pmset disablesleep` is missing.
   */
  setupNeeded: boolean;
  /**
   * Paused at 10% or less on battery.
   */
  lowBattery: boolean;
  others: Array<string>;
  sleepDisabled: boolean;
};

/**
 * What Briardr sends on /ws/live, by its `t`.
 */
export type LiveMessage =
  | { "t": "hello", instanceId: string }
  | { "t": "model" } & ModelState
  | { "t": "mac", mac: MacSummary, host: HostCapabilities }
  | { "t": "battery", level: number | null, percent: number | null, sleeping: boolean }
  | { "t": "transition" } & Transition
  | { "t": "plugins", plugins: Array<PluginInfo> }
  | { "t": "plugin_event", plugin: string, topic: string, data: JsonValue }
  | { "t": "notes", streamId: string, noteId: string, revision: number }
  | { "t": "pong", id?: number, now: number };

/**
 * What a client sends on /ws/live: only `ping`; anything else is ignored.
 */
export type LiveRequest = {
  "t": "ping";
  id?: number | null;
};

/**
 * `GET /api/mac` and `POST /api/power/keep-awake`: the power state and what the host can do.
 */
export type MacState = {
  mac: MacSummary;
  host: HostCapabilities;
};

export type MacSummary = {
  name: string;
  battery: Battery;
  keepAwake: KeepAwake;
  asOf: number;
};

export type Model = {
  workspaces: Array<Workspace>;
  projects: Array<Project>;
  streams: Array<Stream>;
  focusedStreamId: string | null;
  herdrVersion: string | null;
  /**
   * Server time when this model was read from Herdr.
   */
  asOf: number;
};

/**
 * The streams and Herdr's health, with the host's time and Briardr's version.
 */
export type ModelState = {
  model: Model | null;
  herdr: HerdrHealth;
  serverNow: number;
  version: string;
};

/**
 * What a mouse event is (lowercase in the client's messages, as in Herdr's).
 */
export type MouseAction = "down" | "up" | "drag" | "move";

/**
 * The mouse button of an event (lowercase in the client's messages, as in Herdr's).
 */
export type MouseButton = "left" | "right" | "middle";

export type NotificationsConfig = {
  includeNames: boolean;
  needsYou: boolean;
  finished: boolean;
  ready: boolean;
  lowBattery: boolean;
  debounceMs: number;
};

/**
 * `GET /api/notifications`: this device's subscription and alert choices, and the host's settings.
 */
export type NotificationsState = {
  publicKey: string;
  subscribed: boolean;
  endpoint: string | null;
  settings: NotificationsConfig;
  prefs: DevicePrefs;
};

/**
 * A coming change of the device's UTC offset.
 */
export type OffsetChange = {
  /**
   * When it changes, in ms since the epoch.
   */
  at: number;
  utcOffset: number;
};

/**
 * `POST /api/devices/pairing-code`: a one-time code, the link that opens Briardr with it filled in, and
 * that link as a QR code's rows of modules (`1` dark, `0` light), for the new device's camera.
 */
export type PairingCode = {
  code: string;
  expiresAt: number;
  /**
   * At the address this device reached the host by, or the host's own when this device is the host.
   */
  link: string;
  /**
   * None when the link is too long for a QR code.
   */
  qr: Array<string> | null;
};

export type PairingHost = {
  noun: string;
  platform: HostPlatform;
};

export type PasteRequest = {
  text?: string | null;
  /**
   * Wrapped as a bracketed paste when the program turned that on.
   */
  bracketed?: boolean | null;
};

/**
 * A plugin as the client lists it (`GET /api/plugins`, the session, the live `plugins` message).
 */
export type PluginInfo = {
  id: string;
  name: string;
  description: string;
  version: string;
  builtin: boolean;
  enabled: boolean;
  contributes: Contributes | null;
  backend: BackendInfo | null;
  /**
   * The client's URL, while the plugin is on and its build is ready.
   */
  client: string | null;
  /**
   * "ok", or why its client can't load: never built ("missing") or built for another Briardr ("stale").
   */
  build: "ok" | "missing" | "stale";
  problem: string | null;
};

/**
 * `GET /api/plugins` and `PUT /api/plugins/:id`: the plugins as they are now.
 */
export type Plugins = {
  plugins: Array<PluginInfo>;
};

export type PowerConfig = {
  keepAwake: boolean;
};

/**
 * `PUT /api/notifications/device`: the device's alert choices after the change.
 */
export type PrefsBody = {
  prefs: DevicePrefs;
};

export type Project = {
  id: string;
  workspaceId: string;
  name: string;
  number: number;
  focused: boolean;
};

export type PushPayload = {
  id: string;
  /**
   * "needs_you" | "finished" | "ready" | "battery" | "test"
   */
  kind: string;
  title: string;
  body: string;
  /**
   * Stream to open when tapped.
   */
  streamId?: string;
  tag: string;
  ts: number;
  /**
   * Agents that need attention when the alert was sent, for the app's icon badge.
   */
  attention?: number;
};

/**
 * A span of the device's day, in minutes after its midnight; `from` after `to` runs over midnight.
 * The host can't see the device's time zone, so the device says how far its clock is ahead of UTC and
 * when that changes next (summer time), and says it again when it opens Briardr after a change.
 */
export type QuietHours = {
  from: number;
  to: number;
  /**
   * Minutes the device's clock is ahead of UTC.
   */
  utcOffset: number;
  next?: OffsetChange;
};

/**
 * A section a plugin adds to Settings.
 */
export type Section = {
  id: string;
  label: string;
};

/**
 * `POST /api/streams/:id/seen`: whether every Herdr client shares what was seen (the Herdr app's
 * `herdr.seen` label), or only this client keeps it (Herdr TUI).
 */
export type Seen = {
  shared: boolean;
};

/**
 * `GET /api/session` and `POST /api/pair`: the same startup data for an existing session and a newly
 * paired device.
 */
export type Session = {
  device: DeviceRef;
  version: string;
  protocol: number;
  /**
   * Changes on every server start.
   */
  instanceId: string;
  host: HostCapabilities;
  plugins: Array<PluginInfo>;
};

/**
 * `GET /api/settings`.
 */
export type Settings = {
  /**
   * Which Herdr Briardr talks to: Herdr TUI's server, or herdr-core (the Herdr app's).
   */
  herdrBackend: "tui" | "core";
  herdrPath: string;
  herdrSession: string;
  herdrCoreSocket: string;
  herdr: HerdrState;
  notifications: NotificationsConfig;
  usage: UsageSettings;
  power: PowerConfig;
  tls: TlsModes;
  caProfileAvailable: boolean;
  previews: boolean;
  dictation: boolean;
  host: HostCapabilities;
  publicOrigin: string | null;
  version: string;
};

/**
 * `GET /api/session` from a device that isn't signed in: enough for the pairing screen's words.
 */
export type SignedOut = {
  error: string;
  paired: boolean;
  host: PairingHost;
};

/**
 * `POST /api/power/sleep`'s answer: the Mac goes to sleep (`duplicate` when this request ID already
 * put it to sleep, `at` then), or the request is refused with an `error`.
 */
export type SleepAnswer = {
  ok: boolean;
  duplicate?: boolean;
  at?: number;
  error?: string;
};

export type Stream = {
  id: string;
  workspaceId: string;
  projectId: string;
  terminalId: string;
  /**
   * Descriptive name: the user's stream name wins over everything else.
   */
  name: string;
  /**
   * The user-assigned stream name (Herdr pane label), if any.
   */
  label: string | null;
  kind: StreamKind;
  /**
   * Tool identifier: claude, codex, nvim, zsh, ...
   */
  tool: string;
  /**
   * Agent lifecycle state as reported by Herdr; None for shells, editors and programs.
   */
  state: AgentState | null;
  stateChangeSeq: number | null;
  /**
   * When Briardr observed the current state begin (server clock, ms).
   */
  stateSince: number | null;
  cwd: string | null;
  folder: string | null;
  focused: boolean;
  title: string | null;
  rows: number | null;
  /**
   * The machine its terminal runs on, as herdr-core names it; none for this computer. `cwd` is
   * only ever this computer's: a terminal elsewhere has none, so its files aren't looked for here.
   */
  machine?: string;
};

/**
 * `GET /api/streams/:id/info`: the stream, and what Herdr says about its agent.
 */
export type StreamInfo = {
  stream: Stream;
  agent: AgentInfo | null;
  asOf: number;
};

export type StreamKind = "agent" | "shell" | "editor" | "program";

/**
 * Whether a client controls the stream or watches it (lowercase in the terminal's messages).
 */
export type StreamMode = "observe" | "control";

/**
 * A tab a plugin adds to a stream or to the app.
 */
export type Tab = {
  id: string;
  label: string;
  /**
   * One of Briardr's icon names (web/src/ui/icons); unknown names show a generic icon.
   */
  icon?: string;
  /**
   * A stream tab that works only for a stream on this computer (it reads its folder, or serves
   * its localhost): hidden for a terminal on another machine.
   */
  local?: boolean;
};

/**
 * What Briardr sends on /ws/term, by its `t` (frames go as binary messages, `encode_frame`).
 */
export type TermMessage =
  | { "t": "hello", conn: string, mode: StreamMode, protocol: number, raw: boolean }
  | { "t": "live", mode: StreamMode, width: number, height: number }
  | { "t": "modes", mouse: boolean }
  | { "t": "warning", code: string }
  | { "t": "closed", category: CloseCategory, reason: string }
  | { "t": "input_ok", id?: number }
  | { "t": "input_rejected", id: number | null, reason: string }
  | { "t": "pong", id?: number };

/**
 * What a client sends on /ws/term, by its `t`. Anything else is ignored.
 */
export type TermRequest =
  | { "t": "ping", id?: number | null }
  | { "t": "release" }
  | { "t": "resize", cols: number, rows: number }
  | { "t": "scroll", id?: number | null, direction: Direction, column: number, row: number }
  | { "t": "mouse", id?: number | null, action: MouseAction, button: MouseButton, column: number, row: number, modifiers: number }
  | { "t": "input" } & InputRequest
  | { "t": "reply", data: string };

/**
 * `POST /api/notifications/test`: the test alert's delivery, when there was a subscription to send to.
 */
export type TestSent = {
  delivery?: DeliveryRecord;
};

/**
 * The TLS mode asked for, and the one in effect.
 */
export type TlsModes = {
  wanted: string;
  effective: string;
};

export type Transition = {
  streamId: string;
  terminalId: string;
  from: AgentState | null;
  to: AgentState | null;
  kind: StreamKind;
  tool: string;
  stateChangeSeq: number | null;
  at: number;
};

/**
 * `GET /api/update`: this version, the latest release the last check found, and how to update to it.
 */
export type UpdateStatus = {
  current: string;
  latest: string | null;
  /**
   * The latest is newer than this one.
   */
  available: boolean;
  checkedAt: number | null;
  /**
   * What went wrong at the last check.
   */
  error: string | null;
  /**
   * Whether Briardr checks once a day (`updates.check`).
   */
  check: boolean;
  /**
   * A release installation, which can update itself.
   */
  canInstall: boolean;
  how: string | null;
  /**
   * The command that updates another kind of installation.
   */
  command: string | null;
  /**
   * Whether a service manager starts Briardr again after an update.
   */
  restarts: boolean;
  installing: boolean;
  /**
   * The release notes of the newer version.
   */
  notes: string | null;
};

/**
 * `POST /api/streams/:id/uploads`: where the upload is on the host, as a path and quoted for a shell.
 */
export type Uploaded = {
  path: string;
  quoted: string;
  bytes: number;
};

export type UsageSettings = {
  claudeKeychainOptIn: boolean;
  codexEnabled: boolean;
};

export type Workspace = {
  id: string;
  name: string;
  number: number;
  focused: boolean;
  activeProjectId: string | null;
  cwd: string | null;
  /**
   * The machine it lives on, as herdr-core names it (`hc machine ls`); none for this computer.
   */
  machine?: string;
};
