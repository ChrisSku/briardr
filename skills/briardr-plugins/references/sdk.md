# @briardr/sdk reference

The source is `{{briardr}}/web/src/sdk/index.ts` (exports) and `{{briardr}}/web/src/plugins/types.ts`
(types). Read those when this page and the code disagree: the code wins.

Briardr's own Files, Changes, Preview, Docs, Notes and Usage are plugins built on exactly this API, with
nothing else from Briardr: `{{briardr}}/plugins/<id>/`. They're the best examples of everything below.

## Exports

| Export | What |
| --- | --- |
| `definePlugin(m)` | The client entry's default export: `{ streamTabs?, appTabs?, settings?, onTerminalLink? }`. |
| `Empty` | Empty or failure state: `<Empty title="No runs">Explanation.</Empty>` |
| `AsOf` | How old data is: `<AsOf at={msSinceEpoch} />` ("as of 2 min ago", in the host's clock); empty when undefined. |
| `Icon` | `<Icon name="refresh" size={20} label?="…" />`, names as in the manifest's `icon`. |
| `iconName(s)` | A valid icon name for any string (unknown → `plug`). |
| `Sheet` | A bottom sheet (iPhone) / dialog (iPad, Mac): `<Sheet title="…" onClose={…}>…</Sheet>`. |
| `StateBadge` | An agent state badge: `<StateBadge state={stream.state} />`. |
| `PullToRefresh` | `const ptr = new PullToRefresh(load)`, then `<div {...ptr.handlers}>`; `ptr.pull` is the distance. |
| `longPress(fn)` | Touch-and-hold (or right-click) handlers: `const lp = longPress(({ x, y }) => …)`, then `<div {...lp.handlers}>`. |
| `ago(elapsedMs)`, `duration(ms)` | "just now", "5 min ago"; "2 h 5 min". |
| `ApiError` | What failed requests throw: `message` (ready to show), `status`, `code`, `category` (`network`, `timeout`, `auth`, …), `data`. |
| `persisted(key, initial)` | Reactive state kept on this device (`localStorage`): `.value`, read and assign. Prefix keys with your plugin ID. |
| `state(initial)`, `derived(fn)` | Reactive state shared between modules (`.value`), for plain `.ts` files. |
| `keyboardInset()` | Call in a component: `.current` is the on-screen keyboard's height (0 when hidden), to keep an editor above it. |
| `prepareImage(blob, name)` | An image made ready to upload: orientation fixed, camera and location metadata dropped, PNG of at most 4096 px and 20 MB: `{ blob, width, height, name, url }`. |
| `isLocalhostUrl(url)` | True for `http://localhost:…`, `127.0.0.1` and the like (for `onTerminalLink`). |
| `renderMarkdown(text, { docPath, streamId })` | From `@briardr/sdk/markdown`: Briardr's Markdown renderer, safe for `{@html}` (raw HTML escaped, no remote images; relative images load from the stream's folder). Separate so it loads only with plugins that use it. |
| `API_VERSION` | `1`, the manifest's `"briardr"`. |
| Types | `Stream`, `StreamTabProps`, `AppTabProps`, `SettingsProps`, `PluginContext`, `StreamContext`, `TerminalAccess`, `PluginModule`, `MenuItem`, `HostCaps`, `AgentState`, `StreamKind`, `Settings`, `Save`. |

## Props

```ts
interface StreamTabProps { stream: Stream; visible: boolean; compact: boolean; briardr: StreamContext }
interface AppTabProps    { compact: boolean; briardr: PluginContext }
interface SettingsProps  { briardr: PluginContext }   // Briardr adds the section heading

interface Stream {
  id: string; workspaceId: string; projectId: string; terminalId: string;
  name: string; label: string | null;
  kind: 'agent' | 'shell' | 'editor' | 'program';
  tool: string;                                   // claude, codex, zsh, nvim, …
  state: 'blocked' | 'working' | 'done' | 'idle' | 'unknown' | null;   // agents only
  stateSince: number | null;
  cwd: string | null; folder: string | null;      // the pane's folder; folder is its short name
  focused: boolean; title: string | null; rows: number | null;
}
```

`stream` is reactive: it changes as Herdr reports (an agent going from working to done). Refresh with
`$effect(() => { void stream.state; if (visible) untrack(load) })`.

## briardr (PluginContext)

```ts
interface PluginContext {
  plugin: { id: string };
  api<T>(method, path, body?, opts?: { timeout?: number; signal?: AbortSignal }): Promise<T>; // your backend
  core<T>(method, path, body?, opts?): Promise<T>;                                          // Briardr's API
  upload<T>(path, file: Blob, opts?: { onProgress?(sent, total): void; signal?: AbortSignal }): Promise<T>; // a file to Briardr's API
  toast(text: string): void;
  confirm(o: { title: string; message?: string; confirm: string; destructive?: boolean }): Promise<boolean>;
  menu(items: MenuItem[], anchor: HTMLElement | { x: number; y: number }, title?: string): void; // MenuItem: { label, onSelect, icon?, disabled?, destructive?, hint? }
  navigate(to: { stream: string; tab?: string } | { app: string } | { settings: string } | { agents: true }): void;
  onEvent(topic: string, fn: (data: unknown) => void): () => void;   // your backend's events
  onCoreEvent(type: string, fn: (msg) => void): () => void;          // Briardr's own, e.g. 'notes' ({ streamId, noteId, revision })
  prefs: { get<T>(key: string, fallback: T): T; set(key: string, value: unknown): void }; // this device only
  host(): HostCaps & { the: string };   // platform: 'macos' | 'linux'; the: "the Mac" | "the server"
  connection(): 'connecting' | 'online' | 'offline' | 'stopped';     // reactive
  now(): number;                        // the host's clock in ms, ticks every second; reactive
  clockSkew(): number;                  // host clock minus this device's; reactive
  fontSize(): number;                   // the terminal's font size (px), for code views; reactive
  record(event: string, fields?: Record<string, number | boolean>): void; // Settings → Diagnostics log
}
interface StreamContext extends PluginContext {
  terminal: { readonly canSend: boolean; readonly generation: number; readonly maxBytes: number;
              send(text: string, o?: { generation?: number }): boolean };
  openTab(tab: string): void;           // "terminal" or a tab ID
}
```

Failed requests throw an `ApiError` whose `message` is ready to show (`briardr.toast(e.message)`); it
also has `status`, `code`, `category` and `data` (the response body). Show "Can't reach
${briardr.host().the}" style messages for `network` and `timeout`, and keep showing the last data.

`terminal.send` returns false (and sends nothing) when this device doesn't control the terminal, the
text is over `maxBytes` (128 KB), or, with `generation`, the terminal reconnected since you read it. Read
`briardr.terminal.generation` when the user starts composing and pass it to `send`, so a reconnect in
between never sends into a different session.

## Briardr's API through briardr.core

Read-only and stable enough for plugins. The paths take the stream's ID (URL-encoded). The ones marked
with a plugin answer `404 {"error":"plugin_disabled"}` while that built-in is off.

| Request | Answer |
| --- | --- |
| `GET /api/state` | `{ model: { streams, workspaces, projects, … }, herdr, … }`, everything Herdr reports |
| `GET /api/streams/<id>/changes` (changes) | Git status of the stream's folder: `{ state: 'ok' \| 'clean' \| 'not_repo' \| …, branch, files: [{ path, change, additions, deletions, … }] }` |
| `GET /api/streams/<id>/changes/diff?path=<file>` (changes) | That file's diff |
| `GET /api/streams/<id>/files?path=<dir>` (files) | `{ path, entries: [{ name, path, … }], total, truncated, asOf }` for one folder |
| `GET /api/streams/<id>/files/file?path=<file>` (files) | `{ text, size, … }` |
| `GET /api/streams/<id>/files/search?q=<text>` (files) | Matching file names |
| `GET /api/streams/<id>/docs` (docs) | Markdown files in the folder |

Writes (creating streams, renaming, notes) are Briardr's own; a plugin that needs to change something
does it in its backend, and only what the user asked for.

## Backend protocol

One JSON object per line. Briardr writes requests to the backend's stdin; the backend writes responses
and events to stdout, in any order (several requests can be in flight; match by `id`).

```jsonc
// Briardr → backend
{"type":"request","id":7,"method":"POST","path":"/runs/3","query":{"stream":"w1:p2"},
 "body":{"x":1},                // JSON bodies, parsed; null without a body
 "text":"…", "base64":"…",      // instead of body, for other content
 "stream":{…Stream…} | null,    // the stream named by ?stream=
 "device":{"id":"…","name":"My iPhone"}}

// backend → Briardr
{"type":"response","id":7,"status":200,"headers":{"cache-control":"no-store"},"body":{…}}
{"type":"response","id":8,"status":200,"text":"plain","headers":{"content-type":"text/csv"}}
{"type":"event","topic":"progress","data":{"done":3}}
```

- Allowed response headers: content-type, cache-control, content-disposition, etag, last-modified.
- Lines that aren't JSON are ignored; don't rely on that, use stderr for logs.
- Environment: `BRIARDR_PLUGIN_ID`, `BRIARDR_PLUGIN_DATA` (a folder for this plugin's data),
  `BRIARDR_API_VERSION`, and a PATH that includes Bun, mise and Homebrew. The working directory is the
  plugin folder.
- Requests time out after 30 s (the device sees 504). Exit when stdin closes.
- A backend that exits fails the request in flight; Briardr starts it again on the next request. After
  three quick crashes Briardr waits 30 s.
- Request bodies are at most 1 MB.
