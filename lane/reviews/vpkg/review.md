Found five defects. Review was read-only; no Cargo commands or Rust tests were run. Python probes confirmed the reference behavior described below.

1. **P1 — ZIP filename encoding flag is ignored.** [zip.rs:199](/home/admin/src/filmcraft-wt/vpkg/crates/vpkg/src/zip.rs:199)  
   **Input:** An otherwise valid package whose local and central filename bytes are `project/\xC3\xA9.scene`, with UTF-8 flag `0x0800` clear, and whose manifest lists `é.scene`.  
   **Why:** The reader unconditionally decodes UTF-8 and accepts the manifest/name match. Python’s ZIP reader decodes the filename as `project/├⌐.scene`, so the listed scene is actually absent under the reference implementation’s interpretation.  
   **Minimal fix:** Reject non-ASCII filenames without the UTF-8 flag, or decode unflagged names using the ZIP legacy encoding consistently.

2. **P1 — Uppercasing misses Unicode case collisions.** [zip.rs:108](/home/admin/src/filmcraft-wt/vpkg/crates/vpkg/src/zip.rs:108)  
   **Input:** A package containing both `project/ß.png` and `project/ẞ.png`, correctly listed and hashed.  
   **Why:** Their uppercase keys are `PROJECT/SS.PNG` and `PROJECT/ẞ.PNG`, so both pass. Their Unicode casefold keys are identical. The package therefore passes the advertised collision protection while retaining names that alias under Unicode caseless comparison.  
   **Minimal fix:** Use full Unicode casefolding for collision keys and parent-prefix checks. Add this pair to the regression cases.

3. **P2 — Valid XML declarations break included documents and MaterialX files.** [verify.rs:284](/home/admin/src/filmcraft-wt/vpkg/crates/vpkg/src/verify.rs:284), also line 361  
   **Input:** A referenced `inc.xml` containing:
   ```xml
   <?xml
   version="1.0"?>
   <assets><data src="data.json"/></assets>
   ```
   with `data.json` packaged.  
   **Why:** Declaration removal recognizes only `<?xml ` with a literal space. A valid newline or tab leaves the declaration inside the synthetic `<scene>` wrapper, where XML parsing rejects it. Python’s reference parser accepts the original document.  
   **Minimal fix:** Recognize all XML whitespace after `<?xml`, and share the bounded declaration-removal helper between includes and MaterialX.

4. **P2 — Image-sequence matching differs from `refs.py`.** [verify.rs:706](/home/admin/src/filmcraft-wt/vpkg/crates/vpkg/src/verify.rs:706)  
   **Input:** `<imageSequence src="frame_%02d.png"/>` with packaged `frame_٠١.png`—two Arabic-Indic decimal digits—and no fetch or producer.  
   **Why:** `refs.py` uses Unicode-aware `\d` and finds the frame. Rust uses `is_ascii_digit()`, finds nothing, and rejects the package. The same mismatch affects `#` patterns.  
   **Minimal fix:** Match Unicode decimal digits with the same semantics as Python `\d`.

5. **P2 — Written packages omit the embedded schema’s MIT notice.** [write.rs:154](/home/admin/src/filmcraft-wt/vpkg/crates/vpkg/src/write.rs:154)  
   **Input:** Any successful minimal `pack()` call.  
   **Why:** The output includes the upstream MIT schema, but neither that schema nor the generated README carries its copyright and permission notice. The repository’s sibling `LICENSE` file does not accompany the generated ZIP.  
   **Minimal fix:** Include the upstream MIT notice in the generated README before computing `metaSha256`.

No additional defect found in EOCD scan bounds, descriptor-flag/CRC/size cross-checks, actual-output inflate limits, overlap rejection, symlink rejection, ZIP64 rejection, required metadata hashes, or deterministic ordering/timestamps. The new source contains no hostile-data `[i]` indexing; the inspected `as` casts are widening or locally range-checked.

**Verdict: BLOCK**