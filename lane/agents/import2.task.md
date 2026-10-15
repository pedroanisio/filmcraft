**S6: `scene::import`, part two: text and shapes, groups and sequences, transitions, markers, captions and audioMix.**

S5 is committed: `scene/import*` maps project, assets, layers, transforms, keyframes and audio, plus the B7
overlays. Read it, its tests, `scene/resolve.rs` (the semantic pass) and `scene/stack.rs` first. You extend S5's
importer; do not restructure it beyond what your rows need.

Scope (you may edit):
- `crates/interchange/src/scene/import*` (prefer new submodules, e.g. `import/graphic.rs`, `import/group.rs`,
  `import/transition.rs`, `import/meta.rs`), and their tests.
- New fixtures under `crates/interchange/tests/fixtures/scene/import/`, and `crates/interchange/tests/scene_import.rs`.
- Read-only: `scene::read`, `values`, `validate`, `resolve`, `stack`, `export`. If `resolve` lacks something, say
  so in your reply; work around it only if the workaround is small and local.

Every timing, layout and paint-order fact still comes from `resolve`. Time becomes `Tick` once, with S5's snapping.

## Mapping rows (from the plan's import table)
- **Text and shapes:** text asset layers and `shape` (rect, ellipse, polygon, star, path) become graphic clips with
  text or shape layers (`filmcraft_project` graphic model; read how `scene::export` writes them to see the inverse).
  Fill, stroke, font family/size/weight, alignment and colour where the graphic model has them; the rest reported.
- **Groups:** a group that isolates or retimes (resolve's `NodeClass::Group { isolated, retimed }`) becomes a nested
  sequence (its own sequence item, the same size and fps, a nest clip in the parent at the group's span). A plain
  group is flattened into the parent: its children are already placed and transformed by resolve.
- **Sequences:** flattened; resolve has already auto-placed the children.
- **Transitions:** resolve's `Transition { from, to, cut, window, kind, curve }` becomes a track transition between
  the two clips (or a one-sided one) through a type table (the inverse of `scene::export`'s table). The clips must be
  on the same track: when stacking put them on different tracks, report it and keep both clips without the
  transition. Unsupported curves, params and handles are reported.
- **Markers:** `markers` become sequence markers (name, comment, colour where it maps, duration).
- **Captions:** `captions` become a caption track with its cues.
- **audioMix:** track volume and pan (basic); the rest reported.

## Tests (TDD: write each test, see it fail on the VM, then implement; hand-worked expected values)
- a text layer and a rect, ellipse, star and path shape
- an isolated group becoming a nested sequence (with its interval and its children inside)
- a retimed group (timeScale 2) becoming a nested sequence whose clip is retimed
- a plain group flattened (the child's world transform and inherited opacity)
- a sequence with timeGap flattened
- a cross dissolve between siblings, a one-sided fade-in, a transition whose clips landed on different tracks
- markers, captions, audioMix
- **round trip of your rows through `scene::export`:** export → import gives the same graphics, markers, captions
  and transitions (to the frame).
- hostile: deep nested groups up to the resolve limits never panic; proptest over random small documents mixing
  your elements never panics.
