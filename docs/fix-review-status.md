# Fix review implementation status

Reviewed `screenshots/3ds-kingdom-newlands-port-fixes.md` on 2026-11-03.
The folder contained the review document, with no screenshot images.

Implemented all six priority patch groups: version-gated save migration and world-index validation, empty offset tracks, vertex flush before frame split, queued effect-channel protection, packaging source pins and expanded publication guard, audio header/mixing hardening, panic hook, abort strategy, and declared Rust 1.88 minimum. Workspace packages inherit the minimum version. Validation also covers scenery prototypes and packed sheet references.

Additional fixes: scatter termination guards, stream header reads before linear allocation (including mono predictor initialization), payment event bound, duplicate wall scan, multiple-midnight/nonfinite clock handling, unbiased integer RNG including the full signed range, explicit save serialization failure, arrow stuck lifetime consistency, JSON page validation, short WAV fmt rejection, pivot-aware island hit areas, absolute XDG/APPDATA save paths, fsync before save rename, preservation of the existing SD save on rename failure, island argument bounds, single world-file read, opt-in desktop debug overlay, render-target/shader setup checks, localhost proxy bypass, audio cache channel classification, stale desktop atlas pruning, and GUID-last parsing.

Regression coverage includes changed legacy migration fixtures, byte mutations and truncated current saves/progress, invalid world indices, empty animation tracks, and nonfinite gain/zero audio rate. Minimal save fixtures now include a steed so their saved ids name real entries.

## Remaining review work

- Hardware: long reflection soak, same-frame effect bursts/cache pressure, track switching and buffer lifetime, forced-panic applet, real legacy saves, desktop listening and touch comparisons.
- Hardware instrumentation: dropped quad and cache-flush failure counters, bounded stream refill retries, deferred stream-buffer destruction after DSP clear.
- Measure Old/New 3DS tick/draw time before allocation and rendering optimizations; no measurements were fabricated for `perf.md`.
- Broader simulation soak and direct subsystem tests, audio callback scratch reuse, per-frame allocation and clip-name reuse.
- Pipeline follow-ups: contextual conversion-worker panic reporting, chunked audio comparison, parser sentinel documentation/default diagnostics, packaging dependency vendoring, CIA permission reduction, YAML dependency migration, staging cleanup.
- Packaging binaries already supplied through PATH remain operator-provided; source pins apply to the fallback source builds. CIA tools were not rebuilt from scratch, and Rust 1.88 itself was not installed/tested.
- The review's screenshot-result advice does not match the locked macroquad API: `export_png` returns `()` and unwraps internally. Replacing that API requires a separate encoder path.

## Validation

Workspace build/tests, Clippy with warnings denied, shellcheck, publication guard, and 3DS release build are run locally. Compilation does not establish GPU/DSP behavior on hardware. Generated assets and device binaries remain ignored and were not published.
