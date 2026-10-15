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
