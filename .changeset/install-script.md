---
"briardr": minor
---

**Install Briardr with one command, and let it install Herdr.** `curl -fsSL https://briardr.dev/install.sh | sh`
downloads the release for your Mac or Linux computer, checks it against the release's checksums, puts it
in `~/.local/lib/briardr` and `briardr` in `~/.local/bin`; run it again to upgrade. It then runs the new
`briardr setup`, which checks for Herdr and offers to install the version Briardr is tested with
(checksum-verified, into `~/.local/bin`), leaves another installed version alone, reports Git, Tailscale
and Swift, and says what's next. When Herdr is missing or not running, `briardr serve` and Settings → Herdr
now say what to do. New: `briardr --version`, and `briardr install-power` for Always on with the lid
closed (Settings and the pairing screen now name `briardr` commands instead of `mise run`). Herdr's pinned
version and checksums moved to `deploy/versions.env`.
