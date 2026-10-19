# Scenarios, configurations and extending the harness

## Files

| File | Role |
|---|---|
| `bench/run.ts` | Orchestrator: builds both sides, runs stages, writes `report.md` and `meta.json` |
| `bench/fixtures.ts` | Deterministic fixture data (seeded): 100 or 500 streams, changes, docs, notes, terminal screens |
| `bench/server.ts` | `FixtureServer`: serves a build like the server's `server/src/app.rs` (`scripts/static.ts`, the TS mirror of `server/src/static_files.rs`: same cache headers, the build's Brotli/gzip copies when accepted) and answers every API route and WebSocket; control methods `pushModel`, `broadcast`, `termBurst`; options `fixedNow` (frozen clock) and `busy` (streams another client controls) |
| `bench/page-harness.ts` | In-page instrumentation installed before app code: event and WebSocket timestamps, `MutationObserver`-based measurement, frame gaps, long tasks, listener/interval/observer accounting, launch milestones |
| `bench/perf.ts` | Scenario runner (Playwright) |
| `bench/report.ts` | Summaries: median, p95, n, Δ, Δ%, bootstrap interval per metric |
| `bench/sizes.ts` | Sizes per file and category; source-map attribution by package |
| `bench/routes.ts` | Files fetched per screen; the real server's content encodings |
| `bench/parity.ts` | Screen parity (pixels, text, accessibility tree) at a frozen clock |
| `test-ui/validation.test.ts` | Behaviour checks runnable against any build (`BRIARDR_WEB_DIST`, `BRIARDR_UPDATE_DIST`) |

## Configurations

| Id | Browser | Viewport | CPU |
|---|---|---|---|
| `chromium-desktop` | Chromium | 1440×900 @2x | 1× |
| `chromium-iphone-4x` | Chromium | iPhone 15 Pro (touch, mobile) | 4× throttled (CDP) |
| `webkit-desktop` | WebKit | 1440×900 @2x | 1× |
| `webkit-iphone` | WebKit | iPhone 15 Pro | 1× (WebKit can't throttle) |

Service workers are blocked in perf runs (the validation stage covers them). Fixture sizes: 100 and 500 streams;
size-dependent scenarios run at both, the rest at the smallest size only.

## Scenarios

| Scenario | What happens | Main metrics |
|---|---|---|
| `launch` | Fresh browser context per launch, alternating builds every launch | `populated` (rows in DOM), `populated-frame` (list on screen), `lcp`, `fcp`, `interactive`, `cdp:ScriptDuration`, `js-heap-used`, long tasks |
| `nav` | Agents → Workspaces → Usage → Settings → Agents through the main navigation; each ends when that screen's full content is present (all tree rows; Settings after its API response) | `nav:*` `dom`, `after-frame`, `proc` |
| `jump` | Open Jump, type a matching query, a non-matching one, clear | `jump:open/match/no-match/clear` `dom` |
| `filter` | Agents count strip: Working on/off, Ready (collapsed group) on/off | `filter:*` `dom` |
| `expand` | Collapse and expand the first workspace in the tree | `expand:collapse/expand` `dom` |
| `live` | The server moves one stream Working ↔ Needs you; measured from WebSocket receipt to the row in its new group | `live:to-blocked/to-working` `dom` |
| `burst` | 20 full model updates 80 ms apart, each renaming one stream | `update-median`, `update-max`, `last-update`, frame gaps, scripting |
| `term-open` | Open a stream from the list; then Back | `dom:mounted` (xterm), `dom:live` (controlling), `term-close` `dom` |
| `term-output` | 180 full-screen frames at ~60 fps into a controlling terminal | `last-frame-shown`, `frames-over-50ms`, `frame-gap-p95`, long tasks, scripting |
| `surfaces` | Changes, Preview, Docs, Notes, Terminal tabs; checks the terminal element and its socket survive every switch | `surface:*` `dom`; check `terminal-kept:*` |
| `notes` | New note, 10 keystrokes, paste 13 KB of Markdown, autosave, Preview, back to the list | `notes:*` (`proc` for keys), check `autosave` (PUTs and base revisions) |
| `memory` | 30 opens and closes of a stream (and a Peek every third), once per round | heap, DOM nodes, listeners, sockets, intervals, observers before/after; checks `released-after-*` |

## How a measurement works

```ts
await measure(c, page, cdp, 'scenario:name', { trigger: 'click', preds: { ready: `<JS expression>` } }, () => locator.click(), record);
```

- `trigger` is a DOM event type (the first one reaching `window` in the capture phase starts the clock) or
  `recv:<id>` for a WebSocket message the fixture server marked with `bench: '<id>'` (`s.pushModel(mutate, id)`).
- Each predicate is a JavaScript expression evaluated in the page; its first true moment (checked on every DOM
  mutation, focus change and frame) is the `dom` time. Use content, not presence: e.g. "exactly 500 tree rows and the
  heading says Workspaces", so a half-rendered screen doesn't count.
- `record=false` during warm-ups. `measure` also records `frame`, `after-frame`, `proc`, long tasks (Chromium) and CDP
  deltas (Chromium).
- Keep scenarios framework-neutral: CSS classes, roles, labels and visible text only. The one hook into app code is
  `.term-host`'s `briardrTerminal` (the terminal controller), which both builds set.
- Leave the page where the next scenario expects it (most start with `toAgents(page, s)`), and restore fixture state
  you changed (see how `notes` deletes its note on the server and broadcasts the change).
- Add the scenario name to `ALL_SCENARIOS` and, if its cost grows with the number of streams, to `SIZE_DEPENDENT`.

## Adding a parity screen

Append a step to `STEPS` in `bench/parity.ts`: a name (`NN-name`) and a function that drives the page from the
previous step's state. Steps run in order in one page per profile and scheme, so each step must leave a predictable
state. Run a build against itself first (`bun bench/parity.ts a=<dir> b=<dir>`): every screen must be identical, or
the step depends on timing (animations, unfocused fields, a frame not yet painted).

## Fixture changes

`makeFixture(size, now, seed)` is deterministic for a given size and seed, and both sides get identical data. Keep it
that way: no `Math.random`, and times relative to `now` (the parity stage freezes `now` on the server and the page).
`picks` names the streams scenarios use (`terminal`, `live`, `burst`); the busy stream for parity is `s3`.
