# Interpreting results

## Metrics

| Metric | Meaning |
|---|---|
| `dom`, `dom:<name>` | Event (or marked WebSocket message) to the first moment the expected DOM is present |
| `frame` | Start of the next animation frame after `dom` |
| `after-frame` | A task queued from that frame: roughly when the frame was produced (on screen) |
| `proc` | Event to the end of its handlers and the microtasks they queued (Svelte schedules renders as microtasks) — how long the input blocked the main thread |
| `populated` | Cold start: navigation start to the Agents rows being in the DOM |
| `populated-frame` | Cold start: the list on screen (a task queued from the first frame after `populated`) — the cold-start number to report |
| `lcp`, `fcp` | Browser paint timing. FCP is unreliable for comparisons: one build may paint an empty shell first while the other's first paint is the full list |
| `interactive` | First time after `populated` that a queued task runs within 50 ms |
| `cdp:ScriptDuration` etc. | Chromium's `Performance.getMetrics` deltas over the interaction (script, layout, style, task) |
| `longtask-*` | Long tasks overlapping the interaction (Chromium only) |
| `update-median/max`, `last-update` | Burst: per-update receipt-to-DOM latency |
| `last-frame-shown`, `frames-over-50ms`, `frame-gap-*` | Terminal output: when the last frame reached xterm's buffer, and frame pacing during the burst |
| memory metrics | Before vs after 30 cycles; heap and DOM counters after forced GC (Chromium); listeners on window/document/visualViewport/media queries, intervals, resize observers, open sockets (all engines) |

## Noise

- `report.ts` resamples each side 2,000 times (seeded) to get a 95% interval of the difference of medians; an interval
  containing 0 is reported as within noise. With 8 samples (quick mode) intervals are wide; use full mode (20) before
  stating conclusions.
- Even "significant" differences under a few percent can be drift: the same build measured hours apart
  differed by a median of 1.5% (p95 27%, mostly on 1–3 ms WebKit metrics with 1 ms resolution).
- Read absolute numbers too. A +50% on a 2 ms interaction is 1 ms; on unthrottled engines most interactions finish
  within one or two frames in any build. The throttled iPhone config exaggerates both gains and losses.
- Fixture timings exclude real network latency, which adds equally to both builds.

## How Svelte 5 shows up in the numbers

- **Effect timing.** Svelte runs `onMount`/`$effect` before paint, so effect work (focus, mounting xterm, starting a
  request) counts in `proc`. Moving work into or out of an effect shifts time between `dom` and `proc`.
- **Teardown timing.** Svelte tears components down synchronously, so leaving a screen whose cleanup is expensive
  (disposing xterm) is slower by that cleanup.
- **Fine-grained updates.** Svelte updates only the bindings whose state changed. Live updates, bursts, filters and
  tree toggles show a change that makes many components depend on the same state.
- **Bundle/runtime.** A larger bundle and runtime show up in cold start (`populated-frame`, script, heap), not in
  interactions.

## Investigating a large difference

1. Rerun only the affected scenarios in full mode (`--stages perf,report --scenarios <x> --mode full`) to confirm.
2. Record a Chromium trace of one iteration for each build: Playwright's `browser.startTracing(page, { categories:
   ['devtools.timeline', 'disabled-by-default-devtools.timeline', 'v8.execute'] })` around the interaction, with the
   page harness installed (`ctx.addInitScript(pageHarness, { launchPredicate })`) so `window.__bench` timestamps line
   up with `performance.now()`. Compare the main thread's long `RunTask`s and their children (`FunctionCall`,
   `RunMicrotasks`, `Layout`, `Paint`, `MinorGC`).
3. Read the framework's scheduling code if the trace shows work moving between tasks.
4. Write down the cause next to the number. A difference without a cause is not a finding yet.
