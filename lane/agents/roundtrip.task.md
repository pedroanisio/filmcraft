**S7b: round-trip tests between `scene::export` and `scene::import`.**

Both are committed. Scope: only the new integration test `crates/interchange/tests/scene_roundtrip.rs` and fixtures
under `crates/interchange/tests/fixtures/scene/roundtrip/`. If a round trip fails because of a bug in export or
import, write the failing test, mark it `#[ignore = "<reason>"]`, and list it in your reply with the minimal
reproduction; do not edit export or import.

## Tests
- FilmCraft → scene → FilmCraft: build sequences in the test (one clip; trimmed at speed 2; two overlapping tracks;
  motion with an anchor; opacity keyframes with ease; a cross dissolve; markers; captions; text and a rect; a
  nested sequence; 29.97 fps). After export then import, compare: track count, clip intervals (exact, in Tick),
  source in-points, speed, Motion values (within 1e-6), keyframe times (exact) and values, transition windows,
  markers and captions.
- scene → FilmCraft → scene: for each import fixture that maps fully (no overlays, nothing reported as dropped),
  export the imported sequence and run `scene::validate` on it (zero errors), then resolve both documents and
  compare the layer intervals and source maps (within one tick).
- proptest: random FilmCraft sequences on the four common grids (24, 25, 30000/1001, 48 kHz audio) within two hours
  round-trip their clip intervals exactly.
