# Security policy

Briardr is in early development. Security fixes currently target the latest development branch;
there is no long-term supported release line or guaranteed response time yet.

## Reporting a vulnerability

Use GitHub's **Security → Report a vulnerability** for
[ChrisSku/briardr](https://github.com/ChrisSku/briardr/security/advisories/new) when private vulnerability
reporting is available. Include the affected commit, deployment mode, impact, and minimal steps
to reproduce. Use a disposable test setup and redact credentials and personal data.

If private reporting is unavailable, open an issue asking the maintainer for a private reporting
channel **without disclosing the vulnerability or exploit details**. Do not put vulnerabilities,
live keys, or personal terminal data into a public issue or pull request.

## Deployment trust model

- A paired device is trusted to operate terminals and files with the host user's permissions.
  Briardr has no separate user roles, per-project authorization, or sandbox for terminal commands.
- Run Herdr and Briardr as the same non-root user. Keep Herdr's control socket local.
- Use trusted HTTPS. With public access, use the documented local reverse proxy configuration;
  keep backend and development-server ports off the public internet.
- VPN membership permits network access; it does not replace device pairing. Revoke lost devices
  in Briardr and remove their VPN access separately.
- Keep runtime data, backups, agent credentials, CA private keys, pairing codes, and WireGuard
  configurations out of the repository and issue attachments.
- Build deployed binaries without `test-hooks`. Runtime test hooks must stay disabled; a local
  reverse proxy connects from loopback too.

The server validates peer networks, Host, Origin, and device cookies. Public proxy deployments
also use pairing/request limits and separate preview origins. These are defense measures, not a
claim of an independent security audit. See [architecture](docs/architecture.md) and the detailed
[Linux security model](docs/linux-deployment.md#security-model).

Web Push payloads are encrypted, but delivery uses browser vendors' infrastructure. Optional usage
sources and agent CLIs have their own provider connections. Review each integration's settings for
the data it accesses.
