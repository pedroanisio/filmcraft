1. **P1 — Warning skips also suppress validity checks.** [sr_core_corpus.rs:257](/home/admin/src/filmcraft-wt/corpus/crates/interchange/tests/sr_core_corpus.rs:257)  
   `skip_reason` removes the entire expectation. The pinned corpus’s `srep-0073-default-no-finding` expects `valid: true` and absence of `INERT-I16`; any ordinary strict-import error is nevertheless counted as an expected refusal.  
   **Minimal fix:** skip only unsupported diagnostic assertions, preserving independently applicable validity checks.

2. **P1 — Successful lenient imports never reach export.** [sr_core_corpus.rs:249](/home/admin/src/filmcraft-wt/corpus/crates/interchange/tests/sr_core_corpus.rs:249)  
   The lenient result is checked for panic and, when strict succeeds, refusal. Its imported project is discarded. If strict correctly rejects an invalid case but lenient recovers a project that makes export panic, this test passes without exercising that panic.  
   **Minimal fix:** run `roundtrip` on each successful import mode and aggregate failures with mode labels.

3. **P1 — The ignored sibling regression can pass without exporting.** [sr_core_corpus.rs:423](/home/admin/src/filmcraft-wt/corpus/crates/interchange/tests/sr_core_corpus.rs:423)  
   The selected `10000-siblings` case has `expected: None`. If import starts returning an ordinary error, `pipeline` counts it as an expected refusal, and the ignored export regression passes vacuously.  
   **Minimal fix:** require successful import for this case, assert 10,000 imported clips, and require export/re-import completion.

4. **P1 — Known-error substrings suppress additional failures.** [sr_core_corpus.rs:528](/home/admin/src/filmcraft-wt/corpus/crates/interchange/tests/sr_core_corpus.rs:528)  
   Overlay validation combines multiple diagnostics into one error string. A material-overlay failure containing both the known `error[MOV2]` and a new unrelated diagnostic is discarded wholesale. Matching also does not constrain the failing stage. The panic exclusion works, but new ordinary failures can disappear.  
   **Minimal fix:** match the expected stage and complete normalized diagnostic set; preserve every additional diagnostic as a failure.

5. **P2 — Python checks have no execution deadline.** [sr_core_corpus.rs:234](/home/admin/src/filmcraft-wt/corpus/crates/interchange/tests/sr_core_corpus.rs:234)  
   Both Python invocations use blocking `.output()`. A stalled interpreter or schema check hangs the suite indefinitely; the advertised 120-second budget only appears in assertion text and enforces nothing.  
   **Minimal fix:** spawn with a deadline, kill and reap timed-out children, and aggregate a timeout failure. A hard deadline for importer hangs requires process isolation too.

No fixture-provenance finding: all nine fixtures contain original-work/license declarations; `.scene` is outside `xtask`’s asset extension list, consistent with existing text-fixture conventions. No files changed and no Cargo commands run.

**Verdict: BLOCK**