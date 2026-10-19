# Networking and HTTPS

Briardr supports Tailscale, WireGuard, and public HTTPS behind a local reverse proxy. Device pairing
is required with every access path. HTTPS is needed for installation, clipboard, microphone, and
notification features.

| Access | Host setup | Client setup | TLS |
| --- | --- | --- | --- |
| Tailscale | macOS or Linux, existing tailnet | Join the same tailnet; permit access in its policy | Tailscale certificate, or Briardr local CA |
| WireGuard to Linux | Ubuntu helper manages the tunnel | Import a generated WireGuard configuration | Briardr local CA |
| WireGuard through a LAN | Existing VPN router and routing | Connect VPN; route the host's LAN address | Briardr local CA or supplied certificates |
| Public HTTPS | Ubuntu helper with Caddy and your domain | Browser; no VPN | Caddy / Let's Encrypt |

Loopback stays available. The main port defaults to 7443; direct-mode preview ports default to
7444–7459. Only expose these to your intended private network. With a public proxy, backend ports
stay on loopback; expose TCP 80/443 through Caddy instead.

## Tailscale on macOS or Linux

Install Tailscale separately, connect the host and clients to the same tailnet, and ensure
`tailscale status --json` succeeds as Briardr's user. Permit the main port and preview ports in your
tailnet's access policy. Enable MagicDNS and
[HTTPS certificates](https://tailscale.com/docs/how-to/set-up-https-certificates). Certificate names
appear in public certificate transparency logs; choose your machine and tailnet names accordingly.

From a built checkout:

```sh
./server/target/release/briardr configure --mode tailscale --listen tailscale
./server/target/release/briardr serve
```

Briardr invokes `tailscale cert` as its own user and renews certificates. That command must work
without an interactive sudo prompt. Connect using the tailnet DNS name and port 7443 printed by
`briardr pair`, and pair every client device.

For a Mac, use the [macOS guide](macos-setup.md) for installation and startup. For Linux, install
Herdr, Git, a C compiler/toolchain, and mise; then run `mise trust`, `mise install`,
`mise run install`, `mise run build`, and `mise run build:server`. Run Briardr and Herdr as the same
non-root user. To generate service files for a manual deployment:

```sh
./server/target/release/briardr systemd-units /tmp/briardr-units
```

Review the generated units, their absolute executable paths, user, HOME, and PATH before installing
them in `/etc/systemd/system` and enabling them. The Briardr service user needs access to the
Tailscale CLI and certificate issuance. Manual Linux Tailscale service setup has not received the
same deployment testing as the helper's public and WireGuard modes. The helper does not manage
Tailscale; rerunning its `install` command applies its selected public or WireGuard configuration.

If you prefer your own CA, choose `--mode local-ca` instead. Install and trust the CA on each
client as described in [device setup](devices.md#trust-briardrs-local-ca).

## WireGuard to an Ubuntu server

Use [the Linux WireGuard guide](linux-deployment.md#wireguard-access-private-no-domain). The helper
manages `wg0`, the private network, firewall guard, services, and peer enrollment:

```sh
scripts/setup-linux.sh install --access wireguard
scripts/setup-linux.sh wg-add phone
scripts/setup-linux.sh pair
```

The default private URL is `https://10.8.0.1:7443`. A tunnel key permits network access; a separate
Briardr pairing code signs the device in. Removing a device means removing its WireGuard peer
**and** revoking it in Briardr Settings → Devices.

## WireGuard through an existing LAN

For a VPN that routes clients into the host's LAN, Briardr does not create the tunnel or configure
your router. Reserve a stable LAN address for the host, route that address through WireGuard, and
permit forwarding from the tunnel to the LAN on the router.

For example, for a host at `192.168.1.50`, with client source addresses on `192.168.1.0/24`:

```sh
./server/target/release/briardr configure \
  --mode local-ca \
  --listen 192.168.1.50 \
  --allowed-networks 192.168.1.0/24 \
  --public-origin https://192.168.1.50:7443
```

Replace the example addresses with your own. If the router preserves the clients' tunnel addresses,
use the tunnel subnet in `--allowed-networks`, or a comma-separated list of both networks if needed.
This is a source-address policy, not a setting for the VPN's `AllowedIPs`. Restrict it to the networks
that should reach Briardr. Explicit listeners must be IPv4 addresses assigned to the host; `0.0.0.0`
is refused.

Restart Briardr, connect the VPN on your client, install and trust its CA, and pair at the configured
URL. Explicit listener addresses are added to the local certificate and Host allowlist. Previews
follow the same network policy, on their own ports.

## Public HTTPS

Follow [Linux public deployment](linux-deployment.md#public-access) for DNS, Caddy, firewall rules,
trusted proxies, pairing limits, and isolated preview subdomains. Proxy mode serves plain HTTP only
on loopback. Do not forward Briardr's direct port onto the internet.

## Certificates and changes of address

- Tailscale certificates cover the tailnet DNS name; use that name in the browser.
- Briardr's local CA is per installation. Distribute only `ca.crt` or the CA profile, never `ca.key`
  or another private key. Trust it only on devices you administer.
- In `files` mode, `tls.certFile` and `tls.keyFile` in `config.json` select your PEM certificate and
  key; maintain their validity and renewal yourself.
- Pairing cookies, Home Screen apps, notification subscriptions, and browser data belong to an
  origin (scheme, host, and port). Changing the URL can require pairing and installing again.
