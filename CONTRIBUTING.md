# Contributing to Briardr

Documentation, reproducible bug reports, accessibility feedback, and device testing are welcome
alongside code. Help us make Briardr useful beyond its first setup.

## Start here

- Use [GitHub Issues](https://github.com/ChrisSku/briardr/issues) for bugs and feature proposals.
- For a larger change, open an issue describing the use case first. Small fixes and documentation
  improvements can go straight to a pull request.
- Report vulnerabilities privately using [SECURITY.md](SECURITY.md).
- Set up the code with the [development guide](docs/development.md). Keep personal work out of the
  `briardr-dev` test session and keep real secrets out of fixtures.

Good first areas include improving setup instructions, testing a device/browser combination, and
making an interaction easier with a keyboard or screen reader. Only label an issue `good first issue`
once its expected behavior and a small starting point are clear.

## Bug reports

Include the Briardr commit/version (Settings → About), host OS and architecture, Herdr version,
client OS/browser, access mode, reproduction steps, and expected versus observed behavior. Share
only sanitized logs or screenshots. Do not include pairing codes, credentials, WireGuard QR codes,
private keys, personal terminal output, or complete private configuration files.

## Pull requests

1. Work on a branch with a focused change.
2. Explain the problem, resulting behavior, and relevant tradeoffs.
3. Run the checks appropriate to the change. For code, start with `mise run typecheck`,
   `mise run build`, `mise run check`, and `mise run test`. For UI changes, run relevant browser
   checks and include sanitized screenshots when useful.
4. Add a regression test when it demonstrates meaningful behavior. Documentation-only changes do
   not need application tests; verify paths, commands, links, and claimed platform support.
5. Update user-facing docs if setup or behavior changes. Separate tested behavior from assumptions.
6. Add a [changeset](.changeset/README.md) with `mise run changeset`: how the change moves the
   version and what the changelog says about it. Docs, CI and test changes add an empty one
   (`mise run changeset --empty`).

Preserve the [terminal and authentication rules](docs/architecture.md#boundaries-to-preserve).
Never solve a connection problem by skipping authentication, disabling Origin checks,
replaying queued terminal input, or broadening network access by default.

There is no guaranteed review or release schedule. Everyone taking part follows the
[Code of Conduct](CODE_OF_CONDUCT.md). Keep discussion constructive: explain your
reasoning, respect people's time, and assume good intent. Harassment, personal attacks, and sharing
someone else's private information are not welcome. Maintainers may remove abusive content or
restrict participation through GitHub's moderation tools.

## License of contributions

By intentionally submitting a contribution for inclusion, you agree it may be distributed under the
[MIT license](LICENSE), unless explicitly stated otherwise.
You must have the right to contribute it. Preserve third-party notices and identify material
needing separate attribution. No separate contributor agreement is currently required.
