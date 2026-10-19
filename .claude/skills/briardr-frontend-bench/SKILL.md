---
name: briardr-frontend-bench
description: Compare two builds of Briardr's web client (web/src) with the bench/ harness — bundle and route-payload sizes, interaction performance in Chromium and WebKit at desktop and iPhone sizes, pixel/text/accessibility parity of every screen, and behaviour checks (pairing, revocation, uploads, service-worker updates, the UI test suite) — and report what changed. Use this whenever a change touches the frontend, the build, a client dependency or the framework and someone wants to know if it got faster, slower or bigger, whether any screen changed, or wants before/after numbers, or before deploying a frontend change to the production server. Not needed for server-only changes.
---

# Briardr frontend bench

`bench/` measures two builds of the web client against each other with the same fixture data, so differences come
from the client, not from Herdr, Git or the network. One command builds both sides and runs any of these stages:

| Stage | Answers | Time | Needs |
|---|---|---|---|
| `sizes` | How big is each build, raw/gzip/Brotli, and which parts (framework, app, xterm, markdown) grew | seconds | — |
| `routes` | What a fresh Agents load downloads; does any later screen fetch more; does the server compress | ~1 min | briardr-dev Herdr |
| `parity` | Do 20 screens × iPhone/iPad/Mac × light/dark look, read and announce the same (pixels, text, accessibility tree) | ~3 min | — |
| `validation` | Pairing refusal, revocation, upload cancel, service-worker update + offline launch, for each build and base → cand | ~1 min | briardr-dev Herdr |
| `tests` | The whole `test-ui` suite against each build, plus `bun test ./test` | ~3 min | briardr-dev Herdr |
| `perf` | Interaction latency, scripting, long tasks, memory after repeated navigation | quick ~15 min, full ~2.5 h | idle machine |
| `report` | `report.md`: everything above in one page, largest significant performance differences first | seconds | `perf` data |

## Ground rules, and why

- **Never build experiments into `web/dist`.** A Briardr running from this checkout (`mise run serve` or the
  LaunchAgent, port 7443) serves `web/dist` on every request, so writing there changes what paired devices get immediately. The harness
  builds into `bench/builds/` (`BRIARDR_WEB_DIST`); do the same for any manual build, and don't run `mise run build`
  or `mise run test:ui` (which builds) unless deploying is the goal.
- **Performance needs a quiet machine.** Other builds, test suites, browsers or a second benchmark running at the
  same time distort the numbers, and a laptop slows down as it heats up. Run `perf` alone, in the background, and wait for it; never
  in parallel subagents. The runner alternates the builds round by round so drift and heat affect both equally.
- **Read differences against noise.** Each comparison carries a bootstrap 95% interval; when it contains 0 the
  difference is noise. Measuring one build twice drifted 1.5% (median) in earlier runs, so smaller differences mean
  nothing even when "significant".
- Bun comes from mise: prefix commands with `export PATH="$HOME/.local/share/mise/installs/bun/1.4.2/bin:$PWD/node_modules/.bin:$PATH"`
  if `bun` isn't found. Playwright's Chromium and WebKit must be installed. The test helpers start the isolated
  `briardr-dev` Herdr session themselves; they never touch the user's real panes.

## Running it

```sh
# Quick (~20 min): sizes, routes, parity, validation, a short perf run (2 configs, 100 streams, 8 samples), report
bun bench/run.ts --base main --cand .

# Everything, with the full perf matrix (4 configs × 100/500 streams, 20 samples) — takes about three hours
bun bench/run.ts --base main --cand . --mode full

# Only what a change can affect, e.g. a fix to opening and closing terminals
bun bench/run.ts --base main --cand . --stages perf,report --scenarios term-open,memory --mode full
```

Then read `bench/results/<name>/report.md` (name defaults to `<base>-vs-<cand>`; set it with `--name`).

**Sides** (`--base`, `--cand`): a git ref is checked out in a temporary worktree under `bench/worktrees/`, installed
with its own lockfile and built with its own build script (older commits without `BRIARDR_WEB_DIST` are handled);
builds are cached in `bench/builds/<label>-<sha>` (`--rebuild` forces a new build). `.` builds the working tree
including uncommitted changes. A directory that already holds a build (`index.html` + `assets/`) is used as is.
`--labels a,b` names the sides in reports.

**Perf options**: `--configs` (chromium-desktop, chromium-iphone-4x, webkit-desktop, webkit-iphone), `--sizes`
(100,500), `--scenarios`, `--rounds`, `--per-round`, `--warmup`, `--launches` override the mode. Scenarios: `launch`,
`nav`, `jump`, `filter`, `expand`, `live`, `burst`, `term-open`, `term-output`, `surfaces`, `notes`, `memory` — what
each does and measures is in `references/scenarios.md`. Use `--mode full` (20 samples) before drawing conclusions;
quick runs are for spotting regressions early.

When a stage fails, its log is in the results directory (`validation-*.txt`, `test-ui-*.txt`) and the others still
run; `report.md` is written either way.

## Reading the results

Read `references/interpreting.md` before reporting performance numbers. The short version:

- `dom` is when the expected content is in the DOM after the triggering event; `after-frame` approximates when it is
  on screen; `proc` is how long the event's handlers and their microtasks kept the main thread busy. A change can make
  `dom` faster and `proc` slower at the same time (work moved into the event) — report both when they disagree.
- For cold starts use **`populated-frame`** ("list on screen") and `lcp`; first contentful paint compares different
  paints between builds and is misleading.
- WebKit times have 1 ms resolution and no CDP or long-task data; the iPhone-sized Chromium config is 4× CPU-throttled
  and magnifies differences. None of it is a real device.
- `perf/checks.jsonl` lists behaviour checks (all fixture content shown, terminal kept across surfaces, autosave
  correct, resources released). A failed check is a real problem.
- Parity: identical means pixels, `innerText` and the accessibility snapshot all match at a frozen clock. Earlier runs
  compared a build with itself and every screen matched, so any difference is real. Look at the `-DIFF.png` images in
  `bench/screens-full/<name>/` and the text/aria diffs in `parity/parity.json`.

Before concluding that a large difference is a framework or code effect, find its cause (a Chromium trace of one
iteration is usually enough; see `references/interpreting.md`) and rerun the affected scenarios.

## Reporting back

Lead with the answer, then the evidence:

```markdown
**<cand> vs <base>: <one-sentence verdict>.**
- Size: app JS <a> → <b> (<Δ%>), gzip <Δ%>; <what grew>.
- Screens: <n>/<m> identical (<what differs and whether it is intended>).
- Behaviour: validation <pass/fail>, tests <pass/fail>, perf checks <pass/fail>.
- Performance (<mode>, <n> samples): <largest real wins and losses with ms and %>, <what is within noise>.
Full report: bench/results/<name>/report.md
```

Each run creates everything it writes: results in `bench/results/<name>/`, full-size screenshots in
`bench/screens-full/<name>/` and builds in `bench/builds/`, all ignored by git. Put the numbers that matter in the
commit message or the document that uses them.

## Extending

New UI that a change should be measured on: add a scenario to `bench/perf.ts` or a screen to `bench/parity.ts`, and
fixture data to `bench/fixtures.ts` / `bench/server.ts`. `references/scenarios.md` explains the measurement API, the
predicates and how to keep scenarios framework-neutral (selectors and visible content only, nothing from a
framework's internals).
