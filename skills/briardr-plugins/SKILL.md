---
name: briardr-plugins
description: Build, change or debug a Briardr plugin — a folder with briardr-plugin.json that adds a tab to every stream, a tab of its own or a Settings section to Briardr (the remote window into Herdr), with an optional backend that runs on the host. Use whenever the working folder has a briardr-plugin.json, or the user asks to make or fix a Briardr plugin.
---

# Briardr plugins

Briardr shows the agents and terminals Herdr runs on a Mac or Linux server, on the user's iPhone, iPad
and Mac. A plugin adds to it:

- **stream tabs**: a tab in every stream, next to Terminal, Files and Changes (most plugins)
- **app tabs**: a tab of its own in the tab bar (iPhone) or sidebar (iPad, Mac), like Usage
- **settings**: a section in Settings

Its client is Svelte 5 (runes) built with Briardr's own toolchain and loaded by devices at runtime. An
optional **backend** is any program Briardr starts on the host and talks to over stdin/stdout.

The Briardr checkout is `{{briardr}}`; the command is `{{briardr_bin}}`.

## The loop

1. Edit `briardr-plugin.json`, `src/` and `server/`.
2. `{{briardr_bin}} plugin build .`: builds the client. Every device with Briardr open reloads the plugin
   within a second, and the backend starts again with the new code on its next request. A failed
   build prints the error and changes nothing.
3. `{{briardr_bin}} plugin check .`: type errors in the plugin's own files. Fix them all.
4. The backend's log: Briardr's data folder, `logs/plugins/<id>.log` (`~/Library/Application
   Support/Briardr` on a Mac, `~/.local/share/briardr` on Linux).

`{{briardr_bin}} plugin list` shows whether the plugin is on and built. Ask the user to look at it on a
device; you can't see their screen. To try a backend without a device, run it and type a request:
`echo '{"type":"request","id":1,"method":"GET","path":"/hello","query":{},"body":null,"stream":null,"device":{"id":"d","name":"test"}}' | bun server/main.ts`.

## Manifest: briardr-plugin.json

```json
{
  "id": "ci-status",
  "name": "CI",
  "version": "0.1.0",
  "description": "One sentence for Settings → Plugins.",
  "briardr": 1,
  "client": "src/index.ts",
  "backend": { "command": ["bun", "server/main.ts"] },
  "contributes": {
    "streamTabs": [{ "id": "ci", "label": "CI", "icon": "changes" }],
    "appTabs": [],
    "settings": [{ "id": "ci-settings", "label": "CI" }]
  }
}
```

- IDs: lowercase letters, digits and dashes, starting with a letter. The folder Briardr links is named
  after `id`; don't change `id` once installed (unlink and link again if you must).
- Tab and section IDs must not clash with other plugins (`files`, `changes`, `preview`, `docs`,
  `notes`, `usage` are taken) or Briardr's routes (`terminal`, `settings`, `agents`, …).
- `icon`: one of Briardr's icons: agents, bell, bolt, changes, check, clipboard, copy, device, docs,
  editor, file, folder, image, keyboard, link, mic, notes, plug, preview, program, refresh, search,
  settings, terminal, trash, usage, warning, wifi, workspaces. Anything else shows a plug.
- `backend.command` runs without a shell, in the plugin folder. **Changing it turns the plugin off
  until the user approves the new command** (`{{briardr_bin}} plugin enable <id>` asks them). Don't
  change it casually, and tell the user when you do.

## Client: src/index.ts

```ts
import { definePlugin } from '@briardr/sdk';
import Tab from './Tab.svelte';

export default definePlugin({
  streamTabs: { ci: Tab },        // keys are the IDs in "contributes"
  // appTabs: { … }, settings: { … },
  // onTerminalLink(url, stream, briardr) { return false }  // true = this plugin handled a tapped link
});
```

Imports: `@briardr/sdk`, `@briardr/sdk/markdown`, `svelte` and `svelte/*` are Briardr's own copies (never
install them). Other npm
packages work if you `bun add` them in the plugin folder; they're bundled into the plugin, so keep them
small (devices on mobile data load it).

The full SDK, with every type: [references/sdk.md](references/sdk.md). Briardr's own Files, Changes,
Preview, Docs, Notes and Usage are plugins made exactly like yours, with this SDK and nothing else from
Briardr: read the closest one in `{{briardr}}/plugins/<id>/` before you start. The essentials:

```svelte
<script lang="ts">
  import { AsOf, Empty, Icon, PullToRefresh, type StreamTabProps } from '@briardr/sdk';
  let { stream, visible, compact, briardr }: StreamTabProps = $props();
</script>

{#if visible}
  <div class="surface">…</div>
{/if}
```

- **Render only while `visible`.** Briardr mounts every stream tab when the stream opens and keeps them
  mounted while the user switches tabs. Load data when the tab becomes visible, not on mount.
- `stream`: `id`, `name`, `kind` (agent, shell, editor, program), `tool` (claude, codex, zsh, …),
  `state` (blocked, working, done, idle, unknown; agents only), `cwd` (the pane's folder), `folder`.
- `briardr.api(method, path, body?)`: your backend (`/api/plugins/<id><path>`). Add
  `?stream=${encodeURIComponent(stream.id)}` and the backend gets the stream.
- `briardr.core(method, path)`: Briardr's own read API, e.g. `/api/streams/<id>/changes` (Git changes),
  `/api/streams/<id>/files?path=src`, `/api/streams/<id>/docs`.
- `briardr.terminal.send(text)`: inserts text into this stream's terminal as one paste. **It never
  presses Enter**, and only works while this device controls the terminal (`briardr.terminal.canSend`;
  disable the button otherwise). Never try to submit for the user.
- `briardr.onEvent(topic, fn)` returns an unsubscribe: use it inside `$effect(() => briardr.onEvent(…))`.
- `briardr.toast(text)`, `briardr.confirm({title, message, confirm})`, `briardr.menu(items, anchorEl)`,
  `briardr.navigate({stream, tab} | {app} | {settings})`, `briardr.openTab('terminal')`,
  `briardr.prefs.get/set` (this device only), `briardr.host()` (`.the` is "the Mac" or "the server"),
  `briardr.connection()` ("online" or not; keep showing the last data while offline).

## Backend: server/main.ts

Briardr starts it on the plugin's first request, in the plugin folder, as the user, and stops it by
closing stdin. With Bun, `server/briardr.ts` does the protocol:

```ts
import { dataDir, emit, serve } from './briardr.ts';

serve(async (req) => {
  // req: { method, path, query, body (JSON) | text | base64, stream | null, device: {id, name} }
  if (req.method === 'GET' && req.path === '/runs') return { body: await runs(req.stream?.cwd) };
  return { status: 404, body: { error: 'not_found' } };
});
// Responses: { status?, headers?, body (JSON) | text | base64 }. emit(topic, data) reaches every device.
```

The raw protocol, for other languages: [references/sdk.md](references/sdk.md#backend-protocol).

- stdout is the protocol: log with `console.error` (`console.log` is redirected by `briardr.ts`).
- 30 seconds per request at most; longer work runs in the background and reports with `emit`.
- Keep data in `dataDir` (`$BRIARDR_PLUGIN_DATA`), not in the plugin folder.
- The backend runs with the user's full access. Briardr's rules still hold for it: never write to a Git
  repository unless the user asked for exactly that, never type into terminals behind Briardr's back,
  never send data anywhere the user didn't ask for, never log secrets or terminal content.

## Look and feel

Plugins sit inside Briardr and should look like it. Use its classes and variables, not your own palette:

- Stream tab: `<div class="surface">` → `<div class="surface-head"><div class="surface-title">
  <strong>Title</strong><span class="muted"> · detail</span><AsOf at={asOf} /></div>
  …icon buttons…</div>` → content. App tab: `<div class="screen"><header class="screen-head"><h1>`
  → `<div class="screen-body">`.
- Lists: `<ul class="rows"><li class="row"><button class="row-main"><span class="row-text">
  <span class="row-title">…</span><span class="row-sub">…</span></span></button></li></ul>`.
- Buttons: `btn`, `btn btn-primary`, `btn btn-small`, `icon-btn` (with `aria-label`) + `<Icon>`.
  Text: `hint`, `muted`, `error` (with `role="alert"`), `note`, `padded`; `tag` for small labels.
  Settings switches: `<label class="switch-row"><input type="checkbox" role="switch">…`.
- Empty and failure states: `<Empty title="…">explanation</Empty>`. Say what's true and what to do;
  never pretend data is live: show its age with `<AsOf at={ms}>`.
- Colors only through variables (`var(--fg)`, `--fg-2`, `--bg`, `--bg-2`, `--line`, `--accent`,
  `--danger`, `--add`, `--del`), so light and dark mode work. Scoped `<style>` in components is fine.
- It must work on an iPhone: one column, tap targets ≥ 44px (`--tap`), no hover-only controls,
  `compact` is true on narrow screens.

## When you're done

`{{briardr_bin}} plugin build .` and `{{briardr_bin}} plugin check .` pass, `{{briardr_bin}} plugin list`
shows the plugin on and built, and you've told the user what to look at on their device. Commit in the
plugin folder (it's its own Git repository).
