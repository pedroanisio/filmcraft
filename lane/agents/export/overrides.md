
## Follow-up from the integrator (this overrides anything above)
Your previous run is already in this worktree (`scene/export.rs`, `scene/export/tests.rs`, the `lib.rs` and
`mod.rs` changes). You replied BLOCKED on time precision. Here is the decision; continue from the existing code.

**Policy: the exporter never drops content for float precision.**
- `seconds(t)` stops failing on precision. It returns the text plus whether it is exact, e.g.
  `fn seconds(t: Tick) -> Result<Secs>` with `struct Secs { text: String, error: i64 }`, where `error` is
  `values::seconds(&text)? - t` in ticks (0 when exact). It is `Err` only for a tick outside the reader's range.
  - Exact case: unchanged (shortest decimal that parses back to the same Tick).
  - Inexact case: among the bracketing doubles (`s`, `next_down`, `next_up`), pick the one whose
    `values::seconds_to_tick` is nearest to `t` (ties: the lower one), and write its shortest round-trip text.
- The writer accumulates inexact times: a count and the maximum absolute tick error. At the end it adds **one**
  Report warning: `"N time(s) beyond exact f64 tick precision written to the nearest representable value (max
  error K ticks)"`. No per-site warnings for precision.
- Remove every precision-motivated skip: `window()`, the media duration check (~line 321), animation keys
  (~463), source span/time (~565-569), the nested clock (~618), transition duration (~695), and any others. Keep
  the skips that are about **invalid** values (negative start, zero/negative duration, beyond `MAX_TIME`, speed 0
  where it is invalid, and so on).
- Guard against a nearest-value collapse: if a clip's written start and end parse back to `end <= start`, that
  clip is skipped with a reported warning. Show in a test that this cannot happen for any frame-grid clip of at
  least one 48 kHz sample within `MAX_TIME` (max f64 error at 1e6 s is a few dozen ticks; one sample is 5,292,000).

**Tests (TDD: change the tests first, see them fail on the VM, then fix):**
- `scene_export_seconds_on_common_grids`: keep it exact-only. Bound it to where f64 is exact on every grid: an
  absolute time under 2 h (`|t| < 7200 s`). Assert `error == 0`.
- Replace `scene_export_full_duration_exact_or_reported` with a proptest over the whole range up to `MAX_TIME` on
  the four grids. Assert:
  - the clip is always written (a `layer` is present);
  - `|parsed - t|` is at most 64 ticks;
  - the report mentions `"beyond exact f64 tick precision"` exactly when some written time was inexact.
- `scene_export_large_frame_precision_regression` (32768 + 1/24 s): assert the clip **is** written, its `start`
  parses back to within 1 tick of the target, and the report has the single precision warning with count and max
  error.
- `scene_export_unrepresentable_tick_is_explicit` (2^53 + 1 ticks): `seconds` returns `Ok` with `error != 0`.
- Keep everything else green: `cargo fmt --check`, clippy with `-D warnings`, `cargo test -p filmcraft-interchange`,
  and `cargo xtask wasm`, all on the VM through `vm-agent.sh @NAME@`.

Reply HANDOFF with what changed (functions, tests, the count of former skip sites removed), or BLOCKED with the
precise reason.
