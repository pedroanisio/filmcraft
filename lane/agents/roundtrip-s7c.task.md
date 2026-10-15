**S7c: fix the two round-trip regressions that S7b pinned as `#[ignore]` in `crates/interchange/tests/scene_roundtrip.rs`.**

Your worktree is on `b/roundtrip` at 8084adab (S7b, committed). Scope (you may edit):
- `crates/interchange/src/scene/export.rs` and `scene/export/**`;
- `crates/interchange/src/scene/import/properties.rs`;
- tests in those modules and `crates/interchange/tests/scene_roundtrip.rs`.

Ask in your reply before touching anything else (BLOCKED with the exact file and reason).

## 1. `filmcraft_trimmed_speed_opacity_ease`: export puts keys at the wrong time (S7 bug)
FilmCraft evaluates effect keyframes at **media time** (`TrackItem::effect_time_at`). `scene::export::animation` writes those key times as
if they were clip-local, so a trimmed or speed-changed clip's first key comes back at 6s instead of 2s.
- Convert each key from media time to the scene's time base exactly, with Tick/rational math (no f64 seconds for edit math), honouring:
  - the clip's in-point;
  - speed;
  - reverse.
- Keys outside the clip's visible range: keep them if the scene format allows keys outside the layer interval. If it doesn't, bake the
  boundary value and report it. Never drop silently.

## 2. `filmcraft_opacity_ease_curve`: import approximates CSS beziers (S5 approximation)
At one quarter of the way it gives 26.5625% instead of 29.375%. Policy: **never drop for precision**. Find out what FilmCraft's
keyframe model can represent exactly (`filmcraft_project` keyframes: temporal handles / bezier interpolation), then:
- if a CSS `cubic-bezier(x1,y1,x2,y2)` can map exactly onto FilmCraft temporal handles, do that, and also for the named eases
  (`ease-in`, `ease-out`, `ease-in-out`, which are CSS beziers);
- otherwise bake the segment into extra keys at the sequence frame rate, so every frame matches the CSS curve within 1e-6, and
  report it as baked, not approximated.

Also check that export writes FilmCraft's own eases so they re-import identically. The round-trip must be exact at frame times.

## TDD
- Un-ignore both regression tests first and show them red.
- Add unit tests with hand-worked values:
  - a speed 2 clip with in-point 4s, whose keys are at 6s and 8s media time;
  - a reversed clip;
  - one named-ease segment at t = 0.25 / 0.5 / 0.75.
- Make them green.
- Keep `scene_import`, `scene_export` and all S5/S6 tests green. Update an existing expectation only if it encoded the bug, and say
  which ones in your reply.

Gate on the VM only: `cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange`.
Don't commit. Reply HANDOFF with the gate result.
