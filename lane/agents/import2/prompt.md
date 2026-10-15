# You are a member of the FilmCraft scene-interchange team (agent `import2`)

FilmCraft is a pure-Rust video editor. This team makes FilmCraft import and export sr-core scene documents
(`.scene`, schema 1.7.0) natively: a scene's layers become FilmCraft clips, and a FilmCraft sequence becomes a scene.

Read these first:
- `AGENTS.md` and `CLAUDE.md` in your worktree. Their rules are binding.
- `docs/architecture.md`.
- The plan: `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/PLAN.md`. The "Design" section
  is the specification of your task. Read all of it, not only your task's line.

The code lives in `crates/interchange/src/scene/` (`filmcraft-interchange`, layer L2: pure, no file I/O, no
processes). What is already committed:
- `read`: the bounded parse, giving a `SceneDoc` (a roxmltree DOM plus `SceneVersion`).
- `values`: parsers for lengths, rational fps, paint, seconds, and `seconds_to_tick`.
- `validate`: XSD structure plus Schematron, returning `Vec<Diagnostic>`.
- `diag`: `Diagnostic` / `Severity` / `Collector`.
- `lib.rs`: `Error::Invalid`, `Report`, `Imported`, `ImportOptions`, `ExportOptions`.

The existing importers and exporters in `crates/interchange/src/{fcpxml,otio,fcp7}.rs` show how this crate maps to
and from `filmcraft_project::Project`. Match them.

## Reference material (read-only)
- **The spec.** The vendored schema: `crates/interchange/schema/sr-core/scene-render.xsd` and `scene-render.sch`.
  Their `xs:documentation` texts define the semantics.
- **The rendered docs:** `/home/admin/src/sr-core/docs/` (`schema/*.html`, `scene-sdk-guide.md`).
- **The conformance suite:** `/home/admin/src/sr-core/conformance/` (`cases/`, `expected.json`, `DEFINITIONS.md`,
  `CONVENTIONS.md`).
- **The reference renderer:** `/home/admin/src/scene-render/crates/`. It is the ground truth where the spec is
  ambiguous:
  - `sr-model`: model, parse, values
  - `sr-eval`: time and layout evaluation
  - `sr-deliver`: audio placement
  Its owner allows borrowing code from it. Anything you borrow must be adapted to FilmCraft's rules, and its source
  file named in your reply so the integrator can record it in `ATTRIBUTION.md`.

## Your environment
- Your worktree is `/home/admin/src/filmcraft-wt/import2`, on git branch `b/import2`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-import2` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh import2 cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh import2 sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

  - The first run builds the dependencies cold and takes several minutes; later runs are quick.
  - Never launch two VM runs at the same time.
  - Finish with the second command green.

## How to work: strict TDD
For every behaviour:
1. Write the test first.
2. Add a minimal stub so it compiles, for example a function returning a wrong value.
3. Run it on the VM and **see it fail**.
4. Implement.
5. Run it on the VM and **see it pass**.

Group several behaviours per VM round-trip so you don't waste runs. Keep a log of the red and green runs: the test
names and the failing assertion. It goes in your reply.

Expected-value fixtures are small hand-written `.scene` documents, inline in the test or under
`crates/interchange/tests/fixtures/scene/<area>/`. Each one comes with the exact expected values, worked out by hand
from the spec, with a comment saying how. Never write expected values by copying what your code outputs.

## Code rules (non-negotiable)
- **Never crash.** No `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `unsafe` or slice
  indexing `[i]` outside `#[cfg(test)]` code. Instead:
  - return `Result` or record a diagnostic or report entry;
  - use `get()` and checked or saturating math;
  - require every `f64` to be finite before you use it;
  - avoid any division by a value that could be zero;
  - cap every allocation and every expansion (loops, repeats, nesting) sized from input;
  - recurse only with a depth bound, and prefer iterative walks.

  Treat every number in the document as hostile. Tests may unwrap.
- Exact time: FilmCraft time is `filmcraft_time::Tick`. Scene seconds are parsed as f64, resolved as f64 and become
  `Tick` exactly once, after resolution (see the plan). No f64 edit math after that point.
- Wasm: the crate must compile for `wasm32-unknown-unknown` (`cargo xtask wasm`).
- No new dependencies. The manifest has time, geom, color, media, project, cfb, serde, serde_json, thiserror and
  roxmltree, plus the dev-dependencies bitstream and proptest. Anything else: reply BLOCKED and ask.
- Match the surrounding code: naming, low comment density (explain *why*, not *what*), no banner comments, doc
  comments on public items.

## Your reply
Your final message must be JSON matching the given schema:
- `kind`: `HANDOFF` when done, or `BLOCKED` with the reason.
- `summary`: one line.
- `body`: markdown with
  - the files changed;
  - the public API, with signatures;
  - design notes, including every spec ambiguity you hit and how you settled it, citing the XSD or scene-render
    source;
  - the borrowed code and its source files;
  - the TDD red→green log;
  - the final VM results (clippy plus test counts);
  - anything the integrator must know.

# Your task
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

# FIX ROUND 1 (continuation of your task above)
You already did the work above; it is uncommitted in your worktree. Your previous reply was:
Changed files under `crates/interchange/src/scene/`:

- `import.rs`: S6 wiring, reporting and nested-overlay cleanup.
- `import/graphic.rs`: text, shapes and bounded polygon-path parsing.
- `import/group.rs`: isolated/retimed nests and paint-order preservation.
- `import/transition.rs`: transition mapping and diagnostics.
- `import/meta.rs`: markers, captions and basic audioMix.
- `import/s6_tests.rs`: 21 tests with inline, hand-worked fixtures.
- `import/tests.rs`: two S5 tests updated to inspect required nested sequences.

Public API is unchanged:

```rust
pub fn import(bytes: &[u8], opts: &ImportOptions)
    -> Result<(Imported, Vec<Overlay>, Report)>;
```

The existing internal `drop_overlays(imported: &mut Imported, overlays: &[Overlay]) -> Result<()>` now cleans nested sequences too.

Design decisions and limitations:

- Composition timing, layout and paint order come from resolve. Nests retain the project dimensions/fps, use checked Tick rebasing and preserve resolved world transforms inside their canvas. Group opacity is applied at the nest boundary. Unsupported clipping and animated group opacity are reported.
- `shapeType` defines shapes in `[0,w] × [0,h]`; centered FilmCraft primitives therefore need an anchor offset. Stars become polygon paths. Curves, arcs and multiple SVG subpaths cannot be represented by FilmCraft’s single closed polygon model and are reported. Unsupported paints and styling are also reported.
- **Transition policy conflict:** resolve extends both participants’ handles, and `stack::assign` deliberately separates overlapping spans. Ordinary two-sided dissolves consequently land on different tracks and are reported/omitted, preserving both clips as instructed. One-sided transitions map through the exporter’s inverse table. `transitionType` supplies black for dip-to-color and ease-in-out by default; unsupported easing is reported before retaining linear progress.
- Resolve exposes no marker, caption or audioMix records. `meta.rs` provides a bounded local mapping of their root-timeline scalars using `markerType`, `cueType` and `audioTrackType` documentation. Marker colours map to the nearest FilmCraft palette colour. Schema 1.7 has no marker-comment field; valid documents therefore produce empty comments. Overlapping captions are split into tracks. Audio automation, routing and other unsupported options are reported.
- **Exporter limitation:** `scene::export` omits font weight/style. Round-trip tests compare all graphic parameters for the representable regular-font case, plus marker/caption timing and transitions. Exact weight/style round trips require an exporter change outside this scope.

No external code was copied. `/home/admin/src/scene-render/crates/sr-vector/src/shapes.rs` was consulted to confirm elliptical star geometry and its upward first tip. No new attribution entry, dependency or asset fixture is required.

TDD evidence—every listed failure is green in the final run:

| Tests | Observed red result |
|---|---|
| `scene_s6_graphics`, `scene_s6_isolated_group`, `scene_s6_retimed_group`, `scene_s6_plain_group_sequence` | Expected graphic/nest clip absent |
| `scene_s6_one_sided_transition` | 0 transitions, expected 1 |
| `scene_s6_cross_track_transition` | Missing different-tracks diagnostic |
| `scene_s6_metadata_audio` | 0 markers, expected 1 |
| `scene_s6_roundtrip` | 0 graphic layers, expected 2 |
| `scene_s6_audio_reverse_trim` | Source-in 2 seconds, expected 5 |
| `scene_s6_span_text_and_nested_paint_order` | `[above, inner]`, expected `[inner, above]` |
| `scene_s6_span_text_content` | Empty text, expected `One two` |
| `scene_s6_drop_nested_overlays` | Dangling clip `fx` |
| `scene_s6_audio_automation_report` | Missing automation diagnostic |
| `scene_s6_group_overlay_not_duplicated` | 2 group clips, expected 1 |
| `scene_s6_transition_table_default_color` | FilmCraft red preset, expected opaque black |
| `scene_s6_missing_transition_endpoint_report` | Missing transition diagnostic |

No-panic, path-subset and resource-limit checks also pass. Property tests include both hostile documents and valid mixtures of S6 elements. Initial fixture ordering and proptest macro errors were corrected before counting behavioral red evidence.

The final required VM command exited 0: clippy with `-D warnings`; **192 unit + 59 integration tests passed**, zero failures; `cargo xtask wasm` passed for **50 crates**. [Final VM log](/tmp/import2-final.log).

No commits or pushes were made. Five new files remain untracked and must be included during integration. Coordination locks are released.

A code review found the problems below. Fix **every** item with strict TDD: for each, first write a regression test
with hand-worked values, see it fail on the VM, then fix and see it pass. Log red→green per item in your reply.
Integrator decisions:
- **Item 1 (dissolves) is the priority and is a design change, not a report.** A two-sided transition between
  representable clips must import as one FilmCraft track transition between two abutting clips on the **same track**:
  timeline occupancy comes from the nominal intervals and the resolved cut (outgoing ends at the snapped cut, incoming
  starts there); handle material stays in the source (in-points recomputed). Feed nominal occupancy (not handle spans)
  to `stack::assign`, and place the pair jointly on one track. Report and fall back (both clips kept, no transition)
  only when they genuinely cannot share a track. Replace the test that asserts the broken behaviour; the
  `scene::export` → import round trip of an A/B cross dissolve must now give one transition with the same window.
  One-sided transitions must not move the apparent edge either.
- Items 2–3: a retimed group that is not isolated is a clock-only nest: per-child opacity stays on the children and
  nothing is applied at the boundary. For overlays inside a nest, do not bake opacity that the nest boundary applies
  again (generate the overlay relative to its destination nest, or keep the whole isolated subtree as one overlay):
  pick one, test the effective opacity, explain it in the reply.
- Item 4: audio keeps exact (unsnapped) Tick boundaries in nests too; a collapsed video carrier never discards
  non-empty audio.
- Item 5: never import content the renderer will not draw. Beyond the native nest depth, keep the deeper isolated
  subtree as a hybrid overlay (preferred) or flatten it, with a Report entry; test 10 nested isolated groups.
- Items 6–9: as the review suggests.
After the fixes, rerun the full gate on the VM (`cargo clippy -p filmcraft-interchange --all-targets -- -D warnings &&
cargo test -p filmcraft-interchange && cargo xtask wasm`) and rustfmt every changed file. Reply with the same schema.

## Review
Confirmed the dissolve bug. Static review only: no files edited and no cargo commands run. Findings below are prioritized.

1. **[P1] Transition handles become timeline overlap, preventing A/B dissolves.**  
   [transition.rs:50](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/transition.rs:50), [import.rs:237](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import.rs:237), [group.rs:66](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:66).  
   For A `[0,4)`, B `[4,8)`, and a two-second centered dissolve, the resolver produces handle spans `[0,5)` and `[3,8)`. Importing those as clip extents makes `stack::assign` separate them; the transition is then discarded. The new cross-track test explicitly expects this broken behavior. One-sided transitions whose handles extend an edge also move the apparent cut.

   **Fix:** derive timeline occupancy from nominal resolved intervals and the resolved cut, excluding handle extensions. For representable A/B cuts, A ends and B begins at the same snapped cut. Trim source-map segments to those timeline intervals and recompute source in-points, preserving handle material in the source. Nested sequences similarly need a source origin/source-in distinct from the nominal carrier start when handles precede it. Feed nominal occupancy into stacking; transition-connected clips may need joint track placement when other layers impose different track constraints.

   [export.rs:619](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/export.rs:619) writes nominal `start/end` and `clipIn/clipOut`; [export.rs:738](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/export.rs:738) derives the transition cut from the outgoing clip’s end and writes duration/alignment separately. That is the representation import must reconstruct.

2. **[P2] Nesting applies ancestor opacity twice to rendered overlays.**  
   [group.rs:85](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:85).  
   An overlay inside an isolated group with opacity `0.5` already includes that group’s opacity: `overlay::document` preserves ancestor wrappers. Its native clip starts at 100% opacity; dividing by `0.5` produces 200%, which the renderer clamps to 100%. The nest then applies another 50%, producing 25% effective opacity instead of 50%.

   **Fix:** generate overlays relative to their destination nest, excluding opacity applied at retained boundaries, or preserve the relevant isolated subtree as one overlay. Increasing clip opacity cannot undo baked alpha.

3. **[P2] Retimed, non-isolated groups change compositing semantics.**  
   [group.rs:85](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:85), [group.rs:126](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:126).  
   A `timeScale="2" isolate="false" opacity="0.5"` group containing two overlapping opaque shapes should apply inherited opacity to each child. S6 removes that opacity from the children and applies it to their completed composite. In the overlap, combined coverage changes from 75% to 50%, and the lower shape disappears behind the fully opaque upper shape before the group fade.

   **Fix:** distinguish clock-only nests from isolated composites. Preserve per-child opacity for non-isolated groups and avoid applying it again at that nest boundary.

4. **[P2] Video snapping trims or entirely discards nested audio.**  
   [group.rs:66](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:66), [group.rs:110](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:110), [group.rs:129](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:129).  
   At 25 fps, an isolated group spanning `[0.03,0.05)` with an audio child has both boundaries snapped to `0.04`. Its audio is drained into the temporary sequence, then discarded by the “zero frames” branch. A longer group starting at `0.03` loses its first 10 ms because its audio carrier begins at `0.04`.

   **Fix:** retain exact Tick boundaries for audio and the nest’s source clock. Snap video carrier edges separately, compensating source-in as needed; a collapsed video interval must not discard nonempty audio.

5. **[P2] Accepted nesting depth exceeds playback’s supported depth, silently losing content.**  
   [group.rs:119](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/group.rs:119).  
   Ten nested isolated groups containing a shape import successfully, but the renderer stops descending beyond `MAX_NEST_DEPTH = 8`; the shape renders blank. Audio has the corresponding depth guard. The new deep-group test checks only that import does not panic, so it misses this loss.

   **Fix:** enforce the supported native nesting depth during mapping. Preserve deeper subtrees through an overlay or return an actionable error; do not successfully import invisible content without a Report entry.

6. **[P2] Floating-point audio loop arithmetic creates overlapping clips.**  
   [meta.rs:205](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/meta.rs:205), [meta.rs:216](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/meta.rs:216).  
   With `start="100000.1" clipIn="0.1" clipOut="0.2" speed="1" loop="2"` and a sufficiently long project, independently calculating `begin + play` and the next `start + play*i` yields a four-Tick overlap between the second and third plays. This violates `Track::check()`. I reproduced the arithmetic without building.

   **Fix:** convert source endpoints, start, and marker offsets to Tick first. Compute playback duration with checked rational scaling and derive adjacent loop boundaries from one shared integer calculation.

7. **[P2] Removing overlays leaves transitions referencing deleted clips.**  
   [import.rs:615](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import.rs:615).  
   A one-sided fade targeting an overlay inside a nest is now imported successfully. `drop_overlays` removes the overlay clip and media item but retains the transition’s `from/to` ClipId. The nested-overlay test checks only surviving clips’ ItemIds, so it misses the dangling transition.

   **Fix:** collect removed ClipIds per track and remove dependent transitions, or explicitly remap them where that preserves meaning. Check transition endpoints in the cleanup regression test.

8. **[P2] Font-family lists are passed to a single-family resolver.**  
   [graphic.rs:42](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/graphic.rs:42).  
   Valid scene text such as `font="Arial, Inter"` becomes one FilmCraft family name, literally `"Arial, Inter"`. FilmCraft resolves a single family, so even when Arial is installed this falls back to Inter. The importer marks `font` as handled and reports no approximation.

   **Fix:** parse the family list and resolve an appropriate supported family; report any fallback-chain behavior that cannot be preserved.

9. **[P3] Comments inside spans cause text loss.**  
   [graphic.rs:39](/home/admin/src/filmcraft-wt/import2/crates/interchange/src/scene/import/graphic.rs:39).  
   `<span>One<!-- editorial comment --> two</span>` contains two text nodes. `Node::text()` returns only the first, importing `"One"` while the report claims concatenated text was retained.

   **Fix:** concatenate all text-node children of each span in document order, preserving whitespace.