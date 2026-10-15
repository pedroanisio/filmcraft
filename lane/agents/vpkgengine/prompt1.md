# You are a member of the FilmCraft scene-interchange team (agent `vpkgengine`)

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
- Your worktree is `/home/admin/src/filmcraft-wt/vpkgengine`, on git branch `b/vpkgengine`. Other agents work in parallel in
  their own worktrees.
- **Only edit the files in your scope** (listed in your task). If you need a change outside it, describe it in your
  reply instead of making it.
  - `scene/mod.rs` is shared: add only your `pub mod …;` and `pub use …;` lines to it.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-vpkgengine` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh vpkgengine cargo test -p filmcraft-interchange scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh vpkgengine sh -c 'cargo clippy -p filmcraft-interchange --all-targets -- -D warnings && cargo test -p filmcraft-interchange && cargo xtask wasm'

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

**S10 (part 2): the engine commands `file.importVpkg` and `file.exportVpkg`.** The crate `filmcraft-vpkg` (`crates/vpkg`, L3, pure, in memory) already reads, verifies and packs scene video packages. Read its README.md and `src/lib.rs` first. This task wires it into the engine, following the existing scene commands: `import_scene`, `write_overlay` and `export` in `crates/engine/src/interchange.rs`, and their `cmd!` entries in `crates/engine/src/commands.rs` (`file.importScene`, `file.exportScene`). The reference is sr-core's `sr_core/vpkg/{pack,archive,init}.py` (`/home/admin/src/sr-core`, MIT; the owner wrote it and grants borrowing, with provenance noted in ATTRIBUTION.md for anything you adapt).

## Behaviour
- `file.importVpkg {path:str, dir:str?, mode:"rendered|native|hybrid"=hybrid, lenient:bool=false}`
  1. Read the package through `Services` (never `std::fs` directly). Check its size against the vpkg `Limits` before reading.
  2. Run `filmcraft_vpkg::read`. If verification fails, return an error with the vpkg diagnostics in the JSON, the same shape as `EngineError::InvalidScene`; generalise that variant or add a sibling, whichever is cleaner. **Write nothing until verification has succeeded** (SREP 6).
  3. Extract into `dir`. The default is a sibling folder named after the package stem; refuse a default that would collide with an existing non-empty folder. Every path is checked with `filmcraft_vpkg::zip::safe_path` and must stay inside `dir`. Refuse to overwrite a different existing file, as `write_overlay` does. If any write fails, the project is not touched.
  4. Import the extracted primary scene through the same code path as `file.importScene` (same mode and lenient semantics), so the assets resolve relative to it.
  5. Return the scene import result plus `{"package": {id, version, title, primaryScene, extractedTo, warnings:[…]}}`.
- `file.exportVpkg {path:str, sequence:id?, id:str?, version:str?, title:str?}`
  1. Export the sequence as a scene (same code as `file.exportScene`).
  2. Collect the media files it references through `Services`. Rewrite the absolute or outside paths to package-relative `assets/<name>`, with names made unique deterministically.
  3. Run `pack`, then write the archive via `Services::write_file`.
  - A missing media file is an error naming the file, never a silent omission.
  - `SOURCE_DATE_EPOCH` is not read by the engine: pass `None`, or take it from a param if you need it for tests.
  - Defaults: `id` from the sequence name (slugified), `version` "1.0.0", `title` the sequence name.
- Menus: File ▸ Import ▸ "Scene Package…" and File ▸ Export ▸ "Scene package (.vpkg.zip)". Add the open/save dialog filters as the scene ones do (`crates/ui-egui/src/lib.rs` `EXPORT_SAVE_DIALOGS`, `panels/file_dialogs.rs`).
  - **Every new menu label must be translated** in `crates/ui-egui/src/i18n/{es,zh-cn,ja,pt-br,fr,de,it,ko,id}.tsv`, next to the existing `Scene…` / `sr-core scene (.scene)` rows. `cargo test -p filmcraft-ui-egui` enforces this for es, zh-cn, ja and pt-br.
  - Any new Help item needs a hover hint in `links.rs`.
- Add both commands to `docs/control-protocol.md`, where the scene commands are documented.
- Never crash: hostile params (wrong types, unknown keys, absurd sizes, a `dir` with `..`) get `BadParams`. Use checked arithmetic and no indexing.

## Tests (TDD: red first)
- Engine tests in a new `crates/engine/src/vpkg_tests.rs`, using in-memory services as the existing `scene_interchange_tests.rs` does:
  - import of the three fixtures in `crates/vpkg/tests/fixtures/`
  - a tampered package (bad hash): no file is written and the project is unchanged
  - path traversal
  - an existing different file
  - export → import round trip, preserving the clip count and the media bytes
  - a missing media error
  - hostile params
  - one undo step per import
- Opt-in oracle, required in no CI: with `SR_CORE_DIR` set and `python3 -c 'import lxml'` working, run `python3 -m sr_core.vpkg verify <exported.vpkg.zip>` (`PYTHONPATH=$SR_CORE_DIR`, with a 120 s deadline as in `crates/interchange/tests/sr_core_corpus.rs`). Otherwise print "skipped".

## Scope (you may edit)
- `crates/engine/src/{interchange.rs,commands.rs,lib.rs}`, a new `crates/engine/src/vpkg.rs`, `vpkg_tests.rs`, and `crates/engine/Cargo.toml` (add the workspace crate `filmcraft-vpkg` only; no external dependency).
- `crates/ui-egui/src/{lib.rs,menus.rs,links.rs,panels/file_dialogs.rs}` and the i18n tsv files listed above.
- `docs/control-protocol.md`, `ATTRIBUTION.md` (provenance only).
- Run `cargo xtask layers`: the engine (L4 or L5) may depend on vpkg (L3).

## Gate (VM)
`cargo fmt --check && cargo clippy --workspace --all-targets -- -D warnings && cargo test -p filmcraft-engine -p filmcraft-ui-egui -p filmcraft-vpkg && cargo xtask layers && cargo xtask wasm`. Also run the oracle once with `SR_CORE_DIR=$HOME/sr-core`.

Reply HANDOFF with: the API and param shapes, the red→green log, the oracle result, any borrowed code, and the gate result.
