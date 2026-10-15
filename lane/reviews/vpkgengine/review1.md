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