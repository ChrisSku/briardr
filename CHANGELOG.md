# Changelog

Notable changes to Briardr, newest first. Each version's section becomes its
[GitHub release](https://github.com/ChrisSku/briardr/releases) notes. Versions follow
[Semantic Versioning](https://semver.org); before 1.0, a minor version can change setup or behavior, so
read its notes before upgrading.

## 0.3.6

- **A new look for the client: quiet chrome, loud state.** Warm neutral colours in light and dark, with
  saturated colour kept for what agents are doing: orange when one needs you, blue while working, green
  when finished. Buttons, links and the selected tab are ink instead of blue.

  - The iPhone tab bar has three tabs: Agents, Workspaces and Settings. On iPad and Mac the sidebar lists
    Agents and Workspaces, with Jump and a Settings gear at the bottom.
  - Usage is no longer a tab: it's a page in Settings (Settings → Usage), and `/usage` opens it there.
  - Agents shows what needs you in its own card, with every row; the other groups show their first three
    rows and Show all. The Mac summary moved to the top of Settings. On iPhone, swipe a row to the left
    for Peek and More (a long press still opens the menu).
  - A stream no longer shows that you're in control. When another device has control, a Watching banner
    with Take over appears under the header and the keys dim.
  - Settings is a grouped list; each section opens as its own page.
  - The key bar keeps its keys and sizes; only its colours change. ([#14](https://github.com/ChrisSku/briardr/pull/14))

## 0.3.5

- **Choose the key bar's keys in Settings, with a row of their own for an app, and keep the on-screen keyboard up when a tap lands between keys.** Settings › Key bar picks the top row of the key bar from symbols, Control chords, Tab, Shift-Tab, Home, End, Page Up, Page Down, Forward Delete and F1–F12, up to 10 keys. That row is the default; give an app (Claude, Codex, a shell, Neovim, or any program running in a pane) keys of its own and the bar shows them instead while it runs. The choice is stored on each device. A tap in a gap between the bar's keys no longer takes focus from the terminal and closes the iOS keyboard, and while the keyboard is open the bar sits closer to it. ([#13](https://github.com/ChrisSku/briardr/pull/13))

## 0.3.4

- **The on-screen keyboard resizes the terminal instead of pushing its top out of view.** While you type
  in a stream you control, the pane takes the rows above the keyboard, so a full-screen program such as
  Claude Code lays itself out to fit: its input box and what's above it stay in view. The pane gets its
  full size back when the keyboard closes; the program redraws each time. Watching never resizes the pane. ([#12](https://github.com/ChrisSku/briardr/pull/12))

## 0.3.3

- **Note edits, settings changes and device sessions are preserved reliably.** Delayed note refreshes no
  longer discard newer edits, concurrent settings saves keep each other's changes, and failed device
  revocation reports an error. Newly paired devices load the current plugins and follow changes made on
  other devices without a reload. ([#11](https://github.com/ChrisSku/briardr/pull/11))

## 0.3.2

- **Opening the on-screen keyboard no longer pushes a shell prompt out of view.** The terminal moves up only
  as far as its content needs: the last line with text and the cursor stay above the keyboard. What you
  type at a prompt near the top of the screen stays visible, and an agent's input box at the bottom
  still sits on the key bar. ([#3](https://github.com/ChrisSku/briardr/pull/3))

## 0.3.1

- **`briardr setup` and the Linux helper install Herdr 0.9.5, and Briardr is tested with Herdr 0.9.3 to 0.9.5.** Setup no longer warns about a Herdr you already have when it is one of these versions. Upgrading isn't required, but newer Herdr helps Briardr: from 0.9.4, closing the terminal you started Herdr in no longer stops its agents, and Codex no longer stays unknown after a turn; 0.9.5 also shows Claude Code questions in custom-agent sessions (`claude --agent`) as needing you. From 0.9.4, the key bar's ⌘ reaches only programs that read it (those using the kitty keyboard protocol), and no longer types stray characters into plain shells. Replacing Herdr restarts the agents in it, so upgrade when none are working. ([#9](https://github.com/ChrisSku/briardr/pull/9))
- **Tapping terminal text where an OSC 8 link used to be no longer opens that old link.** On a phone or tablet the tap opened it, and the text's menu (a long press, or a right-click on the Mac) offered Open Link and Copy Link for it; it now gets Copy Line and Select Text, like any other text. ([#9](https://github.com/ChrisSku/briardr/pull/9))

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
