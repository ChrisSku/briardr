# Briardr on a Linux server

See the [documentation index](README.md) for macOS and client device setup. For an existing Tailscale
tailnet, see the [manual Tailscale path](networking.md#tailscale-on-macos-or-linux); the helper below
manages public HTTPS and WireGuard deployments.

Run Briardr, Herdr, your repositories and agent CLIs on an always-on Linux server, and use Briardr from an
iPhone (Safari or the Home Screen app) over mobile data or Wi-Fi. The server offers Briardr one of two
ways (its *access mode*; one per server, switchable):

| | **Public** (`install --domain D`) | **WireGuard** (`install --access wireguard`) |
| --- | --- | --- |
| Phones open | `https://briardr.example.com` | `https://10.8.0.1:7443`, through the tunnel |
| Needs | a domain and DNS records | nothing but the server's static IP |
| Open ports | TCP 80, 443 (and SSH) | UDP 51820 (and SSH) |
| HTTPS | Let's Encrypt, via Caddy | Briardr's own CA, installed on each phone |
| Who can reach the pairing screen | anyone on the internet | only devices with a WireGuard key you issued |
| On the phone | nothing extra | the WireGuard app (On-Demand) and Briardr's CA profile |
| Third parties | Let's Encrypt, your DNS host | none (no Tailscale, no coordination service) |

Public access is described first; [WireGuard access](#wireguard-access-private-no-domain) has its own
section below. Everything else (Herdr, agents, backups, upgrades) is the same for both. Public access:

```text
Phone / Home Screen app
  → https://briardr.example.com (443; 80 only for certificates and redirects)
  → Caddy: Let's Encrypt certificate, WebSockets, request limits
  → Briardr on 127.0.0.1:7443 (plain HTTP, loopback only, "proxy" mode)
  → Herdr's socket (~/.config/herdr) and terminal sessions
  → repositories, agent CLIs and dev servers, all as one non-root user

Previews: https://p1.briardr.example.com … p16 → Caddy → 127.0.0.1:7444 … 7459 → your dev server
```

**Supported:** Ubuntu 24.04 LTS and 26.04 LTS on x86_64 or aarch64, with systemd. Verified on fresh
Ubuntu 26.04.1 and 24.04.5 VMs (aarch64) with Caddy's internal CA: install and the smoke test below on
both; rerun, upgrade, rollback, a failed upgrade, backup/restore and a reboot on 26.04. WireGuard
access: install, rerun, phones added and removed, both smoke tests, the guard, upgrade/rollback,
backup/restore, ufw, switching modes both ways and a reboot on 26.04.1; a fresh install, the smoke tests
and a reboot on 24.04.5, each with a network namespace playing the phone. Not yet run on x86_64 hardware,
with a real Let's Encrypt certificate, or with a real iPhone over WireGuard. Other distributions need
adaptation.

## Public access

### Before you start

- **A server.** 2 vCPUs and 8 GB RAM is a starting point; agents and builds decide the real size, so
  measure them before settling. On AWS Lightsail, pick Ubuntu LTS in a region near you. See
  [provider notes](#provider-notes) for Lightsail, EC2 and Azure.
- **A static public IPv4 address** attached to the server.
- **A domain** you control. Create:
  - `A briardr.example.com → <static IP>`
  - `A *.briardr.example.com → <static IP>` (previews; or `p1` … `p16` individually)
  - an `AAAA` record only if the server really has public IPv6; a stale one breaks phones on IPv6 networks.
- **Firewall:** inbound TCP 80 and 443 from anywhere; SSH only from where you administer. Nothing else.
  Briardr's port and dev servers stay on loopback and must not be opened.
- **A non-root user** for Briardr, Herdr and the agents, with sudo for the setup (e.g. `briardr`).
- Never put keys or tokens in the repository, in chat, or in Briardr's config.

### Install

As the Briardr user:

```sh
git clone --branch v0.3.10 https://github.com/ChrisSku/briardr.git ~/briardr
cd ~/briardr
scripts/setup-linux.sh check --domain briardr.example.com     # read-only: what's there, what's missing
scripts/setup-linux.sh install --domain briardr.example.com --agents claude,codex
```

`install` installs what's missing and configures everything; run it again any time (it changes only what
differs, and keeps paired devices, notes, keys and agent sign-ins):

| Step | What it does |
| --- | --- |
| Preflight | OS, CPU, systemd, sudo, user, disk, ports 80/443, DNS vs. public IP, existing services and versions. Stops on blockers. |
| Packages | `git curl openssl jq unzip gnupg ca-certificates` via apt, if missing. |
| Caddy | From Caddy's apt repository; if that isn't reachable, the release `.deb` pinned in `deploy/versions.env` (checksum-verified). |
| mise, Bun, Rust | mise per user (`~/.local/bin`), Bun and Rust at the versions pinned in `mise.toml`, `build-essential` for the C compiler, the server build (`cargo build --release --locked` into `~/.local/state/briardr-setup/cargo-target/release/briardr`, outside the checkout), `bun install --frozen-lockfile`, client build. Never changes a pin or a lockfile. |
| Herdr | The version pinned in `deploy/versions.env` (0.9.5) from Herdr's GitHub release, checksum-verified, into `~/.local/bin`. An existing Herdr is left alone; one that isn't a tested version (0.9.3 to 0.9.5) is reported. |
| Agent CLIs | `--agents claude,codex`: Claude Code's native installer; Codex with npm (Node from mise). It never signs them in. |
| Briardr config | `briardr configure --mode proxy --public-origin https://D --preview-origin https://p{n}.D --herdr-path …`, validated first; the previous file is kept as `config.json.bak`. |
| systemd | `briardr-herdr.service` and `briardr.service`, generated with absolute paths, explicit HOME/PATH, the same user; verified with `systemd-analyze` before they're installed. |
| Caddy site | `/etc/caddy/briardr.caddy` (from `briardr caddyfile`), imported by `/etc/caddy/Caddyfile`. Other sites are kept; the package's placeholder site is replaced. Validated before reload; restored if invalid. |
| Firewall | If ufw is active: allow 80 and 443 (SSH rules untouched). The provider's firewall is yours to set. |
| Verify | Briardr on loopback, HTTPS through Caddy with a certificate that verifies, the WebSocket route, and the public address. |

Every system file it replaces is backed up under `~/.local/state/briardr-setup/backups/<time>/`.
On a failure it stops, says which step failed, and leaves later steps untouched; fix the cause and rerun.

Then sign in to the agents as the Briardr user (`claude`, `codex` once each in a terminal on the server) and
pair the phone:

```sh
scripts/setup-linux.sh pair
```

Open the URL on the iPhone, choose Share → Add to Home Screen, and open the installed app before
entering the code. The Home Screen app may use a separate sign-in from Safari. Notifications on
iPhone work only from the Home Screen app (Settings → Notifications in Briardr). See [device setup](devices.md).

## WireGuard access (private, no domain)

Nothing of Briardr is on the internet: phones reach it only through a WireGuard tunnel to the server,
run by the server itself (the kernel's WireGuard and `wg-quick`; no Tailscale, no coordination service).

```text
iPhone: WireGuard app, On-Demand on cellular and Wi-Fi, AllowedIPs 10.8.0.0/24 (split tunnel)
  → UDP 51820 on the server's static public IP (the only open port besides SSH)
  → wg0 on the server, 10.8.0.1/24
  → Briardr on https://10.8.0.1:7443 (Briardr's own CA; the certificate covers the IP)
  → Herdr, repositories, agent CLIs; previews on https://10.8.0.1:7444 … 7459
```

**Choose it when** you'd rather not have a pairing screen on the internet, don't want a domain, or
don't want Let's Encrypt and DNS involved. The cost: each phone needs the WireGuard app and Briardr's CA
profile, and you hand out tunnel configs yourself.

**Security model:** two layers. A WireGuard key (with a preshared key) gets a device onto the tunnel;
pairing with a one-time code gets it into Briardr. Only UDP 51820 is open, and WireGuard answers nothing
to packets without a valid key, so the server doesn't show up in port scans. Briardr listens only on
`10.8.0.1` and loopback and accepts only the tunnel network (`allowedNetworks: ["10.8.0.0/24"]`); an
nftables table (`inet briardr_wg`, loaded by wg0's `PostUp`) drops packets for the tunnel network that
arrive on any other interface (Linux would otherwise accept them: the weak host model) and lets phones
on wg0 reach only Briardr's ports, not SSH or other services. Phones can't reach each other (no
forwarding). Every Briardr rule from [Security model](#security-model) still applies.

### Before you start

- **A server** as for public access, with a **static public IPv4 address**. No domain, no DNS.
- **Firewall:** inbound UDP 51820 from anywhere, SSH only from where you administer, nothing else. On
  Lightsail: *Networking → IPv4 firewall → Add rule → Custom, UDP, 51820*. Close 80/443 if they were
  opened for public access.
- **A non-root user**, as for public access.

### Install

```sh
git clone --branch v0.3.10 https://github.com/ChrisSku/briardr.git ~/briardr
cd ~/briardr
scripts/setup-linux.sh check --access wireguard
scripts/setup-linux.sh install --access wireguard --agents claude,codex
```

Options: `--wg-port N` (default 51820), `--wg-subnet CIDR` (default `10.8.0.0/24`, a private /16 to /29;
the server is its first address), `--endpoint HOST` (what phones dial; default the server's public IPv4
as seen from the internet). The steps are those of public access without Caddy, plus:

| Step | What it does |
| --- | --- |
| Packages | Also `wireguard-tools`, `qrencode`, `nftables`. Preflight checks the kernel module, that UDP 51820 is free, and that the tunnel network doesn't overlap one the server uses. |
| Briardr config | `briardr configure --mode local-ca --listen 10.8.0.1 --allowed-networks 10.8.0.0/24 --public-origin https://10.8.0.1:7443`. Other settings are kept. |
| WireGuard | `/etc/wireguard/wg0.conf` (root only) with a server key generated **once**: reruns never replace it, so phones keep working. `wg-quick@wg0` enabled and started; changes are validated with `wg-quick strip` and applied with `wg syncconf`, so connected phones stay connected. The previous file is kept in `/etc/wireguard/briardr-backups/`. |
| Guard | `/etc/wireguard/briardr-wg0.nft`, loaded with wg0. |
| systemd | `briardr.service` starts after `wg-quick@wg0` and pulls it in (Briardr listens on its address). |
| Caddy | Switching from public access: Briardr's site and its `import` line are removed (backed up); if Caddy serves nothing else, it's stopped and disabled. |
| Firewall | If ufw is active: allow UDP 51820, and Briardr's ports on wg0 only. |
| Verify | wg0 up, guard loaded, Briardr answering on `https://10.8.0.1:7443` with a certificate from its CA, the WebSocket route, the CA profile, and nothing on the server's other addresses. |

`wg0.conf` belongs to the setup (its first line says so): don't edit it by hand. A `wg0.conf` the setup
didn't write stops the install instead of being changed.

### Add a phone

```sh
scripts/setup-linux.sh wg-add iphone
```

It picks the next free address (10.8.0.2, .3, …), generates the phone's key pair and a preshared key,
adds the phone to wg0 at once, and shows the phone's config **once**, as a QR code and as text. The
phone's private key isn't kept on the server; if it's lost, `wg-remove` and `wg-add` again. The config:

```ini
[Interface]
PrivateKey = …
Address = 10.8.0.2/32

[Peer]
PublicKey = <server>
PresharedKey = …
Endpoint = <static IP>:51820
AllowedIPs = 10.8.0.0/24
PersistentKeepalive = 25
```

No `DNS` line and only the tunnel network in `AllowedIPs`: the phone's other traffic doesn't go through
the server. On the iPhone:

1. Install **WireGuard** from the App Store; **+ → Create from QR code**; scan. In the tunnel's settings
   turn on **On-Demand** for **Cellular** and **Wi-Fi**, then turn the tunnel on.
2. In Safari, open `https://10.8.0.1:7443/api/ca.mobileconfig` (accept the certificate warning this
   once) and install the profile: **Settings → General → VPN & Device Management → Briardr → Install**.
3. **Settings → General → About → Certificate Trust Settings**: full trust for **Briardr Local CA**.
4. Open `https://10.8.0.1:7443` in Safari, choose **Share → Add to Home Screen**, and open that app.
5. Run `scripts/setup-linux.sh pair` on the server and enter the code inside the installed app.
   Notifications work from the Home Screen app; enable them in Briardr Settings.

Clear the terminal's scrollback afterwards: it holds the only copy of the phone's key.

**Not yet verified on an iPhone** (2026-10-11): the Home Screen app and Web Push on an IP-address origin
with a private CA. What is known: in the iOS Simulator, Safari treats such an origin as secure and
Briardr's service worker registers and controls the page; Apple's push service accepts
`https://10.8.0.1:7443` as the VAPID subject. If iOS refuses push or the Home Screen app there, the
fallback is a host name that resolves to 10.8.0.1 (a DNS record on any domain you own), added to
Briardr's `hostnames` so the local CA issues for it.

### Manage phones

```sh
scripts/setup-linux.sh wg-list           # names, addresses, last handshake, traffic
scripts/setup-linux.sh wg-remove iphone  # off the tunnel immediately; the others stay connected
```

`wg-remove` only removes the tunnel; also revoke the phone's Briardr device in Settings → Devices (or
`briardr devices` / `briardr revoke`). Briardr's origin never changes, so if the server's public IP changes,
only the `Endpoint` in each phone's WireGuard config changes: pairing and the Home Screen app stay valid.

### Switching access modes

`install --access wireguard` on a public server, or `install --access public` (with the saved domain)
on a WireGuard server, switches it. Devices, notes, keys and Herdr are kept, but **Briardr's URL
changes**, and a phone's sign-in and Home Screen app belong to a URL: pair each phone again, add it to
the Home Screen again, and delete the old app. After switching to public access, wg0 keeps running
(Briardr no longer listens on it); stop it with `sudo systemctl disable --now wg-quick@wg0` if nothing else
uses it. The WireGuard key and phones are kept for switching back.

### Backups and a new server

`backup` adds the phones' names, addresses and public keys and the guard rules, but deliberately **no
WireGuard private keys** (neither the server's nor the preshared keys). Restoring on the same server
leaves `/etc/wireguard` alone. On a new server, `install --access wireguard` creates a new server key,
so each phone needs a new config: `wg-add` it again (and edit nothing else: Briardr's URL is the same).
Disk snapshots (Lightsail, EBS, Azure) keep `/etc/wireguard` and avoid that.

## Day to day

```sh
scripts/setup-linux.sh status            # services, versions, URL
scripts/setup-linux.sh restart           # Briardr only; Herdr and the agents keep running
scripts/setup-linux.sh upgrade [REF]     # fast-forward (or REF), rebuild, redeploy, verify; rolls back by itself on failure
scripts/setup-linux.sh rollback          # back to the commit before the last upgrade
scripts/setup-linux.sh backup [FILE]     # ~/briardr-backups/briardr-<time>.tar.gz (mode 600)
scripts/setup-linux.sh restore FILE      # current data moved aside, archive restored, Briardr restarted
scripts/setup-linux.sh diagnose          # phone can't connect, certificate problems
scripts/setup-linux.sh wg-list           # WireGuard: phones, last handshake (also wg-add, wg-remove)
journalctl -u briardr -f                  # logs (also briardr-herdr, caddy)
```

A checkout cloned at a release tag has no branch to fast-forward, so name the release to move to:
`scripts/setup-linux.sh upgrade v0.2.0`. Settings → About (and `briardr update --check`) says when there's
a newer release and shows that command for it; the helper's checkout doesn't replace itself. The [releases](https://github.com/ChrisSku/briardr/releases)
page lists them with their notes.

`mise exec -- bun scripts/smoke-deployment.ts --origin https://briardr.example.com --restart` (WireGuard:
`--origin https://10.8.0.1:7443 --cacert ~/.local/share/briardr/secrets/tls/ca.crt`) checks a running
deployment end to end through the URL phones use, as a phone would: pairing, live updates, terminal input and
resize, uploads, notes, changes, docs, a preview and its WebSocket, and a Briardr-only restart that keeps
Herdr's panes. It removes what it creates and revokes its own device.

### Restarts and reboots

- Restarting **Briardr** (`restart`, `upgrade`) never stops Herdr: `briardr.service` only *wants*
  `briardr-herdr.service`. Agents keep running; phones reconnect by themselves.
- Restarting **Herdr** ends every pane and the agents in it. The setup never does it on a rerun.
- A **reboot** of the server keeps files (repositories, notes, devices, sign-ins) but ends every running
  process. Both services start again at boot and Herdr restores its workspaces and panes, with fresh
  shells: the agents that were running are gone, so start them again. Uploads in `/tmp` are cleared.

### What's stored where

| Path | Contents | Backed up by `backup` |
| --- | --- | --- |
| `~/.local/share/briardr/config.json` | Settings (public origin, preview origins, Herdr path) | yes |
| `~/.local/share/briardr/secrets/` (mode 700) | Paired devices (token hashes), pairing code hash, Web Push keys and subscriptions, diagnostics salt | yes |
| `~/.local/share/briardr/state/` | Notes and their images, usage reports, monitor state | yes |
| `~/.local/share/briardr/logs/` | Connection diagnostics (no addresses, names, commands or text); rotate at 1 MB | no |
| `/tmp/briardr-uploads/` | Uploaded files, removed after 7 days or at reboot | no |
| `/etc/caddy/briardr.caddy`, `/etc/systemd/system/briardr*.service` | Generated deployment files | copies, in the archive's `deploy/` |
| `~/.local/state/briardr-setup/` | The setup's choices and backups of changed system files | `setup.env` only |
| `/etc/wireguard/wg0.conf` (root, 600) | WireGuard access: the server key, each phone's public and preshared key (`# briardr-peer: NAME`) | names, addresses, public keys only |
| `/etc/wireguard/briardr-wg0.nft`, `briardr-backups/` | The guard rules; previous `wg0.conf` versions (root only) | the rules |

Agent sign-ins and repositories live in the user's home directory and aren't part of Briardr's backup:
use disk snapshots for those. Briardr runs with `BRIARDR_DATA_DIR` set to the directory above, so
`scripts/setup-linux.sh pair` and `~/.local/state/briardr-setup/cargo-target/release/briardr …` as the same user use the same data.

## Security model

This section is about public access; [WireGuard access](#wireguard-access-private-no-domain) adds the
tunnel in front of the same rules.

Briardr on the internet relies on device pairing rather than on network membership:

- **Every route needs a paired device** except the session probe, pairing and static files; WebSockets
  too, and they must come from the public origin. `test/linux.test.ts` checks every route through the
  proxy mode.
- **Device cookie:** a random 256-bit token in a `__Host-` cookie, `Secure; HttpOnly; SameSite=Strict`,
  never in a URL, script, storage or log. Revoking a device closes its sockets, terminals and previews.
- **Host and Origin:** only the public host is served (421 otherwise); state-changing requests and
  WebSockets must carry exactly the public origin. Caddy keeps both headers as the browser sent them.
- **Client addresses:** Briardr accepts connections only from `trustedProxies` (loopback). It believes
  only the rightmost `X-Forwarded-For` entry a trusted proxy added; Caddy replaces any the client sent.
  Addresses are used for rate limits only, never for access.
- **Limits:** requests without a device: 120 per minute per client, 2000 overall. Pairing: 10 attempts
  per 15 minutes per client, 100 overall. A code lives 10 minutes and survives 5 wrong guesses per
  client and 20 in total (about a 3·10⁻¹¹ chance of guessing it), so one stranger can't burn your code;
  failures don't reveal whether a code is active. The remaining denial-of-service risk is many addresses
  guessing during your 10 minutes: create a new code if pairing says it's no longer valid.
- **No extra login layer.** We considered putting Caddy authentication (basic auth or a forward-auth
  service) in front. It isn't adopted: Home Screen apps on iOS don't keep basic-auth credentials
  reliably, service-worker and push navigations would fail, and WebSockets would need the same
  credentials. It is not tested; if you add one, test pairing, WebSockets, the Home Screen app and
  notification taps through it yourself.
- **Mac-only features are off:** battery, Always on, Sleep Mac, server dictation (use the keyboard's
  dictation in text fields) and the Keychain usage source. The UI hides them and the API answers
  `501 unsupported`.

## Previews

A dev server running on the server (say on port 5173) is shown in Briardr's Preview tab. With WireGuard
access, previews are on Briardr's own address at ports 7444–7459 (`https://10.8.0.1:7444` …), with
Briardr's certificate; the device cookie opens them and is removed before the request reaches the dev
server. Behind a public proxy each preview slot has **its own origin** (`https://p1.briardr.example.com` … `p16`), so a dev
server's pages can't act as Briardr, read its data or reuse its cookie:

- Briardr's device cookie is host-only (`__Host-`), so it never reaches a preview origin.
- Opening a preview returns a **one-time launch link** (random, valid for a minute, for that preview and
  device). It sets a preview session cookie for that origin alone and redirects to the page. A reload
  gets a fresh link.
- The session ends when the preview closes or the device is revoked; neither cookie is forwarded to the
  dev server. Dev servers and preview listeners stay on loopback.
- HTTP, assets, redirects and WebSockets (HMR) are forwarded. Point your dev server's HMR client at the
  page's own host and port 443 if it insists on its own port (Vite: `server.hmr.clientPort = 443`).
- The preview origins must be subdomains of the same registrable domain as Briardr (Safari blocks
  cookies in cross-site frames). `--preview-origin` changes the pattern; `--no-previews` turns previews
  off.

## Troubleshooting

- **Certificate isn't issued:** DNS must point at the server and ports 80 and 443 must be open in the
  provider's firewall. `scripts/setup-linux.sh diagnose` shows Caddy's ACME errors. Let's Encrypt
  limits failed attempts per hour; fix the cause before retrying many times.
- **Phone can't connect:** try mobile data and Wi-Fi. Working on one only points at that network or its
  DNS. A stale `AAAA` record breaks IPv6 networks.
- **"Herdr isn't running on the server":** `systemctl status briardr-herdr`; if a Herdr server was
  started by hand, stop it (`herdr server stop`) and `sudo systemctl start briardr-herdr`.
- **Briardr doesn't start:** `journalctl -u briardr -n 50`. Exit code 78 means the configuration is
  invalid; the log says which setting. The service doesn't retry until it's fixed. Exit code 75 ("Can't
  listen on 10.8.0.1") means wg0 isn't up: `systemctl status wg-quick@wg0`. Briardr retries every few
  seconds and starts once it is.
- **WireGuard phone can't connect:** `scripts/setup-linux.sh diagnose`. No handshake in `wg-list` means
  the phone doesn't reach UDP 51820 (provider firewall, tunnel off, wrong `Endpoint`, or a phone clock
  far off). A handshake but no Briardr means the CA isn't trusted on the phone, or Briardr isn't running.

## Provider notes

Briardr's configuration is the same everywhere; only these provider resources differ.

- **AWS Lightsail:** create an Ubuntu LTS instance; attach a *static IP* (review current provider pricing); in
  *Networking*, the IPv4 and IPv6 firewalls each need HTTPS (443) and HTTP (80) and SSH limited to your
  address (WireGuard access: a custom UDP 51820 rule instead of 80/443). If the instance has IPv6, either add a matching AAAA record and open the IPv6 firewall, or turn
  IPv6 off for it. Enable automatic snapshots for the disk. Check the plan's price, data transfer allowance
  and burstable CPU behaviour, and set a billing alarm.
- **AWS EC2:** an Ubuntu LTS AMI; an *Elastic IP* (review current public IPv4 pricing); a *security group* with
  80/443 (WireGuard access: UDP 51820) from `0.0.0.0/0` and `::/0` and 22 from your address; an EBS volume with snapshots (Data
  Lifecycle Manager or AWS Backup).
- **Azure VM:** an Ubuntu LTS image; a *static* Standard public IP; a *network security group* allowing
  80/443 (WireGuard access: UDP 51820) inbound and SSH from your address; a managed disk with Azure Backup or snapshots.

On all three, set a budget or billing alert and keep backups outside the instance.
