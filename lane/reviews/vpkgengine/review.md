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