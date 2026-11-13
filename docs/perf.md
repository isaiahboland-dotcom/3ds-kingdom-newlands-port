# Performance

Profiling results and entity-cap decisions (Phase 3–4).

## Measuring

The `profile` feature drives `game::prof` marks; the in-game debug overlay
(SELECT) shows the top five per tick. A synthetic stress benchmark stands in
for a mid-game kingdom, since the converted assets stay local:

```
cargo test -p game --features profile --release --test perf -- --nocapture
```

It builds a `WorldData` from defaults, injects 40 sites under construction,
24 builders, 24 marked trees, 150 loose coins and a 400-coin purse (a ~300
coin pile), then ticks 600 times plus a burst phase that pays and picks up
10 coins every second. Numbers are host-relative; the ARM11 at 268 MHz runs
the same code far slower: an Old 3DS XL measured 3.4 ms per tick early in a
new game, where this host averages ~0.03 ms, so budget roughly 100× (the
old 6–10× estimate was far off). One tick must fit the
16.7 ms frame alongside rendering (up to 4 catch-up ticks per frame).

## 2026-11-06 results

| metric                  | before      | after       |
| ----------------------- | ----------- | ----------- |
| tick avg                | 0.539 ms    | 0.18 ms     |
| burst worst tick        | 2.589 ms    | 0.6–0.7 ms  |
| bag share of tick       | 97.6%       | ~70%        |
| o3ds estimate, worst    | ~26 ms      | ~4.7 ms     |

(Run-to-run spread on the bench host is a few tenths of the burst number;
the ratios are the stable part.)

Before, the purse pile dominated every tick and spiked 26 ms (o3ds) on any
payment or pickup — the reported stutter while builders build or coins fly.
Per system after (`ms/tick`, stress benchmark): bag_solve 0.11, bag_events
0.04, bag_sweep 0.02, coins 0.005, population 0.002, everything else under
0.002.

### What changed

- `bag.rs`: spending and spilling woke **every** coin in the purse; now only
  the neighbours of the hole fall asleep/wake, like a Box2D island. The warm
  start left a `BTreeMap` node per contact per step — a sorted flat `Vec`
  now, and the contact and sweep-order buffers are reused across ticks.
  (A 1/30 s solve every other tick was tried and reverted: see "Purse step
  size" below.)
- `coins.rs`: each free coin scanned every grabber twice (dropper check plus
  taker search); one fused pass now, coins too high to touch anyone skip it,
  and grabbed coins find their purse through a sorted `Who` index instead of
  a linear scan (coins × units is exactly the many-coins-at-once spike).
- `state.rs`: a builder's hammer stroke used to mark the payables dirty, so
  the whole payable list rebuilt at up to 60 Hz while any crew worked.
  Refreshes now happen when a site finishes or is paid for.
- `units/mod.rs`: `reassign_work` counted busy workers per site by scanning
  all units per site (sites × units, once a second); one pass tallies them
  now. The night-shelter check looks its target up by binary search instead
  of a linear scan of every object, per worker per tick.

### On-hardware retest checklist

- The purse pile still tumbles believably (drop into a fat purse, pay
  things, watch the brim overflow).
- No new jitter in a 300-coin pile; coins still spill when heaped past the
  brim.
- Paying several walls in a row no longer drops frames (watch the debug
  overlay's `sim` number during a building spree).
- The `bag_solve` / `bag_sweep` marks show in the debug overlay's top five
  only while the pile is awake.

### Ground light (render)

`draw_ground_light` drew 3 px additive columns per fire in three bands, up
to ~255 quads per fire, and the 3D right eye draws them again. The lights
are now summed into one row of columns and each column is a single
gradient quad, so the cost is capped near 134 quads. Frame bench
(`cargo run --release -p extract --example bench -- assets <island> 600`),
busiest frame: 1173 → 607 quads on island 0, 1127 → 607 on island 2.

### 3DS renderer and build

- `gpu.rs`: translucent quads go straight into the linear vertex buffer as
  they arrive (runs of equal state recorded as ranges); only the opaque
  pass, which regroups by page, is still collected and copied at flush.
- `prof`: marks are skipped unless the debug overlay is up.
- `codegen-units = 1` in the 3DS release profile. `opt-level` 2 or `"s"`
  against 3 is still worth measuring on hardware (16 KB I-cache).

On-hardware check: sprites, glows, fades and the water reflection draw as
before (X still toggles painter's order), in 2D and with the 3D slider up;
compare the overlay's `draw`/`end` numbers with the previous build.

### Purse step size

The pile once solved one 1/30 s step every other tick to halve its cost.
On an Old 3DS the coins visibly jittered: at that step a resting pile
never stops rocking, so it never sleeps. Measured with
`cargo run --release -p extract --example bagjitter` (real purse from
`assets/world.bin`; 20 s to settle, then 10 s watched):

| purse | 1/30 s every other tick | 2 × 1/120 s every tick |
| ----- | ----------------------- | ---------------------- |
| 20 coins | 13 awake, moves every solve, 13 µs/tick | all asleep, 0.1 µs/tick |
| 60 coins | 56 awake, 82 µs/tick | 7 awake, 34 µs/tick |
| 150 coins | 120 awake, 283 µs/tick | 73 awake, 335 µs/tick |

Only while coins pour in is the 1/120 s step dearer (about 2×). It is the
current setting; the `a_settled_pile_sleeps_and_holds_still` test fails
with either longer step.

(The 150-coin row was a bench artifact: the purse only holds 60–80 coins,
and the bench re-asked for every spilled coin, so it measured a pile that
never stopped overflowing. In the game a spill leaves the wallet; the bench
now does the same.)

### Purse sleeping

Even so, a full purse never quite slept: thousands of coins fell asleep and
woke again every 10 s. The wake test (an awake coin moving faster than
`WAKE_SPEED`, 0.05, wakes a sleeping one it touches) ran after gravity was
integrated, and one 1/120 s substep of gravity is 0.075. So any awake coin
merely resting on a sleeping one woke it, every substep. The test now
takes that substep's gravity back out. Same bench, settled pile, purse
filled until everything has dropped in and settled 20 s:

| asked → kept | before | after |
| ------------ | ------ | ----- |
| 60 → 58–59 | 67.6 µs/tick, 14.5 awake, moving in 304/600 frames | 0.3 µs/tick, 0 awake, still |
| 150 → 72–73 | 219.6 µs/tick, 45.0 awake, moving in 542/600 frames | 0.4 µs/tick, 0 awake, still |
| 300 → 77 | 90.0 µs/tick, 17.4 awake, moving in 351/600 frames | 0.5 µs/tick, 0 awake, still |

Filling (while coins drop in) got 17–40% cheaper too (60: 90 → 75 µs,
300: 286 → 169 µs). Waking a settled pile by hand and letting it resettle
moves its coins by at most 0.065 units (mean ≤ 0.011, a coin is 0.094
tall), so nothing was left hanging in the air by the stricter test; it
sleeps again completely. Test: `bag::tests::a_full_purse_sleeps` (fails
with the old wake test).

The stress test (`--test perf`) doesn't see this: its 400-coin purse is
still pouring in for all 600 ticks, so the pile is never settled there.

### Draw list sort

`scene::top::draw_list` was ~7 µs a frame on the bench host (~220 items),
4.1 µs of it a stable sort comparing `(layer, order, z)` tuples. Each key
now packs into one integer that orders the same way (`sort_key`; a test
checks it against the tuple compare, including ±0.0 and infinities), with
the push index as tie-break so equal keys keep their order, and plain
integers are sorted unstably: sort 2.2 µs, list 5.0 µs per frame.

### Known remaining costs

- `draw_list` still allocates its list (now pre-sized) and the sort keys
  every frame; reusing buffers would need scratch space on `Session`
  (it's drawn through `&Session`).
- Filling the purse: while 60–80 coins pour in (about 10 s at one per
  0.15 s), the pile costs ~75–170 µs/tick on the host, most of it
  `bag_solve`. Only the Old 3DS can say what that is there; watch the
  overlay's `sim` while a big bag of coins drops in.
- Autosave writes the SD card synchronously. On an Old 3DS XL with a 57 KB
  save (2026-11-13), the write takes 119 ms (tmp file, fsync, FAT renames)
  out of 125 ms per autosave, and that one frame pulls the 30-frame FPS
  average down to about 24 at dawn. Possible fixes: write on a background
  thread, or spread the steps over several frames. Any fix must keep a
  readable copy at every step (`crates/n3ds/src/savefile.rs`).
- Night runs slower than day. On the same console FPS was about 28 at
  night (high 44.6) and rose steadily toward morning. The night lighting
  (torch and campfire point lights, ground light columns) is the likely
  cost; profile the render with the overlay at night.
