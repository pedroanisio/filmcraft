# You are a member of the FilmCraft scene-interchange team (agent `export`)

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
- Your worktree is `/home/admin/src/filmcraft-wt/export`, on git branch `b/export`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-export` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh export cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh export sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

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
**S7 (export half): `scene::export`, a FilmCraft sequence as an sr-core scene document at the lowest version that
holds it.**

Scope (you may edit):
- `crates/interchange/src/scene/export.rs`, or `scene/export/**`, and its test module.
- New fixtures under `crates/interchange/tests/fixtures/scene/export/`.
- A new integration test `crates/interchange/tests/scene_export.rs`.
- `scene/mod.rs`: your `pub mod` / `pub use` lines only.
- `crates/interchange/src/lib.rs`: **only** the minimum to reach your exporter:
  - a `Format::Scene` variant (name "sr-core scene", extension "scene");
  - its arm in `Format::ALL`/`name`/`extension`/`from_extension`;
  - sniffing in `detect` for a `<scene version="…">` root;
  - the `export()` dispatch arm.
  - Import dispatch for `Format::Scene` returns
    `Err(Error::Other("sr-core scene import is not implemented yet".into()))`. Another agent is writing the
    importer.
  - Check every `match` on `Format` in the workspace (`grep -rn "Format::" crates/`). Wherever a new variant
    breaks a match outside your scope (engine, ui-egui, cli), list the file and line in your reply instead of
    editing it. Do not add a wildcard arm to make it compile. Also tell me if the engine's export or import menus
    would now list `Scene` automatically.

**Another agent is writing `scene::resolve` (S4) in parallel.** Do not depend on it. The round-trip tests
(export → import) come later, after the importer exists. Write your exporter so it can be read back with the
semantics in the plan's "Semantic resolution" section.

## Entry point
Follow the shape of `fcpxml::export` / `otio::export`:
`pub(crate) fn export(p: &Project, seq_id: ItemId, opts: &ExportOptions, report: &mut Report) -> Result<String>`.
If the scene needs options (for example, a forced minimum version), add them to `ExportOptions` with a default that
changes nothing for other formats.

## Mapping
This is the inverse of the plan's import table. Read `filmcraft_project` (sequence, tracks, clip items, media,
keyframes, graphic, caption, markers, transitions, mixer) and the scene XSD to choose the exact elements and
attributes.
- **Sequence settings:** `project` width, height, fps (rational, e.g. `30000/1001`, where the rate is not an integer)
  and duration.
- **Media:** `assets` video, image and audio. The paths come from the media refs, relative when `opts` gives a base,
  the way the other exporters do it. Each asset is written once even when several clips use it.
- **Track order:** clip items become layers in paint order. A lower video track paints first; audio tracks become
  audio layers. Use explicit `start`/`end` in seconds.
  - Write seconds exactly: decimal text that `scene::values::seconds` parses back to the **same `Tick`**. Prove it
    with a property test over ticks on common frame grids (24, 25, 30000/1001, 48000 Hz). If plain decimal cannot
    round-trip, find the representation that does.
- **Trims and speed:** clipIn/clipOut from the source in-point, plus speed and reverse.
- **Motion:** position, scale, rotation and anchor become a transform. Do the anchor conversion explicitly and test
  it with a hand-worked fixture.
- **Basic effects:** opacity becomes opacity; volume and gain become volume (with mute when muted).
- **Keyframes:** they become `animate`/`key` with `timeBase` set explicitly, plus the interpolation mapping.
  Interpolation belongs to the outgoing segment, as the scene format defines it.
- **Graphics:** text graphic layers become text, and shape layers become shapes.
- **Transitions:** track transitions become `transition`, through a type table. Unknown types are reported and
  written as a cross dissolve, or skipped (your call, but say which).
- **Markers and captions:** sequence markers become `markers`; caption tracks become captions.
- **Nested sequences** become groups.
- **Everything else:** effects without an equivalent, masks, multicam, adjustment layers, and so on. Count them in
  the Report and drop them. Never fail the export for them.

## Version selection
The output declares the **lowest** `scene/@version` (1.0 to 1.7) that allows every element and attribute it writes.
- Derive the gates from the Schematron version rules (`V*` in `scene-render.sch`) and the XSD version notes. Build a
  table of gated constructs that your exporter can emit, with their minimum versions.
- **Version-gate tests:** for each gated construct you can emit, export a sequence that needs it, then check:
  - the declared version is exactly its minimum;
  - `scene::validate` passes;
  - the same document with the version decremented fails `validate` with that gate's rule.
- A sequence with only plain clips exports as `1.0`.

## Tests (required)
- **Every exporter test** reads the output back with `scene::read` and runs `scene::validate`: zero errors.
- **Expected-value fixtures:** small FilmCraft sequences built in the test, each with the exact expected XML (or the
  exact expected attribute values) worked out by hand. Cover:
  - one clip
  - a trimmed clip at speed 2
  - two tracks overlapping, for paint order
  - motion with an anchor
  - opacity keyframes with ease
  - a cross dissolve
  - markers
  - a caption track
  - text and a rect
  - a nested sequence
  - the 29.97 fps rate
- **Hostile:** an empty sequence, a missing sequence id gives `Error::NoSequence`, absurd clip values (huge ticks,
  zero-length clips, speed 0) never panic, and names and paths that need XML escaping (`&<>"'`, control
  characters) are escaped and survive `read`.
