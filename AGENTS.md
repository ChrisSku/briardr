# Working on Briardr

Briardr is a remote window into Herdr: a Rust server (`server/`) and a Svelte 5 web client (`web/`), with
Bun for the client build, the tests and the scripts. Read [docs/development.md](docs/development.md) for
setup and checks, [docs/architecture.md](docs/architecture.md) for the code map, and follow
[CONTRIBUTING.md](CONTRIBUTING.md).

## Tools

- mise provides Bun, Rust and the linters at the versions in `mise.toml`. When `bun` or `cargo` isn't on
  PATH, use `mise run <task>` or `mise exec -- <command>`; `mise tasks` lists the tasks.
- Run `mise run install` (`bun install`) in every new checkout or worktree. It installs the dependencies
  and the git hooks in `lefthook.yml`.

## Every pull request

1. **Branch from `main`** and keep the change focused. `main` only takes squash merges, so the pull
   request's title becomes the commit subject.
2. **Add a changeset.** Write `.changeset/<short-name>.md` by hand; `changeset add` asks questions, so
   don't run it without `--empty`:

   ```md
   ---
   "briardr": patch
   ---

   **What changed, as one bold sentence.** Then the detail someone upgrading needs.
   ```

   Until 1.0.0, `patch` is for fixes and new features and `minor` for breaking changes (an upgrade
   that makes people change their setup, configuration or pairing); from 1.0.0, `patch` fixes, `minor`
   adds and `major` breaks. When nothing users notice changes (docs, CI, tests), run
   `mise run changeset --empty`. The summary becomes the pull request's `CHANGELOG.md` entry; see
   [.changeset/README.md](.changeset/README.md). Never set the version or write a version's
   `CHANGELOG.md` section yourself, and never push tags: merging releases.
3. **Let the hooks run.** Before each commit they check Rust formatting. Before each push they check the
   changeset, the version, typecheck, clippy and workflow lint, and run the tests the pushed files need:
   `mise run test` (about 25 seconds) for code, and `mise run test:ui` (about 3 minutes) for the client
   or its tests. Fix what they report and never skip them with `--no-verify`: CI runs all the checks and
   both suites again, for every change, so skipping only moves the failure there.
4. **Run the tests while you work** (the same two commands), so the push doesn't find the failures. Each
   run has its own Herdr session, ports and client build (in `test-results/`), so runs in parallel
   worktrees don't collide. `mise run build` writes `web/dist`, which a running Briardr serves.
5. **Describe the pull request** with the sections of
   [.github/pull_request_template.md](.github/pull_request_template.md): what changes and why, what you
   ran and what you didn't test, and docs and compatibility.
6. **Wait for CI** after pushing (`gh pr checks --watch`) and fix failures until every check passes.

## Rules

- Preserve the [terminal and authentication rules](docs/architecture.md#boundaries-to-preserve).
- Keep real secrets, personal terminal output and private paths out of fixtures, docs and screenshots.
- Update user-facing docs when setup or behavior changes.
