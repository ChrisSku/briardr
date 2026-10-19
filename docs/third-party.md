# Third-party software

Briardr's own source is available under the [MIT license](../LICENSE). This does not relicense its
dependencies, tools, or separately installed software.

| Software | Used for | Upstream |
| --- | --- | --- |
| Herdr | Agent runtime and terminal sessions on the host | [herdrdev/herdr](https://github.com/herdrdev/herdr) |
| Svelte | Web interface | [sveltejs/svelte](https://github.com/sveltejs/svelte) |
| xterm.js and add-ons | Browser terminal | [xtermjs/xterm.js](https://github.com/xtermjs/xterm.js) |
| markdown-it | Markdown rendering | [markdown-it/markdown-it](https://github.com/markdown-it/markdown-it) |
| highlight.js | Code highlighting | [highlightjs/highlight.js](https://github.com/highlightjs/highlight.js) |
| Tokio, Axum, rustls, and other Rust crates | Server, networking, TLS, and supporting functionality | [Cargo manifest](../server/Cargo.toml) and [lockfile](../server/Cargo.lock) |
| Bun, Rust, mise, Playwright, and other development packages | Build, development, and verification | [package manifest](../package.json) and [tool pins](../mise.toml) |
| Caddy, WireGuard, and Tailscale | Optional deployment and network access | [Caddy](https://github.com/caddyserver/caddy), [WireGuard](https://www.wireguard.com/), [Tailscale](https://github.com/tailscale/tailscale) |

This is a project map, not a complete dependency notice bundle. Exact resolved versions are recorded
in `bun.lock` and `server/Cargo.lock`. Before distributing compiled binaries, bundled web assets,
or images, inspect the resolved dependency licenses and include the notices required by those
licenses. Keep any applicable copyright, license, and NOTICE files with the distribution.

Names and trademarks remain with their owners. Briardr is an independent project and does not imply
endorsement by Herdr or the providers of supported agent tools.
