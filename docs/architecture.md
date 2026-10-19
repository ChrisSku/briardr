# Architecture

Briardr serves an installable web client and connects to a local Herdr session. One instance targets
one Herdr session; it is not currently a fleet dashboard. Herdr owns the processes and terminals;
Briardr provides remote views and actions around them.

## Repository map

| Path | Responsibility |
| --- | --- |
| `server/` | Rust binary: HTTPS, WebSockets, authentication, Herdr bridge, Git/files/docs, notes, previews, usage, notifications, and host integrations. |
| `server/src/herdr/` | Local control-socket requests, normalized state, events, and terminal sessions. |
| `server/src/terminal_hub.rs` | Per-device terminal ownership, observation, control, and release. |
| `web/src/screens/` | Svelte app screens, stream tools, settings, and responsive navigation. |
| `web/src/terminal/` | Browser terminal controller, input rules, keys, and links. |
| `web/src/sw.ts` | Offline shell, versioned assets, and push handling. Live operations need a connection. |
| `web/src/sdk/`, `web/src/plugins/` | `@briardr/sdk`, the only API plugins get, and the client side of loading them. |
| `plugins/` | Built-in plugins (Files, Changes, Preview, Docs, Notes, Usage), made like any installed plugin; `plugins/template` is what `briardr create plugin` starts from. |
| `server/src/plugins/` | Installed plugins: manifests, enablement, files, and backends (approved commands speaking JSON over stdin/stdout). |
| `native/transcribe.swift` | Optional macOS on-device speech transcription helper. |
| `scripts/` | Bun build tooling, Linux installation, deployment smoke tests, and optional Mac power setup. |
| `deploy/linux/` | Installer version and checksum pins. |
| `test/`, `test-ui/`, `server/tests/` | Integration, WebKit UI, and Rust fixture tests. Rust unit tests also live beside implementation modules. |
| `bench/` | Performance and comparison tooling. |
| `site/` | The [briardr.dev](https://briardr.dev) website, published by the Pages workflow. |

The Rust binary is built to `server/target/release/briardr`. The browser build is served from
`web/dist`, so a binary copied by itself is not a complete installation. The Linux installer keeps
its Cargo build outside the checkout and configures the service with the right project paths.

## Boundaries to preserve

| Rule | Implementation | Verification |
| --- | --- | --- |
| Never queue/replay terminal input while connecting or disconnected. | `server/src/terminal_hub.rs`, `web/src/terminal/controller.ts` | `test/terminal.test.ts`, `test-ui/interactions.test.ts` |
| No silent takeover; Peek never resizes; leaving/backgrounding releases control. | Terminal hub, `server/src/herdr/stream.rs`, `StreamScreen.svelte` | Terminal and interaction suites |
| Git review uses read-only commands and does not take an index lock. | `server/src/git.rs` | Rust Git unit tests, `test/surfaces.test.ts` |
| Private APIs and WebSockets require a paired device; Host, Origin, and peer checks apply. | `server/src/app.rs`, `auth.rs`, `net.rs` | `test/auth.test.ts`, `test/linux.test.ts` |
| Public proxy mode trusts configured proxies and isolates preview origins. | `server/src/app.rs`, `net.rs`, `preview.rs` | `test/linux.test.ts` |
| macOS-only features are hidden and refused on Linux. | `server/src/platform.rs`, `web/src/host.ts` | `test/linux.test.ts`, `test-ui/linux.test.ts` |
| Diagnostics use an allowlist to keep terminal text, names, addresses, and secrets out of logs. | `server/src/diag.rs`, `web/src/diag.ts` | `test/diagnostics.test.ts` |
| Reading usage never starts an agent turn. | `server/src/usage/` | Rust Codex tests |
| Plugins are off until turned on; disabled ones are hidden and refused; a backend runs only with its approved command; plugin files and backends only for paired devices. | `server/src/plugins/`, `server/src/app/routes_plugins.rs` | `test/plugins.test.ts`, `test-ui/plugins.test.ts` |
| Built-in plugins use only `@briardr/sdk` and build as installed plugins. | `plugins/<id>/`, `web/src/sdk/` | `test/builtin-plugins.test.ts` |

The Changes tool being read-only does not make Briardr read-only: terminals and file deletion can
change the host. Device pairing is the trust boundary, not a separate sandbox. See
[the security policy](../SECURITY.md) and [Linux security model](linux-deployment.md#security-model).

## External services

No Briardr account or central relay is required. Tailscale deployments use Tailscale's network
coordination and certificate services; public HTTPS uses your DNS and Caddy's configured CA. Web
Push uses the browser vendor's push service with encrypted payloads. Optional usage sources contact
their providers. The macOS speech helper transcribes on the host.
