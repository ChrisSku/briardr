# Configuration and data

Briardr stores configuration and private runtime data outside the source checkout by default:

| Host | Data directory |
| --- | --- |
| macOS | `~/Library/Application Support/Briardr` |
| Linux | `~/.local/share/briardr` |
| Override | `$BRIARDR_DATA_DIR` |
| `mise run dev` | `.dev-data/` in the checkout |

The data directory contains `config.json`, owner-only `secrets/`, `state/`, and `logs/`. Keep backups
private: they contain TLS and push keys, device records, notes, and other personal data. Uploaded
terminal attachments are temporary and separate from notes; see the [Linux storage table](linux-deployment.md#whats-stored-where).

## Change deployment settings

```sh
briardr configure --help
briardr status
```

`configure` validates changes before saving and keeps the previous configuration in `config.json.bak`.
Use `--dry-run` to inspect a proposed change. Restart the server after deployment changes. Managed
Linux installations should use `scripts/setup-linux.sh` for access-mode changes so the services,
firewall, and proxy agree with Briardr's configuration.

| Setting | Purpose |
| --- | --- |
| `herdrPath`, `herdrSession` | Herdr executable and local session. An empty session uses Herdr's default. The path is also editable in Settings. |
| `herdrTimeoutMs` | Deadline for each request to Herdr; default 8000. |
| `port` | Main port; default 7443. |
| `listen` | Direct-mode listeners besides loopback: `tailscale` or specific host IPv4 addresses. Default: `["tailscale"]`. |
| `allowedNetworks` | Additional allowed client IPv4 addresses or CIDRs; default empty. Loopback and Tailscale peers are already allowed. |
| `hostnames` | Additional hostnames for local certificates and Host validation. Configure DNS separately. |
| `tls.mode` | `local-ca` (default), `tailscale`, `files`, or `proxy`. |
| `tls.certFile`, `tls.keyFile` | Certificate and private key paths for `files` mode. |
| `publicOrigin` | The HTTPS URL devices use; required in proxy mode. |
| `trustedProxies` | Proxy-mode peers whose forwarded client address is accepted. Default: loopback. |
| `publicPreviewOrigin` | Proxy-mode preview origin pattern, such as `https://p{n}.briardr.example.com`; absent/empty disables proxy previews. |
| `previewPortStart`, `previewPortCount` | Preview listener range; defaults to 7444 and 16 slots. |
| `notifications` | Alert categories, names in alerts, and debounce behavior. |
| `usage` | Optional provider usage sources. |
| `power` | macOS keep-awake settings. |

For advanced settings, use [`server/src/config.rs`](../server/src/config.rs) as the reference. When
editing JSON, preserve unrelated settings; the examples in guides show only the relevant fields.

## Environment overrides

| Variable | Purpose |
| --- | --- |
| `BRIARDR_DATA_DIR` | Configuration, secrets, state, and logs directory. Use the same value for serve and pair. |
| `BRIARDR_HERDR_SESSION` | Override Herdr's session; development uses `briardr-dev`. |
| `BRIARDR_PORT` | Override the main port. |
| `BRIARDR_PREVIEW_PORT` | Override the first preview port. |
| `BRIARDR_LISTEN` | Comma-separated listener list. Empty means loopback only. |
| `BRIARDR_PROJECT_ROOT` | Source checkout root, if the binary cannot infer it. |
| `BRIARDR_WEB_DIST` | Alternate built web directory; used by both the build script and server. |

These are deployment/development controls. Test-only overrides are described in the code and test
harness; do not enable test hooks in a deployed build.

## Usage sources

Usage is optional. Configure it under Settings → Usage sources:

- **Claude Keychain:** macOS only, explicit opt-in; uses Claude Code's sign-in for a read-only usage
  request to Anthropic. macOS may ask for Keychain permission on the host.
- **Claude status-line tap:** opt-in integration that captures rate limits Claude Code provides to
  its status-line command. Review the proposed settings change before installing; Briardr chains the
  existing status line and offers removal/restoration.
- **Codex app server:** opt-in; asks Codex's app server for limits using its own sign-in. Reading
  usage does not start a session or agent turn.

Usage sources depend on provider behavior and local CLI versions. Keep agent credentials on the
host; do not put them in Briardr configuration, bug reports, or the repository.
