# Set up Briardr on macOS

Run Briardr alongside Herdr as your normal Mac user. Your repositories and agent processes stay on
the Mac. Your phone, tablet, or other computer connects to Briardr over HTTPS.

## Before you start

- Pick a network path: Tailscale below, or [an existing WireGuard VPN](networking.md#wireguard-through-an-existing-lan).
- For Tailscale, connect the Mac and clients to the same tailnet. Ensure the `tailscale` CLI is on
  your PATH and `tailscale status --json` succeeds as the user who runs Briardr.

The core app does not require dictation. Optional on-device dictation needs macOS 26, the Swift
toolchain (`xcode-select --install`), and an available speech model for the selected language.

## Install

```sh
curl -fsSL https://briardr.dev/install.sh | sh
```

The installer downloads the latest release for your Mac (Apple silicon or Intel), checks it against
the release's `SHA256SUMS`, puts it in `~/.local/lib/briardr`, and links `briardr` into `~/.local/bin`;
add that folder to your PATH if it says so. Then it runs `briardr setup`, which checks for
[Herdr](https://herdr.dev) and, when it's missing, offers to install Herdr 0.9.5 into `~/.local/bin`.
Briardr is tested with Herdr 0.9.3 to 0.9.5. A Herdr you already have stays as it is, and setup warns
when it's another version: replacing Herdr can restart live agents. Setup also reports Git (for the
Changes tab), Tailscale and Swift, and says what's next. Run `briardr setup` again any time.

Herdr must be running for Briardr to show anything: start it in a terminal with `herdr`, and check it
with `herdr status`. With Herdr 0.9.4 or newer, its server and agents keep running when you close that
terminal; with Herdr 0.9.3, closing it can stop them, so keep it open or upgrade Herdr.

## Configure and start

For Tailscale HTTPS, enable MagicDNS and HTTPS certificates using
[Tailscale's instructions](https://tailscale.com/docs/how-to/set-up-https-certificates). The Briardr
user must be able to run `tailscale cert`; use Tailscale's documented permissions if it is refused.
Then:

```sh
briardr configure --mode tailscale --listen tailscale
briardr serve
```

Briardr starts on port 7443 and obtains a certificate for the Mac's tailnet DNS name. Without
Tailscale HTTPS, skip the `configure` step: Briardr then uses its own local CA, which each device has to
[trust once](devices.md#trust-briardrs-local-ca). Leave this terminal running. In another terminal:

```sh
briardr pair
```

Open the URL it prints. On an iPhone or iPad, [install the Home Screen app](devices.md#iphone-and-ipad)
before entering the code. Codes work once and expire after ten minutes.

If Herdr is installed outside PATH, configure its absolute path and restart Briardr:

```sh
briardr configure --herdr-path /absolute/path/to/herdr
```

## Start at login

Stop the foreground Briardr server with Ctrl-C, then:

```sh
briardr install-agent
```

This writes `~/Library/LaunchAgents/app.briardr.server.plist` and prints the `launchctl bootstrap`
command to start it. Run the printed command. It uses the current binary's path and captures your
PATH, so run it from a shell where `herdr` and `tailscale` work. Herdr must also be running; this
LaunchAgent starts Briardr only.

For status, pairing, and device management:

```sh
briardr status
briardr pair
briardr devices
briardr revoke DEVICE_ID
```

To stop the LaunchAgent, use
`launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/app.briardr.server.plist`.
The CLI's `uninstall-agent` command removes its plist and prints the stop command.

## Keep the Mac reachable

Settings → Mac power → **Always on** holds idle sleep with `caffeinate`. Keeping a closed laptop lid
from sleeping also needs the narrow sudoers rule installed by:

```sh
briardr install-power
```

Review [`scripts/install-power.sh`](../scripts/install-power.sh), which it runs, first: it requests sudo
and writes `/etc/sudoers.d/briardr-power` for `pmset disablesleep`. This is optional. Keep the laptop
ventilated and avoid running another keep-awake tool alongside it.

On battery, Briardr warns at 20%. At 10% it warns again, lets go of Always on, and schedules sleep after
30 seconds; plugging in cancels that sleep. Always on comes back once the battery is above 10%. Turning Always on off, quitting Briardr, or choosing Sleep Mac releases the
keep-awake setting. The Mac must be awake and online for remote terminals and alerts to work.

## Configuration, updates, and troubleshooting

Data is in `~/Library/Application Support/Briardr`, including `config.json`, owner-only `secrets/`,
`state/`, and `logs/`. Back up this directory privately; it includes keys and personal state.
See [configuration](configuration.md) for overrides.

Before updating, read the [release notes](https://github.com/ChrisSku/briardr/releases) and back up
your data. Then run the installer again: it replaces `~/.local/lib/briardr` with the latest release
(`BRIARDR_VERSION=0.2.0` picks one). If running as a LaunchAgent, restart Briardr with
`launchctl kickstart -k gui/$(id -u)/app.briardr.server`. Restarting Briardr leaves Herdr running.

## From source

To run a checkout instead (to work on Briardr, or to make plugins), install
[mise](https://mise.jdx.dev/getting-started.html), Git and Apple's Command Line Tools, then:

```sh
git clone --branch v0.3.5 https://github.com/ChrisSku/briardr.git
cd briardr
mise trust
mise install
mise run install
mise run build          # the web client, into web/dist
mise run build:server
mise run install-cli    # links ~/.local/bin/briardr to this checkout's build
```

`briardr` then runs the checkout's build, and the steps above are the same. To update, switch to the new release's tag and rebuild:
`git fetch --tags && git checkout vX.Y.Z && mise install && mise run install && mise run build && mise run build:server`.
See [development](development.md) for the rest.

- **No remote URL:** check `tailscale status --json`, tailnet membership, and `briardr status`.
- **Certificate failure:** check the TLS mode, Tailscale HTTPS settings, and the CLI's permissions.
  Access the tailnet DNS name printed by pairing, rather than its IP, for a Tailscale certificate.
- **Herdr unavailable:** `briardr setup` says what's wrong; check `herdr status`, `herdrPath`, and the
  configured Herdr session.
- **Cannot reach previews:** allow the client to reach ports 7444–7459 over the chosen private
  network, and ensure any local CA is trusted. See [networking](networking.md).
- **Alerts missing:** follow the [notification checks](devices.md#test-notifications).
