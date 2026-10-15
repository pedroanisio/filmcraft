# You are a member of the FilmCraft scene-interchange team (agent `import`)

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
- Your worktree is `/home/admin/src/filmcraft-wt/import`, on git branch `b/import`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-import` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh import cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh import sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

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
**S5: `scene::import`, an sr-core scene as a FilmCraft sequence (project, assets, layers, transforms, keyframes,
audio), plus the overlay sub-documents for what it cannot map (the interchange half of B7, hybrid rendering).**

Scope (you may edit):
- `crates/interchange/src/scene/import.rs`, or `scene/import/**`, and its test module.
- New fixtures under `crates/interchange/tests/fixtures/scene/import/`.
- A new integration test `crates/interchange/tests/scene_import.rs`.
- `scene/mod.rs`: your `pub mod` / `pub use` lines only.
- `crates/interchange/src/lib.rs`: only the `Format::Scene` import dispatch arm, and `ImportOptions.lenient: bool`
  (default false; no other format reads it).

Already committed and read-only for you:
- `scene::read`, `values`, `validate`
- `scene::resolve` and `scene::stack` (S4). **Read their docs and tests first.**
- `scene::export` (S7)
- `Format::Scene`

Every timing, layout and paint-order fact comes from `resolve`'s output. Do not re-derive any of it from the XML.
If `resolve` lacks something you need, say so in your reply. Work around it only if the workaround is small and
local.

## What S4 gives you (read `scene/resolve.rs`, its tests, and `scene/stack.rs`)
- `resolve::resolve(&doc, &ResolveLimits::default()) -> Result<Resolved>`. `Resolved` has `project`, `nodes` (in **paint
  order**), `transitions`, `unmappable` (sorted by paint position) and `notes`.
- `ResolvedNode.index` is the document-preorder index, **not** the position in `nodes`. Look nodes up with
  `Resolved::node(index)`. `parent` and `transform_parent` hold such indices.
- `interval` is the nominal visibility. `span` adds transition handles and is clipped by the ancestors. Source
  segments cover `span`. Use `span` for clips and stacking.
- Source maps:
  - `SourceMap::Segments` holds `{comp_start, comp_end, src_start, rate}`. Rate 0 is a hold. A negative rate is
    reverse.
  - `SourceMap::Remap { keys, freeze_local }` holds keys in composition time with absolute source seconds.
- `Animation` carries `keys: Vec<(NodeId, f64)>` already in composition seconds, plus `time_base` and
  `default_interpolation`. Read each key's value and interpolation from the XML through `doc.doc.get_node(id)`.
  `Resolved::key_time(node, base, t)` converts other times.
- `Layout` holds `transform` (pixel position/anchor/scale/rotation/skew), `world_matrix` (`Some` only when static and
  flattenable), `transform_static` and `parent_transform_unresolved`. `ResolvedNode.opacity` already includes the
  inherited group opacity.
- `unsupported: Vec<String>` lists per-node features that cannot be mapped (expression, additive, roving and so on).
  Report them.
- `stack::assign(&[Span{start,end}]) -> Vec<u32>` takes Tick spans in paint order.

## Pipeline
`pub fn import(bytes: &[u8], opts: &ImportOptions) -> Result<(Imported, Vec<Overlay>, Report)>`, plus the
`import_with` dispatch arm for `Format::Scene`. That arm returns the `Imported` and `Report`, and **refuses** a
document that has overlays when the caller cannot write them: `Error::Other` naming the count, unless
`opts.lenient`, which drops them and reports it. The engine calls `scene::import` directly.

The steps:
1. `read::decode` → `read::read` (default `Limits`).
2. `validate::validate`.
   - With errors: a strict import returns `Error::Invalid(diags)` (errors first).
   - A `lenient` import continues and puts every diagnostic in the Report.
   - Warnings always go to the Report.
3. `resolve`.
4. Map the result, per the plan's "Import mapping" table and its "Time" section.

## Mapping (this task: everything except the S6 rows)
**Project and sequence**
- One sequence named after the document. Its settings are the project's width, height and fps (integer or
  rational), and its duration keeps trailing blank time.
- The project background:
  - a solid paint (default opaque black) becomes a colour-matte clip on the bottom video track over the whole
    duration;
  - a gradient is reported and becomes its first stop's colour;
  - transparent means no matte.

**Assets**
- `assets/video|image|audio|imageSequence` become media items, with paths resolved against `opts.base_dir` the way
  the other importers resolve them (`common::resolve_path`).
- Their media info comes from what the document says. Never read files.

**Layers**
- Each layer becomes clips: one per source-map segment.
- Each segment becomes a clip with that source in-point and speed. Rate 0 is a freeze or hold: use the project's
  freeze mechanism if there is one, otherwise a still and a report.
- `timeRemap` becomes time-remap keyframes if `filmcraft_project` has them, otherwise it is reported and its first
  segment's behaviour kept.
- A video asset that has audio also gets a linked audio clip.

**Time**
- Seconds become `Tick` once, here.
- Video edges snap to the sequence frame grid with round-half-even.
- A clip that collapses to zero frames is dropped and reported.
- Linked audio keeps the unsnapped, sample-accurate edges.
- The Report counts the snaps and the maximum snap error.

**Tracks and properties**
- Video tracks come from `stack::assign` over the snapped spans in paint order (transition handles included).
- Audio clips go on audio tracks by the same rule.
- Transform: the static transform from `resolve` becomes Motion (position, scale, rotation, anchor). Skew is
  reported.
- Opacity, including inherited group opacity, becomes Opacity.
- `volume` and `mute` become clip volume and gain.

**Keyframes**
- `animate`/`key` on the mapped properties become keyframes in clip time, through resolve's time-base conversion.
- Interpolation belongs to the outgoing segment, and `defaultInterpolation` applies.
- A CSS bezier is mapped to the closest FilmCraft interpolation and kept apart from temporal and spatial handles.
- `additive`, `extrapolate` and roving keyframes are reported.
- Looped clips (several segments) carry their keys across each repeat.

**Report and diagnostics**
- Everything not mapped is counted in the Report with its element name and count. Nothing is dropped silently.
- `resolve` notes become Report entries.

## Overlays (B7, hybrid rendering)
For every **unmappable subtree root** that `resolve` lists (3D, particles, physics, simulation, expressions,
programs, and so on), produce an `Overlay`:

```rust
pub struct Overlay {
    pub item: ItemId,           // the fragment-local media item the overlay clip uses
    pub path: String,           // where the engine must write `document` (the media item's MediaRef::File path)
    pub document: Vec<u8>,      // a complete, valid scene document that renders only this subtree
}
```

**The overlay document**
- It is the **same scene version** and has the same project size, fps and duration.
- Its background is **transparent**, so the render has alpha.
- It carries the referenced `assets` and `defs` (everything the subtree references by id, transitively; nothing else).
- It holds the subtree itself at its original composition timing. Keep its ancestors' clocks and transforms:
  - write the ancestor groups as wrappers with the same time and transform attributes;
  - or, if you show it is exact, fold them.
- **Hard requirement:** it must pass `validate` with zero errors. Test that for every overlay fixture.

**The overlay's place in the sequence**
- The media item is `MediaRef::File { path }`. The path is
  `<base_dir>/<doc stem> Scene Overlays/<paint index>-<subtree id, sanitised to [A-Za-z0-9_-], ≤ 64 chars>.scene`,
  with the stem from `opts.name`.
  - Hash nothing here: the renderer's cache is content-addressed.
  - Sanitise against path traversal. Test it with an id such as `../x`; the XSD may forbid such ids, but the
    sanitiser must not rely on that.
- Its media info has the project size and fps, video with alpha, and the overlay's duration.
- **One clip** spans the subtree's composition interval (snapped) and takes part in `stack::assign` at the subtree's
  **paint position**. It therefore lands on the track above everything painted before it that overlaps it, and
  later-painted nodes go above it.
- A subtree that contains audio is noted in the Report: the overlay clip is video-only, and its audio is not mixed in
  this phase.

## Tests (TDD, all with hand-worked expected values)
**Clips and time**
- one video layer
- trims and speed
- a loop giving three clips
- a hold
- linked audio with an unsnapped edge, plus the snap counter and maximum error
- zero-frame drop
- 29.97 fps

**Picture and tracks**
- background matte (solid, gradient, transparent)
- the A/B/C stacking example becoming V1/V2/V3 (the background matte below them)
- the anchor conversion into Motion
- inherited group opacity
- keyframes in composition, local and normalized time bases
- keyframes on a looped clip

**Diagnostics**
- strict import refuses an invalid document with every error;
- lenient import keeps going and reports;
- lenient import never relaxes malformed XML or resource limits.

**Overlays**
- A `particles` (or other unmappable) subtree between two layers in paint order, checking:
  - the overlay clip's track and interval;
  - its document validates;
  - its background is transparent;
  - only the referenced assets are carried;
  - a node under a timeScale group keeps its timing.

**Unmapped elements and hostile input**
- The Report counts unmapped elements.
- Hostile input: huge counts, NaN, deep nesting and cycles return Err and never panic. Run proptest over random
  small documents built from a generator; they never panic.

---
## FIX ROUND 1 (this is a continuation; your previous work is uncommitted in this worktree, keep it)
Your S5 implementation is in the worktree (`git status`). A review found the issues below; all were checked against
the code and accepted. Fix each one **test-first**: write the failing test, show it red on the VM, then fix it.
Keep scope, rules and gates as above. Reply with the same schema, and include a red→green line per item.

1. **[P1] Unbounded allocation before scene limits** (`lib.rs` `import_with`): `decode_text(bytes)` runs before the
   `Format::Scene` dispatch, so an oversized input is copied (and maybe Latin-1-expanded) before the 64 MiB limit.
   Dispatch `Format::Scene` before `decode_text`.
2. **[P1] Keyframes use the wrong clock** (`properties.rs` ~205): you write `composition_time - clip.start`, but the
   renderer evaluates effects at **media time**: `TrackItem::effect_time_at` (`crates/project/src/lib.rs:643`,
   used by `crates/render/src/lib.rs:281`), which includes `source_in`, speed and reverse
   (`moving_source_time_at`). Map each key to the media time at which that clip will evaluate it (forward, speed,
   reverse, each loop segment). For holds: use FilmCraft's freeze the way the editor does (`frame_hold` with
   `hold_filters: false` and a **moving** effect clock, i.e. a non-zero speed for `moving_source_time_at`) so
   animation continues through a hold, unless the scene semantics say properties also freeze (check the XSD / plan
   and cite it). Test: `clipIn="2"` with a local opacity fade 0→2 s reaches its last key at the clip end, not its
   start; speed 2 and reverse variants; a hold with an animated opacity.
3. **[P2] Linked A/V drift from snapping**: the video start snaps but `source` stays anchored to the raw start.
   Adjust the video source position by `(snapped_start - raw_start) × signed rate` before deriving forward and
   reverse in-points. Test: 25 fps, `start=".02" clipIn="1" speed="2"`: video and linked audio read the same media
   time (within one tick of rounding) at timeline 0.04.
4. **[P2] Overlay serialisation breaks sequences inside symbols/defs** (`overlay.rs` ~213): every `<sequence>`
   becomes `<group>`, but replacement child timings exist only for resolved composition nodes. Keep complete
   (unpruned) sequences unchanged, including their transitions; rewrite only pruned wrappers whose child
   placements are resolved. Test: a symbol holding a 2-layer sequence, referenced from an overlay subtree.
5. **[P2] Missing media durations stay zero** (`import.rs` ~333): `Builder::finish` is skipped, so a video without
   `duration` (lenient) or audio without one (valid) keeps zero length. Fill unknown durations from the **checked**
   source-use bounds (max of source out, held frame + 1 tick), the way `Builder::finish` does but without unchecked
   arithmetic. Test both.
6. **[P2] Overlay closure misses fonts by family** (`overlay.rs` ~95): `font="Test"` / `fallback` on text (and
   transitively through text styles) refer to `<font family="Test">` assets by family name. Include them. Test: a
   pinned-font text inside an overlay validates (no C72).
7. **[P2] Overlay relocation rebases only `src`**: every schema-defined file-path attribute (e.g. `fontFile`,
   material/texture paths; find them all in the XSD and list them in your reply) must be rebased the same way,
   keeping Windows path handling. Test with `fontFile="fonts/test.ttf"`.
8. **[P2] Static `volume="0"` silences animated volume forever** (`properties.rs` ~210): keep permanent silence
   (gain -287.5 dB) for `mute` only; represent volume (including 0) through the volume parameter. Test: volume 0
   with keys rising to 1 is audible at the end.
9. **[P2] Linear volume fades change shape**: converting linear gain keys to dB while keeping linear interpolation
   turns amplitude-linear fades exponential (0→1 should be 0.5 halfway). Preserve the amplitude curve (e.g. sample
   to extra keys within a bounded count and a stated tolerance), or, if the volume param is linear, write linear
   values. Report any approximation. Test the halfway value.
