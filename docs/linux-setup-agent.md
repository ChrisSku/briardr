# Set up Briardr on Linux with an agent

Give this file to an agent (Claude Code, Codex, …) running on the Linux server, as the user Briardr will
run as, inside a clone of the repository. The agent does the setup with the shipped helper,
`scripts/setup-linux.sh`, checks the result and hands over; it doesn't improvise its own installation.
[docs/linux-deployment.md](linux-deployment.md) explains every step, the security model, previews,
backups and provider firewalls.

## Prompt to give your agent

```text
Read docs/linux-setup-agent.md and docs/linux-deployment.md in this checkout. Set up Briardr on this
server for my phone using scripts/setup-linux.sh. Ask me first which access mode I want: public HTTPS
at my own domain, or private WireGuard (no domain). Run the helper's check first and fix or report the
blockers. Ask me for the domain (public), which agent CLIs to install, and for anything only I can do
(DNS, the provider's firewall, agent sign-in, setting up the phone). Preserve existing configuration,
devices and secrets. Finish with the URL, pairing instructions, the checks that passed and what's left
for me.
```

## What the agent does

0. **Ask which access mode** the user wants, and explain the choice in a sentence each (the table at
   the top of [docs/linux-deployment.md](linux-deployment.md)):
   - **public**: `https://<domain>` behind Caddy; needs a domain, DNS and TCP 80/443 open; nothing to
     install on the phone; the pairing screen is on the internet.
   - **wireguard**: `https://10.8.0.1:7443` through a WireGuard tunnel to this server; no domain, only
     UDP 51820 open, no third party; the phone needs the WireGuard app and Briardr's CA profile.
   If `~/.local/state/briardr-setup/setup.env` exists, the server already has a mode (`ACCESS=`): keep it
   unless the user asks to switch, and tell them that switching means pairing each phone again.
1. **Inspect, read-only.** `git status`, `git log -1`, then
   `scripts/setup-linux.sh check --domain <domain>` (public) or `check --access wireguard`. It reports
   the OS, CPU, sudo, disk, ports, DNS against the public IP (public) or the WireGuard module, UDP port
   and tunnel network (wireguard), installed tools and their versions, existing services and Briardr's
   data. Blockers stop the setup; warnings don't.
2. **Get what only the user can provide**, without asking for secrets in chat:
   - public: the domain, and DNS: `A <domain>` and `A *.<domain>` → the server's static IP (no stale
     AAAA); the provider's firewall: TCP 80 and 443 open, SSH restricted (see *Provider notes*);
   - wireguard: a static public IP, and the provider's firewall: UDP 51820 open, SSH restricted,
     80/443 closed;
   - which agent CLIs to install (`claude`, `codex`).
3. **Install:** `scripts/setup-linux.sh install --domain <domain> [--agents claude,codex]` or
   `install --access wireguard [--agents claude,codex]`. It is safe to rerun after fixing something; it
   changes only what differs. Don't replace its steps with your own
   commands, change pinned versions (`mise.toml`, `deploy/versions.env`), regenerate
   `bun.lock`, or edit the generated units and Caddy site by hand.
4. **Agent sign-in:** ask the user to run `claude` / `codex` once in a terminal on the server as this
   user. Never copy credentials, tokens or keys into files, logs or chat.
5. **Prove it works:** `mise exec -- bun scripts/smoke-deployment.ts --origin https://<domain> --restart`
   (public) or `--origin https://10.8.0.1:7443 --cacert ~/.local/share/briardr/secrets/tls/ca.crt
   --restart` (wireguard). It pairs a temporary device, drives a terminal, uploads, notes, changes, docs
   and a preview, restarts Briardr only, then cleans up. All checks must pass.
6. **Set up the phone.** Wireguard: the user runs `scripts/setup-linux.sh wg-add <phone name>` in their
   own terminal on the server and scans the QR code; it holds the phone's private key, so don't run it
   in a way that keeps its output (logs, chat, files), and never ask for the key. Walk them through
   the steps it prints (WireGuard app with On-Demand, CA profile with full trust). Then, both modes:
   have the user open the URL and add Briardr to the Home Screen first. Then run
   `scripts/setup-linux.sh pair`, show the code to the user only (don't save it), and have them sign
   in inside the installed Home Screen app. Phone checks (cellular and Wi-Fi, notifications
   from the Home Screen app) count only when the user confirms them.

## Rules for the agent

- Run as the Briardr user, never as root, and don't change HOME. The helper asks for sudo itself.
- Don't restart Herdr or reboot unless the user agrees: both end the running agents.
- Don't open Briardr's port (7443) or dev-server ports in any firewall, don't widen `allowedNetworks`,
  and don't disable certificate checks to make something pass. Don't edit `/etc/wireguard/wg0.conf` by
  hand or print it (it holds keys); use `wg-add`, `wg-list` and `wg-remove`.
- If the helper stops, read its message, fix the cause it names (or ask the user), and rerun the same
  command. Backups of changed system files are in `~/.local/state/briardr-setup/backups/`.
- For a domain that isn't ready yet, `--internal-tls` gives a working test setup with Caddy's own CA;
  switch with `install --acme-tls` once DNS points at the server.

## Hand-over

Report, without secrets: the access mode and URL; versions (`scripts/setup-linux.sh status`); the
deployed commit; the service user and data directory; the commands for status, logs, restart, pairing,
upgrade, rollback, backup and restore (from `docs/linux-deployment.md`), and for wireguard `wg-add`,
`wg-list` and `wg-remove`; which checks passed (setup verify, smoke test, phone checks the user
confirmed); and what's left for the user, such as DNS, the provider firewall, agent sign-in, IPv6, or
setting up the phone.

## Later

- Update: `scripts/setup-linux.sh upgrade <tag>` with the new release's tag (a checkout of a release
  has no branch to fast-forward), or `upgrade` alone on a branch. It rebuilds,
  redeploys, verifies, and goes back by itself if that fails; `rollback` returns to the previous commit.
- Back up before risky changes: `scripts/setup-linux.sh backup`.
- Phone can't connect or HTTPS fails: `scripts/setup-linux.sh diagnose`.
- Wireguard: a new phone is `wg-add NAME`; a lost or replaced one is `wg-remove NAME` plus revoking its
  Briardr device in Settings → Devices.
