# Develop Briardr

## Set up a checkout

Install Git, [mise](https://mise.jdx.dev/getting-started.html), a C compiler/toolchain, and
[Herdr](https://herdr.dev/docs/install/). On macOS, install Apple's Command Line Tools. On Ubuntu,
install `build-essential`. Herdr 0.9.3 is the deployment/test baseline pinned in
[`deploy/linux/versions.env`](../deploy/linux/versions.env); newer versions need compatibility testing.

```sh
git clone https://github.com/ChrisSku/briardr.git
cd briardr
mise trust
mise install
mise run install
```

Bun and Rust versions come from [`mise.toml`](../mise.toml). Commit lockfile changes only when
intentionally changing dependencies. No npm package publication is required; `private: true` and
Cargo's `publish = false` prevent accidental registry publication while the source remains open.

## Run without touching your everyday Herdr session

In one terminal:

```sh
herdr --session briardr-dev server
```

In another terminal, from the checkout:

```sh
mise run dev
```

Then create a pairing code from another terminal:

```sh
mise run pair:dev
```

Open `https://localhost:17443`. Development uses the `briardr-dev` Herdr session, loopback listeners,
preview ports starting at 17444, and `.dev-data/`. Trust its development CA if testing browser
features that require trusted HTTPS. Keep that CA separate from your real installation.

`mise run dev` builds once and starts the server. For client rebuilds while editing, run
`mise exec -- bun scripts/build.ts --watch` separately. Refresh the page when needed. Restart Briardr
after server changes and rebuild with `mise run build:server`. Rebuilds do not restart Herdr.

## Checks

```sh
mise run typecheck      # TypeScript and Svelte
mise run build          # production web bundle
mise run check          # cargo fmt and clippy, warnings are errors
mise run test           # Rust unit/fixture tests, then Bun integration tests
```

The integration suite builds a debug server with test hooks and starts throwaway instances on
loopback. It uses `briardr-dev`, not the default Herdr session, but creates and removes fixture workspaces
there: keep personal work out of `briardr-dev`. The tests require `herdr`, `curl`, Git, and Python 3 on PATH.

For Rust-only changes, a narrower check is:

```sh
mise exec -- cargo test --manifest-path server/Cargo.toml --features test-hooks
```

The [Linux workflow](../.github/workflows/linux.yml) checks the client, Rust server, integration
suite, shell helper, and generated deployment files. macOS integrations require Mac-specific testing.

## Browser checks

Install Playwright's WebKit browser after installing dependencies:

```sh
mise exec -- bunx playwright install webkit
# Linux also needs browser system libraries:
# mise exec -- bunx playwright install-deps webkit
mise run test:ui
```

Most UI suites start their own isolated servers; the screenshot harness uses the running development
server. `test-ui/screens.ts` expects demo streams such as `refactor auth`; populate the expected
fixtures or adapt that harness before using it for screenshots. Never capture personal terminals,
repository names, keys, or sign-in prompts for a public issue or README.

The automated browser checks focus on WebKit at iPhone, iPad, and Mac sizes. They cannot prove
physical-device keyboard behavior, OS certificate installation, or real Web Push delivery. Document
the OS, browser, network mode, and result when doing device testing.

## Test hooks and deployment smoke tests

The `test-hooks` Cargo feature adds `/__test/*` endpoints. At runtime they also require
`BRIARDR_TEST_HOOKS=1` and a loopback peer. Build deployed binaries with `mise run build:server`
without this feature; a reverse proxy is itself a loopback peer.

For a dedicated test deployment, [`scripts/smoke-deployment.ts`](../scripts/smoke-deployment.ts)
exercises pairing, terminals, uploads, notes, Git changes, docs, previews, and an optional Briardr
restart. See [the Linux guide](linux-deployment.md#day-to-day) for exact commands. This is an active
test that creates temporary state; use a deployment you control and understand the restart option.

## Releases

The version is set in `server/Cargo.toml`; `package.json`, `server/Cargo.lock`, [CHANGELOG.md](../CHANGELOG.md)
and the install instructions (which clone the release tag) must agree, and CI checks that they do
(`mise run release:check`). A pull request releases a new version by bringing it:

1. In the pull request, `mise run release:bump 0.2.0` sets the version everywhere and starts its
   `CHANGELOG.md` section. Write the notes there and commit them with the change.
2. Merge the pull request.

When the Linux checks pass on `main`, the [release workflow](../.github/workflows/release.yml) tags that
commit `v0.2.0` and publishes the GitHub release with the changelog section as its notes. A merge that
keeps the version releases nothing. If a check fails, nothing is published: fix it in the next pull
request, and its merge releases the version.

## Useful starting points

See [architecture](architecture.md) for the code map and invariants. Follow
[Contributing](../CONTRIBUTING.md) for reporting bugs and opening a pull request.
