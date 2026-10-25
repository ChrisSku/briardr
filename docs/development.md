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

Bun, Rust and the linters come from [`mise.toml`](../mise.toml). `mise run install` also installs the
git hooks in [`lefthook.yml`](../lefthook.yml): Rust formatting before each commit, and the changeset,
version, typecheck, clippy and workflow lint checks before each push, with `mise run test` when the push
changes code and `mise run test:ui` when it changes the client or its tests. Skip one job for a push with
`LEFTHOOK_EXCLUDE=ui git push`; CI runs everything again, for every change. Commit lockfile changes
only when intentionally changing dependencies. No npm package publication is required; `private: true` and
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
mise run lint           # the GitHub workflows (actionlint) and the Linux setup helper (shellcheck)
```

The integration suite builds a debug server with test hooks and the client (into
`test-results/web-dist/test`, not `web/dist`), and starts throwaway instances on loopback ports below the
ephemeral range, each with its own data and uploads directory. Each run starts its own Herdr session
(`briardr-test-<pid>`) and removes it and its temp folders when it ends (a killed run's go at the next run),
so runs in parallel don't collide and your sessions are never touched. They ignore the variables git gives
its hooks (`GIT_DIR` and the like, unset in `mise.toml`), so a push from a worktree can't send the suites'
scratch commits into the branch. Its pane shells start without your zsh startup files, as in CI. To watch
the tests in Herdr, run them in the development session with `BRIARDR_TEST_SESSION=briardr-dev`. The tests require `herdr`, `curl`, Git, and
Python 3 on PATH.

For Rust-only changes, a narrower check is:

```sh
mise exec -- cargo test --manifest-path server/Cargo.toml --features test-hooks
```

The [Linux workflow](../.github/workflows/linux.yml) checks the client, Rust server, integration
suite, browser checks, shell helper, and generated deployment files, and packages the
[release archives](#release-archives) for every platform, for every change. macOS
integrations require Mac-specific testing.

## Browser checks

```sh
mise run test:ui
# Linux also needs WebKit's system libraries, once:
# mise exec -- bunx playwright install-deps webkit
```

`mise run test:ui` installs Playwright's WebKit when it's missing and builds the client into
`test-results/web-dist/ui`, so it never changes the `web/dist` a running Briardr serves. The UI suites start
their own servers, like the integration suite; the screenshot harness uses the running development
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

Every pull request brings a [changeset](../.changeset/README.md): `mise run changeset` asks whether the
change is a patch, minor or major one and for its changelog entry, and writes it to `.changeset/`. A pull
request that changes nothing users notice (docs, CI, tests) adds an empty one with
`mise run changeset --empty`. CI fails a pull request without one. Each pull request adds its own file,
so several can be open at once without conflicting over the version or the changelog.

Merging releases it. When the Linux checks pass on `main`, the
[release workflow](../.github/workflows/release.yml):

1. runs `changeset version`, which sets the next version in `package.json` from the changesets waiting
   on `main`, writes their section of [CHANGELOG.md](../CHANGELOG.md) and removes them, and
   `scripts/release.ts sync`, which copies the version to `server/Cargo.toml`, `server/Cargo.lock` and
   the install instructions (they clone the release tag). The release GitHub App pushes that to `main` as
   "Version X.Y.Z".
2. tags that commit `vX.Y.Z` right away, so the install instructions never name a tag that doesn't
   exist yet.
3. builds the release archives on macOS and Linux runners, and publishes the GitHub release with them,
   their `SHA256SUMS`, and the changelog section as its notes, in the same run. The Linux checks then
   run on the version commit too.

Merges that land close together can share a version, and empty changesets release nothing. If a check
fails, nothing is versioned or published: the changesets wait on `main`, and the merge that fixes it
releases them. If building or publishing fails after the push, the run for the version commit (or the
next run on `main`) releases that version first, at its "Version X.Y.Z" commit, before it makes a newer
one, so no version is skipped. Don't set the version by hand; `mise run release:check` checks that it
agrees everywhere.

The workflow needs the release GitHub App (Contents: read and write, installed on the repository, and in
the `main` ruleset's bypass list): its client ID in the repository variable `RELEASE_APP_CLIENT_ID` and
a private key in the secret `RELEASE_APP_PRIVATE_KEY`.

### Release archives

Each release has an archive per platform, `briardr-<os>-<arch>.tar.gz` for `macos` and `linux` on
`aarch64` and `x86_64`, that runs Briardr without a checkout, Bun or Rust.
[`scripts/package.ts`](../scripts/package.ts) makes it: the release server as `bin/briardr`, the web
client in `web/dist` next to it (where the server looks when its executable is a release's
`bin/briardr`), the dictation helper's source on macOS, `LICENSE`, and `THIRD-PARTY-NOTICES.txt` with the
license texts of the Rust crates and npm packages built into them. The Linux archives are built on Ubuntu
24.04 and need its glibc (2.39) or newer.

CI packages every change ([`package.yml`](../.github/workflows/package.yml)), so a version that doesn't
build on some platform is never pushed, and each run keeps its archives as artifacts for a week.
`mise run package` packages for your computer into `dist/` (`-- --target x86_64-apple-darwin` for another
target, after `rustup target add` for it). It builds the server into `server/target/<target>` and the
client into the archive's folder, so the checkout's `server/target/release/briardr` and `web/dist` stay
as they are. Making and building plugins still needs a checkout: see [Plugins](PLUGINS.md).

## Useful starting points

See [architecture](architecture.md) for the code map and invariants. Follow
[Contributing](../CONTRIBUTING.md) for reporting bugs and opening a pull request.
