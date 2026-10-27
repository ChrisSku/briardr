# Plugins

Everything in a stream besides the terminal, and Settings → Usage, is a plugin: Files, Changes, Preview,
Docs, Notes and Usage are built in, and anyone can add their own. Plugins are turned on or off for
every device in Settings → Plugins (or with `briardr plugin enable|disable`).

## We use what we ship

The built-in plugins are in [`plugins/<id>/`](../plugins/), next to the template `briardr create
plugin` copies, and they're made the way anyone's plugin is: a `briardr-plugin.json` with a `client`
entry, Svelte 5 components behind `definePlugin`, and nothing from Briardr but `@briardr/sdk` (and
`@briardr/sdk/markdown`). They don't import the app's modules, and they get no props, context or
routes an installed plugin doesn't get: the `briardr` object, `briardr.core` for Briardr's API,
`briardr.terminal` for inserts.

So when a built-in needs something Briardr doesn't offer plugins yet (an upload, the connection state,
Briardr's live messages, the keyboard's height), it's added to the SDK (`web/src/sdk/index.ts`,
`web/src/plugins/types.ts`, `context.ts`) and to the skill's reference, and every plugin gets it.
What the built-ins can do is what plugins can do, and the SDK is used every day by the code that
needs it most.

`test/builtin-plugins.test.ts` holds this: every built-in's imports stay inside its folder or go to
Svelte, `@briardr/sdk` or an npm package, and each one also builds with `briardr plugin build`'s builder,
as an installed plugin would.

The only differences are where the code comes from and the defaults: built-ins are bundled with the
app (in chunks of their own, loaded when needed or in idle time), so they work on a first launch and
offline; they're on until turned off; their manifests are compiled into the server; and their server
side is Briardr's own API (`/api/streams/<id>/files`, `…/notes`, …) rather than a plugin backend.

To make one, run `briardr create plugin` on the computer that runs Briardr (from a checkout, after
`mise run install-cli` puts `briardr` on the PATH). It asks what the plugin should do, makes it in
`~/briardr-plugins/<id>`, installs and turns it on, installs the `briardr-plugins` skill for Claude Code
and Codex, and starts the agent there with the job. The skill (`skills/briardr-plugins/`) is the plugin author's guide; this page
is how plugins work inside Briardr.

Plugins are made and built with the checkout's tools (`scripts/plugin-build.ts`, the template, the skill
and Bun), which a [release archive](development.md#release-archives) doesn't have. When Briardr runs from
one, clone the same version (`git clone --branch vX.Y.Z https://github.com/ChrisSku/briardr.git`), run
`mise install && mise run install` in it, and run the plugin commands with `BRIARDR_PROJECT_ROOT` set to
that folder.

## What a plugin is

A folder with a manifest, `briardr-plugin.json`:

```json
{
  "id": "ci-status", "name": "CI", "version": "0.1.0", "description": "…", "briardr": 1,
  "client": "src/index.ts",
  "backend": { "command": ["bun", "server/main.ts"] },
  "contributes": {
    "streamTabs": [{ "id": "ci", "label": "CI", "icon": "changes" }],
    "appTabs": [], "settings": []
  }
}
```

- **Client**: Svelte 5 components behind `definePlugin({ streamTabs, appTabs, settings, onTerminalLink })`.
  Stream tabs get `{ stream, visible, compact, briardr }`; `briardr` (`web/src/plugins/context.ts`) is the
  plugin's way into Briardr: its backend, Briardr's API, toasts, menus, navigation, backend events,
  per-device prefs and, in a stream, the terminal.
- **Backend** (optional): any program, run on the host in the plugin folder.

Built-in plugins have the same shape in `plugins/<id>/`: their manifests are compiled into the server
too (`server/src/plugins/mod.rs`), and the app's build bundles their code with `@briardr/sdk` pointing
at its source (`scripts/build.ts`).

| Where | What |
| --- | --- |
| `plugins/<id>/` | The built-in plugins (`files`, `changes`, `preview`, `docs`, `notes`, `usage`) |
| `web/src/plugins/builtin.ts` | Their manifests and loaders, for the app |
| `web/src/plugins/registry.svelte.ts` | Which plugins are on (from the server, cached for offline launches), loading installed clients, what each adds |
| `web/src/plugins/types.ts`, `context.ts`, `web/src/sdk/` | The plugin API; `@briardr/sdk` is what every plugin imports |
| `server/src/plugins/mod.rs` | Manifests, discovery, validation, on/off state, what the client is told |
| `server/src/plugins/backend.rs` | Running backends |
| `server/src/plugins/cli.rs`, `create.rs` | `briardr plugin …`, `briardr create plugin`, `briardr skill`, `briardr install-cli` |
| `server/src/app/routes_plugins.rs` | `/api/plugins`, turning plugins on or off, the backend proxy |
| `scripts/plugin-build.ts`, `plugin-check.ts`, `sdk-modules.ts` | Building and type-checking installed plugins |
| `plugins/template/`, `skills/briardr-plugins/` | What `briardr create plugin` makes and installs |
| `test/builtin-plugins.test.ts` | Built-ins use only the SDK and build as installed plugins |

## Installed plugins

Installed plugins live in the data folder: `plugins/<id>`, usually a link to the folder the user works
in (`briardr plugin link <folder>`). Which plugins are on and which backend commands were approved is
in `state/plugins.json`, not `config.json`, so the command line can change it while the server runs.
The server re-reads the file when it changes and watches plugin folders once a second; any change is
sent to every device as `{"t":"plugins"}` on the live socket.

A plugin is **active** when it's turned on, its manifest is valid (ID format, no clashing IDs, plugin
API 1) and, if it has a backend, the current backend command is the one the user approved.

### Client

The app's build adds an entry point for each module it shares with plugins (`scripts/sdk-modules.ts`:
Svelte's runtime and `@briardr/sdk`) and writes stable files `/sdk/v1/<name>.js` that re-export this
build's copies. Svelte keeps state in module scope, so plugins must use the app's copy, not their own.

`briardr plugin build` (`scripts/plugin-build.ts`) compiles a plugin with Briardr's own Svelte and points
its `svelte…` and `@briardr/sdk` imports at `/sdk/v1/`. The result goes to `<plugin>/dist` with
`build.json` naming the runtime it was built for (`svelte@<version>+sdk1`). The server only serves a
build whose runtime matches the web client's (`web/dist/sdk/v1/runtime.json`), and `mise run build`
rebuilds installed plugins that don't match, so a Svelte update never runs a plugin compiled for another
version.

Devices load the client from `/plugins/<id>/<file>`: signed-in devices only, active plugins only, never
outside `dist/`. Built files have hashed names; the service worker keeps them for offline use. A
rebuild gives the entry a new name, the devices hear about it and import the new one: plugins reload
without a page reload.

Cost: Svelte's runtime can't be tree-shaken any more, about 20 KB (Brotli) more at startup.
`index.html` preloads the startup chunks so they load side by side.

### Backend

Started on the plugin's first request (`/api/plugins/<id>/<path>`), with the plugin folder as working
directory, `BRIARDR_PLUGIN_ID`, `BRIARDR_PLUGIN_DATA` (`plugin-data/<id>`) and a PATH that includes Bun,
mise and Homebrew. Requests and responses are newline-delimited JSON on stdin and stdout; the protocol
is in `skills/briardr-plugins/references/sdk.md`, and `plugins/template/server/briardr.ts` implements it
for Bun.

- Requests go through Briardr's usual checks (device cookie, Host, Origin, Sec-Fetch-Site, rate
  limits). The backend gets the method, path, query, body, the stream named by `?stream=` and the
  device's ID and name; never cookies or other headers.
- Responses may set content-type, cache-control, content-disposition, etag and last-modified; Briardr's
  security headers (CSP and the rest) always apply.
- `{"type":"event"}` lines go to every device's live socket as `plugin_event`.
- 30 s per request; a backend that exits fails the request in flight and starts again on the next; three
  quick crashes pause it for 30 s. stderr goes to `logs/plugins/<id>.log` (set aside at 1 MB).
- Turning the plugin off, a rebuild, a changed manifest and shutting Briardr down stop the process
  (stdin closes; the process is killed).

## Trust

A plugin is code the user chose to install, running with their access: its client runs in Briardr's
page, its backend on the host. Briardr doesn't sandbox it. What Briardr does:

- Installed plugins start off; turning one on is a deliberate act on a signed-in device or the host.
- A backend runs only after the user saw and approved its exact command (Settings shows it; `briardr
  plugin enable` asks). A changed command turns the plugin off until it's approved again.
- Devices reach plugins only through Briardr, after authentication. A disabled plugin's routes, files
  and backend are refused (`plugin_disabled`), for built-ins too.
- `briardr.terminal.send` is the same insert path as Briardr's own (one paste, never Enter, only while
  this device controls the terminal, refused across a reconnect with `generation`).
- The skill tells plugin authors (and their agents) Briardr's rules: no Git writes or terminal input
  behind the user's back, nothing sent anywhere unasked, no secrets or terminal content in logs.

## Tests

`test/builtin-plugins.test.ts` (built-ins use only the SDK, and build as installed plugins),
`test/plugins.test.ts` (on/off, refused routes, approval, client serving, backend proxy, events,
crashes), `test/create-plugin.test.ts` (`briardr create plugin` end to end with a stand-in agent),
`test-ui/plugins.test.ts` (Settings → Plugins on two devices; a real installed plugin with injected CSS,
shared reactivity, terminal insert, backend, events and hot reload), and the Rust unit tests in
`server/src/plugins/`. The test plugin is `test/fixtures/plugins/echo`.
