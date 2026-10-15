# scene-render audio issues found by FilmCraft export work (2026-10-14)

Found while making FilmCraft's sr-core exporter embed linked audio safely (agent fixexport, rounds 2–3).
FilmCraft works around all three by emitting an independent `audioMix` placement (`preservePitch="false"`)
whenever embedding would hit them. Not filed upstream yet — owner to decide.

## 1. Subframe layer starts lose audio until the next frame boundary
- `crates/sr-deliver/src/audio.rs:422` samples layer clocks only at frame boundaries `k / fps`.
- `crates/sr-audio/src/mix.rs:476` skips samples that have no current-frame source time.
- Repro: 24 fps, layer with audio starting at 0.01 s. Audio starts ≈0.041667 s; a layer spanning `[0.01, 0.02)` is silent.
- Expected: audio follows the layer's exact (sub-frame) start, as the XSD's independent audio timing implies.

## 2. Last frame extrapolated forward at 1×
- `crates/sr-audio/src/mix.rs:479` extrapolates the final mapped frame forward at 1× when the next frame has no clock.
- Repro A: reversed source `[5,6)` over 1 s at 24 fps. Final frame should descend 5.041667 → 5.0; it advances toward 5.083333.
- Repro B: 2× source `[5,7)`. Final frame should run 6.916667 → 7.0; it advances toward 6.958333 (plays at half speed).
- Expected: extrapolate using the clock's actual rate/direction (or the previous frame's slope).

## 3. Float frame time vs strict half-open visibility drops whole frames
- `audio.rs:422` evaluates frame time as `k / fps`; `crates/sr-eval/src/eval.rs:1055` uses strict half-open visibility.
- Repro: 24000/1001 fps, layer on frames [9,10), serialized start `0.375375`. Frame 9 evaluates to `0.37537499999999996 < 0.375375`, so neither frame contributes a clock → one-frame placement is silent; longer placements lose their first frame, and upward-rounded ends can gain one.
- Expected: compute frame times rationally (or snap with an epsilon / round to the serialized precision) before visibility tests.
