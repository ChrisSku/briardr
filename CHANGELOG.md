# Changelog

Notable changes to Briardr, newest first. Each version's section becomes its
[GitHub release](https://github.com/ChrisSku/briardr/releases) notes. Versions follow
[Semantic Versioning](https://semver.org); before 1.0, a minor version can change setup or behavior, so
read its notes before upgrading.

## 0.1.2

- **Sheets are no longer clipped in iOS Safari** when opened inside a scrolling screen; on Workspaces the
  tab bar covered New workspace's Create button. ([#1](https://github.com/ChrisSku/briardr/pull/1))
- **Text fields stay above the on-screen keyboard.** Sheets (Rename, Paste text, History, Ask about code,
  New workspace, project and stream) sit on top of the keyboard with their buttons in reach, Pairing's
  Sign in button stays visible, a focused field under the keyboard scrolls into view, and toasts show
  above it. ([#1](https://github.com/ChrisSku/briardr/pull/1))

## 0.1.1

- **A page reloads once for an update, not twice in a row.** When a new build arrived, a page you weren't
  using could hear about it from both the reconnect and the service worker, a moment apart, and start its
  reload over. ([#2](https://github.com/ChrisSku/briardr/pull/2))

## 0.1.0

The first public release: a self-hosted remote window into [Herdr](https://herdr.dev) for your phone,
tablet and desktop browser.

- **See who needs you.** Agents from every workspace grouped by Needs you, Working and Finished, with a
  badge on the app icon and Web Push alerts (end-to-end encrypted payloads) when one needs you.
- **Real terminals.** Peek at a live screen without resizing it, take control on purpose, answer
  approvals, send prompts, paste text, upload files and images, and use a touch key bar. Nothing you
  type is replayed after a reconnect.
- **Plugins.** Files, Changes (read-only Git review), Preview, Docs, Notes and Usage, each of which can
  be turned off. `briardr create plugin` hands writing your own to Claude Code or Codex.
- **Hosts.** macOS over Tailscale or an existing WireGuard VPN, and Ubuntu 24.04 or 26.04 LTS on x86_64
  and aarch64, where a setup helper installs public HTTPS through Caddy or a private WireGuard tunnel.
- **Clients.** An installable web app with an offline shell, phone, tablet and desktop layouts, and
  light and dark themes.

Briardr is in early development. Testing has focused on WebKit at iPhone, iPad and Mac sizes; Android
and other browsers need more testing, and iPhone Web Push over a WireGuard IP address with a private CA
isn't verified yet. Read the
[security policy](https://github.com/ChrisSku/briardr/blob/v0.1.0/SECURITY.md) before exposing a
deployment.

Install: [macOS](https://github.com/ChrisSku/briardr/blob/v0.1.0/docs/macos-setup.md) ·
[Ubuntu](https://github.com/ChrisSku/briardr/blob/v0.1.0/docs/linux-deployment.md)
