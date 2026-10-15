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


# FIX ROUND 1 (continuation of your task above)

Your round-0 work is still uncommitted in your worktree; keep it. The integrator added the one-line `Cargo.lock` refresh (filmcraft-vpkg in the engine deps); leave Cargo.lock as it is. An independent read-only review returned BLOCK. Verify each finding yourself; fix every one you confirm, red-first (a failing test before the fix, as before). If you reject a finding, say why with evidence. The RU/UK fix (finding 4) extends your scope to `crates/ui-egui/src/i18n/{ru,uk}.tsv`; findings 1-3 may touch `crates/engine/src/interchange.rs` and `lib.rs` within your existing scope. Keep the public command API unless a fix requires a change, and say so.

1. **P1 — Hybrid import can write outside the extraction directory.** [vpkg.rs:100](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:100), [interchange.rs:319](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/interchange.rs:319)  
   **Input:** A valid package containing `main.scene` with a `particleEmitter`, imported with explicit `dir="/tmp/out"`. Before import, `/tmp/out/project/main Scene Overlays` is a symlink to `/tmp/outside`.  
   **Why:** Archive-entry checks never inspect the generated overlay directory. Hybrid import subsequently calls `write_overlay`, whose directory creation and ordinary file write follow that symlink. Overlay files are written outside `/tmp/out`.  
   **Minimal fix:** Validate generated overlay destinations against the extraction root and write them through `write_package_file`; preflight these destinations before extraction.

2. **P2 — Import opens paths different from those verified.** [vpkg.rs:100](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:100)  
   **Input:** A packaged image reference `src="assets/a%20b.png"` with `project/assets/a b.png` present. More seriously, `src="assets/pixel.png?x/../../../../outside.png"` with the verified `project/assets/pixel.png` present.  
   **Why:** Package verification percent-decodes references and removes query/fragment suffixes. The scene importer instead treats those strings as filesystem paths. The first reference becomes offline—or loads different bytes if the literal `%20` filename exists. With extraction at `/tmp/out`, the second resolves to `/tmp/outside.png`, opening an unverified external file.  
   **Minimal fix:** Use matching URI-resolution rules when importing verified scene references, preserving the image-sequence pattern exception, and check containment on the resulting paths.

3. **P2 — Rendered import is not always one undo step.** [vpkg.rs:100](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:100)  
   **Input:** Enable project ingest with action `Copy` and a writable destination, then import a valid package with `"mode":"rendered"`.  
   **Why:** `import_scene` dispatches `file.import`. That creates one import edit, then ingest copies and relinks the scene through another `s.edit`. One undo reverses the relink but leaves the imported item.  
   **Minimal fix:** Group the synchronous import and ingest edits into one undo transaction, or use a rendered-import path that bypasses automatic ingest.

4. **P2 — Russian and Ukrainian lack both new menu translations.** [commands.rs:1356](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/commands.rs:1356)  
   **Input:** Select interface language `ru` or `uk`, then open File → Import/Export.  
   **Why:** Neither catalog contains `Scene Package…` or `Scene package (.vpkg.zip)`; both labels fall back to English. The other nine catalogs contain them.  
   **Minimal fix:** Add both keys to `ru.tsv` and `uk.tsv`.

5. **P3 — Failed extraction leaves an incomplete file without recovery context.** [lib.rs:339](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/lib.rs:339), [vpkg.rs:97](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:97)  
   **Input:** Disk fills partway through writing a package asset, after earlier entries succeeded.  
   **Why:** `write_all` returns immediately, leaving the newly created truncated file. The error reports only the archive-relative filename and OS error—not the extraction directory or leftovers. Retrying with explicit `dir` then fails the different-content check. The documentation acknowledges leftovers, but the runtime error does not.  
   **Minimal fix:** Remove the newly created incomplete file on failure and report the extraction directory and any retained files.

Read-only review; no Cargo or runtime tests run. Static inspection confirms `engine` L4 → `vpkg` L3 is allowed by the layer checker.

BLOCK

For finding 2, follow the reference rules in /home/admin/src/sr-core/sr_core/vpkg/ (refs.py) and the scene importer's existing source resolution; a resolved path outside the extraction root must be an error (or an offline/report entry consistent with strict vs lenient), never an open. For finding 3, prefer one undo transaction over bypassing ingest if the existing edit API supports grouping.

Finish with the full VM validation green as before (fmt, workspace clippy -D warnings, engine + ui-egui + vpkg + interchange tests, xtask layers, xtask wasm) and reply with the usual handoff.


# FIX ROUND 2 (continuation of your task above)

Your round-0 and round-1 work is still uncommitted; keep it, and leave Cargo.lock alone. A second independent review returned BLOCK. Its full text follows; the integrator's decisions on each finding come after it.

Found six remaining defects. Review was static against `94bea42e`, including untracked files; no files modified, Cargo or runtime tests run.

1. **P1 — Extraction containment remains vulnerable to a symlink race.** [lib.rs:349](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/lib.rs:349)  
   **Input:** Import a particle-emitter package into `/tmp/out`. After the final `package_path` check, replace `project/main Scene Overlays` with a symlink to `/tmp/outside`.  
   **Why:** `create_new(true)` protects the final filename, but follows parent-directory symlinks. The subsequent pathname-based open writes the overlay outside the extraction root. Archive entries and derived scenes have the same vulnerability. Rechecking before opening does not eliminate this window.  
   **Minimal fix:** Perform directory traversal and exclusive creation through a containment-enforcing directory capability, refusing links during the actual filesystem operation.

2. **P1 — Partial-write cleanup can delete another file.** [lib.rs:323](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/lib.rs:323)  
   **Input:** While a newly created package file is being written, another process renames that file away and moves a pre-existing valuable file into its pathname. The write or sync then fails.  
   **Why:** Cleanup closes the original handle and unconditionally removes the pathname, deleting the replacement. Exclusive creation establishes ownership only at creation time. The regression test covers an uncontended failure only.  
   **Minimal fix:** Write into operation-owned staging storage and publish with atomic no-clobber semantics after successful completion. Cleanup must never unlink a replaceable public destination merely because its pathname matches.

3. **P2 — Included documents bypass reference containment.** [vpkg.rs:111](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:111)  
   **Input:** The primary scene includes `parts/part.scene`; that document references `generated/pixel.png`, declared in `fetch` but omitted from the archive. Before import, `/tmp/out/project/parts/generated` is a symlink outside the extraction directory. Import in rendered mode.  
   **Why:** Verification accepts the declared missing input. Normalization checks only the primary document’s include destination, and extraction checks only packaged files. Nothing checks the included document’s reference against the existing filesystem. Rendering subsequently follows the symlink and reads external, unverified content.  
   **Minimal fix:** Walk the reachable document/dependency graph with bounded work and a seen-set, applying reference resolution and containment to every dependency before extraction or rendering.

4. **P2 — The reference classifier omits inputs consumed by the renderer.** [vpkg.rs:216](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:216)  
   **Input:** A valid scene containing a map backed by `<tiles id="t" src="../../outside.pmtiles"/>`, imported into `/tmp/out` in rendered or hybrid mode.  
   **Why:** Both the existing package verifier and this copied classifier omit `tiles@src`. The schema identifies it as a local file read during rendering. Normalization therefore leaves it unchecked; hybrid overlay serialization can make it an absolute external path, which the second rewrite also ignores. `tiles@cache`, `volume@src`, and file-valued `basemap@mapStyle` expose the same coverage gap.  
   **Minimal fix:** Classify every file input supported by the renderer, including schema-specific exceptions, and reject any unsupported input before handing the document to it.

5. **P2 — Ordinary native/hybrid imports fail on Windows.** [vpkg.rs:205](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:205)  
   **Input:** Import the ordinary packaged PNG fixture with `dir="C:\\out"` and `mode="native"`.  
   **Why:** `native_package_path` obtains a canonical root such as `\\?\C:\out`. Normalization converts media paths to file URLs; `path_to_file_url` deliberately removes the verbatim prefix. The scene importer consequently produces `C:/out/project/assets/pixel.png`. `Path::strip_prefix` does not equate ordinary and verbatim Windows prefixes, so containment rejects this legitimate path.  
   **Minimal fix:** Use a consistent Windows path representation for containment comparisons, preserving the filesystem safety checks.

6. **P3 — Generated file/directory conflicts escape preflight.** [vpkg.rs:154](/home/admin/src/filmcraft-wt/vpkgengine/crates/engine/src/vpkg.rs:154)  
   **Input:** A valid particle-emitter package also contains an ordinary file named `project/main Scene Overlays`; the extraction directory starts empty.  
   **Why:** The destination map detects only identical pathname collisions. Both that file and the generated overlay beneath it pass filesystem preflight because neither exists yet. Extraction writes the metadata and packaged files, then fails creating the overlay directory, leaving a predictably incomplete extraction.  
   **Minimal fix:** Validate the combined archive/generated destination set for file-versus-directory prefix conflicts and platform-equivalent names before writing anything.

The synchronous undo grouping and Russian/Ukrainian translations appear correctly fixed. `edit_group` restores project, editor state, and history on a returned error. Exact-name derived-scene collisions are checked, and ordinary partial-write errors now report extraction context, but the races above prevent considering those fixes complete.

Native and web package reads enforce byte limits; export naming is deterministic and ordinal-prefixed, and rewriting covers the `src` references currently emitted by the scene exporter. The `engine` L4 → `vpkg` L3 dependency is allowed by the layer table. These are static conclusions, not an oracle-verification result.

BLOCK

## Integrator decisions (binding)

- **Finding 1 (concurrent symlink race): do not fix in code.** A concurrent local attacker who swaps directories inside the user's chosen extraction directory during the import is outside FilmCraft's threat model. Safe std Rust has no openat, and a capability crate would be a new dependency that needs owner approval. Instead, document the limitation in one place: a short comment at the write path and a sentence in docs/control-protocol.md. Keep every existing pre-write check.
- **Finding 2 (cleanup deletes by pathname): fix.** Never unlink a public destination pathname. Write each file to an operation-owned staging name in the same directory (created with create_new; make the name unpredictable and collision-checked; bounded retries). Then publish without clobbering: `std::fs::hard_link(staging, dest)` fails if dest exists. After that, remove the staging name. If hard links are unsupported (an error kind you can detect), fall back to a documented best effort, and still never delete the destination pathname. On failure, remove only your staging name. Add a red-first regression test that a pre-existing file at the destination is never removed or overwritten.
- **Finding 3 (included documents): fix.** Walk the reachable include/dependency graph with a seen-set and the existing budgets. Apply the same reference resolution and containment to every document's references before any extraction or rendering. Red-first test.
- **Finding 4 (unclassified renderer inputs): fix in the engine.** Derive the complete set of file-valued attributes from the vendored XSD (`crates/interchange/schema/sr-core/scene-render.xsd`: every attribute documented as a local file or URI read by the renderer, including tiles@src, tiles@cache, volume@src and file-valued basemap@mapStyle). Classify all of them. Reject any file input you cannot resolve and contain. Add a parity test that fails if the XSD gains a file-valued attribute the classifier does not know. Do not edit crates/vpkg (another owner's crate). Instead, list in your reply the verifier gaps you found, and note whether sr-core refs.py has the same gap, so the integrator can file them.
- **Finding 5 (Windows verbatim prefix): fix.** Compare paths in one consistent representation. Add a unit test of the comparison helper using Windows-style strings, so it runs on Linux CI (it must not depend on the host OS).
- **Finding 6 (file vs directory prefix conflicts): fix.** Validate the combined archive and generated destination set before writing anything, covering prefix conflicts and case- or platform-equivalent names. Red-first test.

Finish with the full VM validation green, as before, plus the sr-core Python oracle, and reply with the usual handoff.
