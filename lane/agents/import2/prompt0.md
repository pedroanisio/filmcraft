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
