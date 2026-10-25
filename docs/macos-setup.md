# Set up Briardr on macOS

Run Briardr alongside Herdr as your normal Mac user. Your repositories and agent processes stay on
the Mac. Your phone, tablet, or other computer connects to Briardr over HTTPS.

## Before you start

- Install [Herdr](https://herdr.dev/docs/install/) and start it. Check `herdr status`.
- Install [mise](https://mise.jdx.dev/getting-started.html), Git, and Apple's Command Line Tools
  (`xcode-select --install` if you do not already have them).
- Pick a network path: Tailscale below, or [an existing WireGuard VPN](networking.md#wireguard-through-an-existing-lan).
- For Tailscale, connect the Mac and clients to the same tailnet. Ensure the `tailscale` CLI is on
  your PATH and `tailscale status --json` succeeds as the user who runs Briardr.

The core app does not require dictation. Optional on-device dictation needs macOS 26, the Swift
toolchain, and an available speech model for the selected language.

## Build and configure

```sh
git clone --branch v0.2.0 https://github.com/ChrisSku/briardr.git
cd briardr
mise trust
mise install
mise run install
mise run build:server
```

For Tailscale HTTPS, enable MagicDNS and HTTPS certificates using
[Tailscale's instructions](https://tailscale.com/docs/how-to/set-up-https-certificates). The Briardr
user must be able to run `tailscale cert`; use Tailscale's documented permissions if it is refused.
Then:

```sh
./server/target/release/briardr configure --mode tailscale --listen tailscale
mise run serve
```

Briardr builds the web app, starts on port 7443, and obtains a certificate for the Mac's tailnet DNS
name. Without Tailscale HTTPS, skip the `configure` step: Briardr then uses its own local CA, which
each device has to [trust once](devices.md#trust-briardrs-local-ca). Leave this terminal running. In another terminal in the same checkout:

```sh
mise run pair
```

Open the URL it prints. On an iPhone or iPad, [install the Home Screen app](devices.md#iphone-and-ipad)
before entering the code. Codes work once and expire after ten minutes.

If Herdr is installed outside PATH, configure its absolute path and restart Briardr:

```sh
./server/target/release/briardr configure --herdr-path /absolute/path/to/herdr
```

## Start at login

Stop the foreground Briardr server with Ctrl-C, then:

```sh
mise run install-agent
```

This writes `~/Library/LaunchAgents/app.briardr.server.plist` and prints the `launchctl bootstrap`
command to start it. Run the printed command. It uses the current binary's path and captures your
PATH, so keep the checkout in a permanent location and install from a shell where `herdr` and
`tailscale` work. Herdr must also be running; this LaunchAgent starts Briardr only.

For status, pairing, and device management:

```sh
./server/target/release/briardr status
./server/target/release/briardr pair
./server/target/release/briardr devices
./server/target/release/briardr revoke DEVICE_ID
```

To stop the LaunchAgent, use
`launchctl bootout gui/$(id -u) ~/Library/LaunchAgents/app.briardr.server.plist`.
The CLI's `uninstall-agent` command removes its plist and prints the stop command.

## Keep the Mac reachable

Settings → Mac power → **Always on** holds idle sleep with `caffeinate`. Keeping a closed laptop lid
from sleeping also needs the narrow sudoers rule installed by:

```sh
mise run install-power
```

Review [`scripts/install-power.sh`](../scripts/install-power.sh) before running it: it requests sudo
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
your data. With a clean checkout, switch to the new release's tag and rebuild:

```sh
git fetch --tags
git checkout v0.2.0   # the release you want
mise install && mise run install && mise run build && mise run build:server
```

If running as a LaunchAgent, restart Briardr with
`launchctl kickstart -k gui/$(id -u)/app.briardr.server`. Restarting Briardr leaves Herdr running.

- **No remote URL:** check `tailscale status --json`, tailnet membership, and `briardr status`.
- **Certificate failure:** check the TLS mode, Tailscale HTTPS settings, and the CLI's permissions.
  Access the tailnet DNS name printed by pairing, rather than its IP, for a Tailscale certificate.
- **Herdr unavailable:** check `herdr status`, `herdrPath`, and the configured Herdr session.
- **Cannot reach previews:** allow the client to reach ports 7444–7459 over the chosen private
  network, and ensure any local CA is trusted. See [networking](networking.md).
- **Alerts missing:** follow the [notification checks](devices.md#test-notifications).
