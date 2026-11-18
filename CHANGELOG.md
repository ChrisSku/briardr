# Changelog

Notable changes to Briardr, newest first. Each version's section becomes its
[GitHub release](https://github.com/ChrisSku/briardr/releases) notes. Versions follow
[Semantic Versioning](https://semver.org); before 1.0, a minor version can change setup or behavior, so
read its notes before upgrading.

## 0.3.27

- **The server's dependencies are updated past three RustSec advisories.** rustls-webpki 0.103.17 (RUSTSEC-2026-0382), and crypto-bigint 0.7.11 (RUSTSEC-2026-0387) and ctutils 0.4.8 (RUSTSEC-2026-0385), which the Web Push encryption uses. Nothing to change in your setup. ([#56](https://github.com/ChrisSku/briardr/pull/56))

## 0.3.26

- **An agent's row shows what it's working on.** In the Agents and Workspaces lists and in Jump, an agent's row has a line with its terminal title (Claude Code sets it to the task at hand), unless it only repeats the stream's name, the agent's own name or where it runs (a path, or a shell prompt's "user@host: ~/app"). Jump and the Workspaces search find agents by it too. With the Herdr app, a stream named after its terminal's title no longer starts with the agent's spinner. ([#46](https://github.com/ChrisSku/briardr/pull/46))
- **`briardr revoke` signs a device out while Briardr is running.** Until now a lost device revoked with
  `briardr revoke` on the host stayed signed in until Briardr restarted, and Briardr could write it back
  into its devices file a minute later, undoing the revoke. Now it works like revoking in Settings →
  Devices: the device's next request is refused, its open pages disconnect within a second, and its alerts
  stop. If you revoked a device with `briardr revoke` while Briardr was running, check `briardr devices`
  and revoke it again if it is still listed. ([#52](https://github.com/ChrisSku/briardr/pull/52))
- **An address with a stray `%` opens Agents instead of a blank page.** Briardr couldn't decode an address
  like `/stream/100%` and didn't start when it opened from its offline copy, with the host out of reach. ([#53](https://github.com/ChrisSku/briardr/pull/53))
- **Files can search inside files.** The search in Files finds files by name, as before, or, with Contents, the lines that contain what you type (as plain text, in any case). A line opens its file there, picked, ready to add to a prompt. The search reads text files up to 1 MB and, in a Git repository, leaves out what `.gitignore` names (dependencies, build output). ([#50](https://github.com/ChrisSku/briardr/pull/50))

## 0.3.25

- **Pair a device by scanning a QR code.** `briardr pair` (in a terminal) and Settings → Devices → Add a device show a QR code beside the one-time code. The new device's camera opens Briardr with the code filled in, and you still name the device and choose Sign in. The code travels after the link's `#`, which browsers send to no server or proxy, Briardr takes it out of the address once it has read it, and it still works once and expires after ten minutes. From the host itself (`localhost`), Settings shows the address `briardr pair` prints, which other devices can open. On an iPhone or iPad the pairing screen reminds you to add Briardr to the Home Screen first, since the Home Screen app signs in separately. ([#51](https://github.com/ChrisSku/briardr/pull/51))

## 0.3.24

- **Changes: comment on lines of a diff and send the comments to the agent.** Tap a line number in a diff (a second number makes a range), choose Comment and say what should change. The comments stay on this device, for each stream, until you send or clear them, and their lines stay marked while the file is unchanged. Review shows them with an optional note and sends them to the terminal as one paste; Briardr never presses Enter. Plugins get the same prompt sheet that Files and Changes use: `PromptSheet` and `codeBlock` in `@briardr/sdk`. ([#48](https://github.com/ChrisSku/briardr/pull/48))
- **Each device chooses its own alerts, and can have quiet hours.** In Settings → Notifications, the switches for Needs you, Finished, Ready for input, low battery and Include names now apply to the device you set them on; before, turning one off on a phone turned it off for every device. A device that never chose follows the host's `notifications` setting, so nothing changes until you touch a switch, and the host keeps a device's choices until it is revoked, through turning alerts off and on and repairs. "Quiet hours" keeps a span of the device's day free of alerts (22:00 to 07:00 unless you change it), on the device's own clock, summer time included; alerts in them aren't sent later. A device revoked with `briardr revoke` while Briardr wasn't running no longer gets alerts once Briardr starts again. ([#49](https://github.com/ChrisSku/briardr/pull/49))
- **Briardr can stay light or dark on a device, whatever its system says.** Choose System, Light or Dark in Settings → Appearance; System, which follows the device as before, is the default. The choice is kept on that device, the terminal and diagrams follow it, and Briardr opens in it without a flash of the other theme. ([#47](https://github.com/ChrisSku/briardr/pull/47))

## 0.3.23

- **Settings → Devices says when something fails.** A revoke, a sign-out or "Add a device" that the host couldn't carry out used to do nothing; now a message says so, and a list that can't be loaded offers "Try again". ([#45](https://github.com/ChrisSku/briardr/pull/45))
- **Send several images or files at once, and drop files on the terminal.** "Select image…" and "Select file…" take more than one, and with a mouse or trackpad files (not folders) can be dropped on a terminal you control. All of them are reviewed in one sheet (any can be removed), uploaded in order, and their paths inserted as one paste, never followed by Enter: for an agent each path on a line of its own, for a shell as arguments. ([#41](https://github.com/ChrisSku/briardr/pull/41))
- **Signed-in devices can be renamed.** Settings → Devices has a pencil beside each device; the device stays signed in under its new name. A name keeps at most 60 characters, without spaces around it or control characters, at pairing too. ([#45](https://github.com/ChrisSku/briardr/pull/45))

## 0.3.22

- **Workspace folders on a Linux host show the home folder as `~`.** Workspaces shortened only a Mac's `/Users/name`; `/home/name` and `/root` are shortened now too. ([#36](https://github.com/ChrisSku/briardr/pull/36))
- **rustls, the TLS library, is updated to 0.23.46.** It fixes four advisories (one moderate, three low) in how rustls handles a rejected Encrypted Client Hello, an unauthenticated ServerHello, the debug output of sessions, and compressed certificates. Briardr's own HTTPS listener and its requests to Web Push and usage services use rustls. ([#37](https://github.com/ChrisSku/briardr/pull/37))
- **Compose: write a message in a text field of your device, then insert it.** The key bar's + menu has "Compose…": a field with the device's autocorrection, editing and keyboard dictation (so dictation also works with a Linux host). Insert puts the message into the terminal as one paste, never followed by Enter. A message you don't insert is kept as the stream's draft, and the last messages you inserted are offered again under Recent; both stay on the device. ([#40](https://github.com/ChrisSku/briardr/pull/40))
- **A banner that shows again stays its full time.** When a stream needed you twice within a few seconds, or the low-battery banner changed to "going to sleep", the first banner's timer took the new one down early. ([#33](https://github.com/ChrisSku/briardr/pull/33))
- **A paste into Peek no longer lands in the stream that's open beside it.** On an iPad or a Mac, pasting while a Peek sheet had the focus put the text into the open stream's terminal, short single lines without review. Peek is read-only, so a paste into it now goes nowhere. ([#39](https://github.com/ChrisSku/briardr/pull/39))

## 0.3.21

- **On a touch screen, only the key bar's keyboard key opens and closes the on-screen keyboard.** Tapping
  the terminal no longer opens it, and swiping to scroll while it's up no longer closes it: the terminal
  scrolls and the keyboard stays. Where a hardware keyboard hides the key bar (on an iPad), a tap still
  puts the focus in the terminal. ([#32](https://github.com/ChrisSku/briardr/pull/32))

## 0.3.20

- **Briardr works with the Herdr desktop app too, through its herdr-core.** Choose **Herdr app** in
  Settings → Herdr (or `briardr configure --herdr-backend core`); **Herdr TUI** stays the default and
  switches back. With the Herdr app you watch, peek and type in its terminals, and Briardr encodes your
  keys, pastes and clicks the way the app's own terminal does. You rename, create and close its spaces and
  tabs as workspaces and projects. Opening a finished agent marks it seen in the app too, and terminals
  on machines the app added show up without their file tools. Briardr never starts a herdr-core: open the
  Herdr app first. Plugin stream tabs can say `"local": true` to hide themselves for terminals on another
  machine. ([#31](https://github.com/ChrisSku/briardr/pull/31))

## 0.3.19

- **The key bar comes back on an iPad when the on-screen keyboard opens.** A key press with no on-screen keyboard up counts as a hardware keyboard and hides the bar; opening the on-screen keyboard now shows it again. ([#30](https://github.com/ChrisSku/briardr/pull/30))

## 0.3.18

- **Files previews HTML pages.** Opening an `.html` file shows the page, with the stylesheets, scripts, images and other pages it links to by relative path; the eye button in the file's header switches to its source and back. The page runs in a sandboxed frame with no access to Briardr or other sites, and links to other sites open in a new tab. ([#29](https://github.com/ChrisSku/briardr/pull/29))
- **The stream header is one line, and the key bar is slimmer.** A stream's state shows as a dot and the colour of its name instead of the word "Working" or "Done", with the agent and place beside it. The header and the keys are shorter, and the key bar sits closer to the top of the keyboard. ([#29](https://github.com/ChrisSku/briardr/pull/29))

## 0.3.17

- **Files previews HTML pages.** Opening an `.html` file shows the page, with the stylesheets, scripts, images and other pages it links to by relative path; the eye button in the file's header switches to its source and back. The page runs in a sandboxed frame with no access to Briardr or other sites, and links to other sites open in a new tab. ([#28](https://github.com/ChrisSku/briardr/pull/28))

## 0.3.16

- **Briardr can have several windows open on a Mac.** New Window (in the sidebar, a stream's ⋯ menu, or ⌥⌘N)
  and Open in New Window on a stream open one, and each controls its own stream. A window title now names its
  stream, and changes one window saves, like seen markers and preferences, reach the others instead of being
  overwritten by them. A window opened at the start page goes to Agents if another window is using the last
  stream, and a second window on a stream watches it, as a second device does. ([#26](https://github.com/ChrisSku/briardr/pull/26))
- **The sidebar and the list beside a stream can be collapsed on iPad and Mac.** The button at the top of
  the sidebar (or ⌘B) shrinks it to its icons, and the button next to Back in a stream's header (or ⌥⌘B)
  hides the list, so the terminal gets the room. Both are remembered on the device. ([#24](https://github.com/ChrisSku/briardr/pull/24))

## 0.3.15

- **Clicks and drags in the terminal now reach the program, and dragging copies the selection.** A program
  that asked for the mouse, like a full-screen agent or editor, gets left and middle clicks, drags and
  movement, as in a real terminal; a shell sees nothing. Dragging or double-clicking copies the selected
  text when you let go, as in Herdr, and holding Shift keeps the drag from the program. ([#23](https://github.com/ChrisSku/briardr/pull/23))

## 0.3.14

- **Workspaces, projects and streams can be renamed and closed from Briardr.** Their menus in Workspaces now
  have Rename and Close, and a stream has them in its ⋯ menu anywhere. Closing asks first, because it stops
  what runs inside on the host. A project's button in the tree is now ⋯ instead of +; New stream is still its
  first item. ([#27](https://github.com/ChrisSku/briardr/pull/27))

## 0.3.13

- **The Briardr logo isn't under the MIT license any more.** The code stays MIT. From this version on the logo is all rights
  reserved, as `LOGO.md` says, so a fork or a modified build has to use a different one. Earlier versions keep the terms they
  shipped with. ([#25](https://github.com/ChrisSku/briardr/pull/25))

## 0.3.12

- **Docs and Notes now render the HTML in Markdown files, like the centred header of a README, and draw Mermaid diagrams.** Common tags such as `<p align="center">`, `<img>`, `<a>`, `<details>` and `<kbd>` work, in the same way as on GitHub, and a ` ```mermaid ` block becomes a diagram in the current theme. Anything that could run or reach out stays out: scripts, event handlers, styles and forms never get through, other tags show as text, remote images still aren't loaded and now show just their description (a badge reads "Latest release"), and a diagram can't change its security settings or link anywhere. A diagram Mermaid can't draw stays as code. ([#22](https://github.com/ChrisSku/briardr/pull/22))

## 0.3.11

- **A stream can fill the screen: Full screen in its ⋯ menu (or ⇧⌘↩) hides the header and tabs, and a button in the top right corner brings them back.** It works in the terminal and every tool (Files, Changes, Preview, Docs, Notes and installed plugins); on iPad and Mac it also hides the sidebar and the list beside the stream, and ⌘1–⌘9 still switch tools. Leaving the stream ends full screen. ([#21](https://github.com/ChrisSku/briardr/pull/21))

## 0.3.10

- **Keep the iPhone keyboard up when you tap the key bar's arrow pad, show the Rename sheet above the keyboard in the Home Screen app, and centre the icon on the sign-in screen.** A tap on the arrow pad no longer takes focus from the terminal and closes the on-screen keyboard. In Briardr installed on the Home Screen, sheets whose field sits low on the screen, like Rename stream, stayed hidden under the keyboard: iOS shrinks the page's reported height once it scrolls to such a field, and Briardr now measures the keyboard against the layout instead. The Briardr icon on the sign-in screen is centred. ([#19](https://github.com/ChrisSku/briardr/pull/19))

## 0.3.9

- **Files shows PDFs.** Opening a `.pdf` in the Files tab shows its pages, fitted to the pane, with zoom buttons in the header and the page count and size above them. PDFs up to 50 MB open; pages are drawn as they scroll into view. ([#18](https://github.com/ChrisSku/briardr/pull/18))

## 0.3.8

- **Scrolling with a Mac trackpad or mouse wheel scrolls the terminal again, instead of pressing Up and Down.** The terminal turned the wheel into arrow keys before Briardr saw it, which stepped through shell or prompt history; the wheel now scrolls through Herdr like a swipe on iPhone and iPad. ([#16](https://github.com/ChrisSku/briardr/pull/16))

## 0.3.7

- **Briardr checks for new releases and updates itself.** Once a day the host asks GitHub whether a newer
  release is out, and Settings → About shows it with its release notes and an **Update** button (the
  About row in Settings says so too). Updating downloads the release for the host, checks it against the
  release's checksums, replaces the installed folder and, when Briardr runs at login (the LaunchAgent) or
  under systemd, restarts on the new version; Herdr and your agents keep running and devices reconnect.
  On the host, `briardr update` does the same (`--check` only says whether there's a newer release).

  A checkout, the Linux helper's included, doesn't replace itself: Settings → About and `briardr update`
  show the command to run. Turn the daily check off with **Check for updates daily** in Settings → About,
  or `"updates": {"check": false}` in config.json. ([#15](https://github.com/ChrisSku/briardr/pull/15))

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
