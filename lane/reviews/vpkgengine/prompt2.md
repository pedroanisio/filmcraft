Review the uncommitted diff (git diff plus untracked files) in this worktree, on top of commit 94bea42e. Read-only: do not modify files, do not run cargo.

The work adds two engine commands, `file.importVpkg` and `file.exportVpkg` (new `crates/engine/src/vpkg.rs`, `vpkg_tests.rs`, edits to engine `Cargo.toml`, `commands.rs`, `interchange.rs`, `lib.rs`), UI menu entries (`crates/ui-egui/src/{lib.rs,menus.rs}`, i18n tsv files) and `docs/control-protocol.md`. Import verifies a scene video package (.vpkg.zip) with the existing `filmcraft-vpkg` crate, extracts it to a directory (default: the package stem next to the archive), then runs the existing scene importer on the primary scene, with one undo step. Export runs the existing scene exporter, copies the sequence's media into `assets/<ordinal>-<name>`, rewrites sources, and packs with `filmcraft-vpkg`. File I/O goes through `Services` hooks (new: read_file_bounded, package_dir_empty, package_path, create_parent_dirs, write_package_file; FsServices implements them).

A previous review returned these findings; the author says all five are fixed (with regression tests named vpkg_fix_*):

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


Verify each fix is correct and complete (not just the exact input given). In particular: overlay destination preflight cannot be raced or bypassed by any generated path; the URI rules match /home/admin/src/sr-core/sr_core/vpkg/refs.py _value_path and every reference the scene importer opens goes through containment; the derived <primary>.filmcraft.scene cannot overwrite a verified file or escape; Session::edit_group really yields one undo step and cannot leave a half-applied edit on error; cleanup of partial writes never deletes a pre-existing file. Then check the whole diff again for new defects with the same rules and ranking as below.

Repo rules: AGENTS.md (never crash: no unwrap/expect/panic/indexing on hostile data outside tests, checked math, capped allocations, bounded loops; layering per docs/architecture.md; everything is a command with params validated as hostile). Spec references: /home/admin/src/sr-core/srep/srep-0001.md, srep-0006.md; reference impl /home/admin/src/sr-core/sr_core/vpkg/*.py.

Find real defects only, ranked P1 (crash/panic, unbounded memory or time on hostile input, writes outside the target directory: path traversal, absolute paths, symlinks, Windows device names or drive prefixes, project mutated after a failed import, wrong bytes written, a package that exports but fails `sr_core.vpkg verify`) > P2 (spec divergence that changes accept/reject, non-deterministic export, undo not a single step, error not surfaced through EngineError/diagnostics, missing i18n entries or automation ids, layering violation) > P3. For each: file:line, concrete failing input, why, minimal fix. Specifically check: every path from the archive or from params before it touches the filesystem; collision/identical-content logic; the size caps on reads; asset name sanitisation and uniqueness; source rewriting covers every media reference the exporter emits; whether partial extraction is left behind on failure and whether that is reported; Cargo.toml dependency is allowed by `cargo xtask layers`. End with a verdict line: SHIP or BLOCK.
