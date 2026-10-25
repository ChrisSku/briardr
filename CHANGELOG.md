# Changelog

Notable changes to Briardr, newest first. Each version's section becomes its
[GitHub release](https://github.com/ChrisSku/briardr/releases) notes. Versions follow
[Semantic Versioning](https://semver.org); before 1.0, a minor version can change setup or behavior, so
read its notes before upgrading.

## 0.3.0

- **Install Briardr with one command, and let it install Herdr.** `curl -fsSL https://briardr.dev/install.sh | sh`
  downloads the release for your Mac or Linux computer, checks it against the release's checksums, puts it
  in `~/.local/lib/briardr` and `briardr` in `~/.local/bin`; run it again to upgrade. It then runs the new
  `briardr setup`, which checks for Herdr and offers to install the version Briardr is tested with
  (checksum-verified, into `~/.local/bin`), leaves another installed version alone, reports Git, Tailscale
  and Swift, and says what's next. When Herdr is missing or not running, `briardr serve` and Settings → Herdr
  now say what to do. New: `briardr --version`, and `briardr install-power` for Always on with the lid
  closed (Settings and the pairing screen now name `briardr` commands instead of `mise run`). Herdr's pinned
  version and checksums moved to `deploy/versions.env`. ([#7](https://github.com/ChrisSku/briardr/pull/7))
- **Turning notifications on or repairing them no longer hangs when Briardr isn't saved on the device.**
  Notifications need the offline app's service worker; when it's still installing, the button now waits up
  to 10 seconds, and when it isn't installed it says so (with Settings → Diagnostics → Repair offline app),
  instead of staying disabled with nothing happening. ([#7](https://github.com/ChrisSku/briardr/pull/7))

## 0.2.0

- **Releases come with archives that run Briardr without a checkout.** Each GitHub release now has
  `briardr-<os>-<arch>.tar.gz` for macOS and Linux on aarch64 and x86_64, with a `SHA256SUMS` file: the
  server as `bin/briardr`, the web client next to it, and the third-party license notices. Unpack it and run
  `bin/briardr`; no Git checkout, Bun or Rust needed. The Linux archives need glibc 2.39 or newer (Ubuntu
  24.04 and later). Making and building plugins still needs a checkout of the same version. ([#5](https://github.com/ChrisSku/briardr/pull/5))
- **An update no longer reloads the page over a note this device couldn't keep a copy of.** When storage is
  full, text pasted or dictated into a note in one go now holds the update until the note is saved, as
  typing already did. ([#4](https://github.com/ChrisSku/briardr/pull/4))

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
