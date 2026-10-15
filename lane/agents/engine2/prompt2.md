# You are a member of the FilmCraft phase B team (agent `engine2`)

FilmCraft is a pure-Rust video editor. Phase B makes it render sr-core scene documents (`.scene`) by spawning the
external `scene-render` program. Read these first:
- `AGENTS.md` and `CLAUDE.md` in your worktree. Their rules are binding.
- `docs/architecture.md`.
- The phase B plan: `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/PLAN-B.md`.

The crate you work in is `crates/scenerender` (`filmcraft-scenerender`, layer L3). Its skeleton is committed:
`lib.rs` declares the modules and `Quality`, and each module file holds only its doc comment.

## Your environment
- Your worktree is `/home/admin/src/filmcraft-wt/engine2`, on git branch `b/engine2`. Other agents work in parallel on
  other modules of the same crate in their own worktrees.
- **Only edit the files in your scope** (listed below). If you need a change outside it, describe it in your reply
  instead of making it.
  - `lib.rs` is shared: add nothing to it except `pub use` lines or a `#[cfg(test)] mod …;` line for your own modules.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-engine2` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh engine2 cargo test -p filmcraft-scenerender
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh engine2 sh -c 'cargo clippy -p filmcraft-scenerender --all-targets -- -D warnings && cargo test -p filmcraft-scenerender'

  The first run builds the dependencies cold and takes several minutes; later runs are quick. Never launch two VM runs
  at the same time.
- The scene-render source is at `/home/admin/src/scene-render` (read-only). It is the truth for its CLI flags and
  output (`crates/scene-render/src/main.rs`). Its owner allows borrowing code from it, but adapt anything you borrow
  to FilmCraft's rules.

## How to work: strict TDD
For every behaviour:
1. Write the test first.
2. Add a minimal stub so it compiles, for example a function returning a wrong value.
3. Run it on the VM and **see it fail**.
4. Implement.
5. Run it on the VM and **see it pass**.

Group several behaviours per VM round-trip so you don't waste runs. Keep a log of the red and green runs: the test
names and the failing assertion. It goes in your reply.

Finish with **clippy `-D warnings` and the crate tests both green on the VM**, and every file you touched rustfmt-ed.

## Code rules (non-negotiable)
- **Never crash.** No `unwrap`, `expect`, `panic!`, `unreachable!`, `todo!`, `unimplemented!`, `unsafe` or slice
  indexing `[i]` outside `#[cfg(test)]` code. Instead:
  - return `Result` (use `filmcraft_media::MediaError` / `filmcraft_media::Result`);
  - use `get()`, checked or saturating math and `i128` for intermediate products;
  - avoid any division by a value that could be zero;
  - cap every allocation sized from input.

  Treat every number from the scene file, the filesystem, the renderer's output or the environment as hostile.
  Tests may unwrap.
- Exact time: media time is `filmcraft_time::Tick`. Never use f64 seconds for frame math.
- Wasm: the crate must compile for `wasm32-unknown-unknown`. Process and filesystem code goes behind
  `#[cfg(not(target_arch = "wasm32"))]`, with a wasm fallback that returns `MediaError::Unsupported`.
- No new dependencies. The manifest has time, bitstream (`filmcraft_bitstream::sha256`), frame, media,
  interchange, roxmltree, serde, serde_json, plus the dev-dependency `image` (png). You may add
  `proptest = { workspace = true }` as a dev-dependency. Anything else: reply BLOCKED and ask.
- Match the surrounding code: naming, low comment density (explain *why*, not *what*), no banner comments. Write doc
  comments on public items.

## Your reply
Your final message must be JSON matching the given schema:
- `kind`: `HANDOFF` when done, or `BLOCKED` with the reason.
- `summary`: one line.
- `body`: markdown with
  - the files changed;
  - the public API you added, with signatures;
  - design notes;
  - the TDD red→green log;
  - the final VM results (clippy plus test counts);
  - anything the integrator must know, such as edits needed outside your scope.

# Your task
**S8 + B7 (engine side): wire `Format::Scene` into the engine, commands, menus and file dialogs, and import scenes
natively or hybrid, with overlays rendered by scene-render.**

Already committed (read-only for you):
- `filmcraft_interchange::scene` (read, validate, resolve, stack, export, import). `scene::import(bytes, opts) ->
  Result<(Imported, Vec<Overlay>, Report)>`, with `Overlay { item, path, document }`. `Format::Scene` exists, and
  `import_with` refuses documents that need overlays unless `opts.lenient`.
- B0–B6: scene-render as an external renderer (`filmcraft-scenerender`, `crates/engine/src/scene_render.rs`,
  `media_pool.rs` `open_scene_bytes`). `file.import` of a `.scene` file imports it as **one rendered media item**.
  That behaviour must not change.

Scope (you may edit): `crates/engine/src/interchange.rs`, `crates/engine/src/commands.rs` (the interchange and
scene commands only), `crates/engine/src/scene_render.rs`, new engine test modules, `crates/ui-egui/src/lib.rs`
(only the import/export file-dialog filters and menu entries for scene), `apps/filmcraft-cli` if it lists formats,
and `docs/control-protocol.md`.

## S8 wiring
- `interchange::detect` allowlist: add `scene` (and keep `xml` sniffing working for a `<scene version=…>` root).
  `crates/engine/src/interchange.rs:13`.
- `ImportOptions.lenient` reaches the command params (`lenient: bool`, default false).
- `Error::Invalid(Vec<Diagnostic>)` is carried in the error JSON as a `diagnostics` array (severity, rule, path,
  line, message), not flattened into a string.
- Menus and dialog filters: the export menu lists "sr-core scene (.scene)" (`commands.rs:1475`), and the import
  filter includes `.scene` (`crates/ui-egui/src/lib.rs:1276`).
- Hostile-parameter tests for every new param (wrong types, huge strings, paths with `..`, NUL).
- `docs/control-protocol.md`: the new command and params.

## B7 (engine side): `file.importScene { path, mode: "rendered" | "native" | "hybrid", lenient? }`
- `rendered` is exactly what `file.import` does for a `.scene` today (one rendered media item).
- `native` runs `scene::import` and refuses (an error naming the count) when the document has overlays, unless
  `lenient`, which drops them and reports it.
- `hybrid` (the default) runs `scene::import`, writes each `Overlay.document` to `Overlay.path` the way AAF
  extracted media is written (create the directory, `s.services.write_file`, in-memory export honoured), merges the
  fragment, and links the overlay items through the normal Link Media step: `.scene` media open through scene-render
  (`open_scene_bytes`), so the overlay clips render from the renderer, or show the placeholder slate with the reason
  when it is missing.
- The result JSON adds `overlays: [{ item, path }]` and keeps `report`.
- One undo step for the import, as today.
- The command is registered with id, label, menu path (File ▸ Import ▸ Scene…), params and enabled(), so UI, CLI,
  control channel and MCP all reach it.

## Tests (TDD: write each test, see it fail on the VM, then implement)
- `file.import` of a `.scene` is still one rendered media item (regression).
- `file.importScene` native on a simple document gives the expected sequence (tracks, clip count); on a document
  with a `particles` subtree it refuses, and with `lenient` it imports and reports the dropped overlay.
- hybrid on the same document writes the overlay file (in-memory services), the overlay clip is on the expected
  track at the expected interval, its media item is a `.scene` file item, and with no renderer configured a frame
  at that time shows the placeholder slate (compare with `filmcraft_scenerender::placeholder::slate`).
- `file.export` with format scene writes a document that `scene::validate` accepts.
- invalid documents return `diagnostics` in the error JSON.
- hostile params (see above).

## Overrides for this task (they replace the crate-specific lines of the common section)
- You work in `crates/engine` (`filmcraft-engine`) and the scope above, not in `crates/scenerender`, which is read-only
  for you (report bugs there in your reply). `filmcraft-interchange` is read-only too.
- Errors here follow the engine's own error types and the existing command error JSON, not `MediaError`.
- No new dependencies at all (the engine already has interchange and scenerender). Anything else: reply BLOCKED.
- Wasm: the engine is above L4, so `cargo xtask wasm` does not check it, but keep file and process code behind the
  existing services traits.
- VM commands: iterate with a filter, e.g. `vm-agent.sh engine2 cargo test -p filmcraft-engine scene`. Before you
  finish, run on the VM `sh -c 'cargo clippy -p filmcraft-engine -p filmcraft-ui-egui -p filmcraft-cli --all-targets -- -D warnings && cargo test -p filmcraft-engine && cargo xtask layers'`
  (drop a `-p` that does not exist; check the package names in their `Cargo.toml`).

# FIX ROUND 1 (continuation of your task above)
You already did the work above; it is uncommitted in your worktree. Your previous reply (BLOCKED) was:
Changed files:

- `crates/engine/src/interchange.rs`
- `crates/engine/src/commands.rs`
- `crates/engine/src/scene_interchange_tests.rs` (new)
- `crates/ui-egui/src/lib.rs`
- `docs/control-protocol.md`

Public APIs added:

```rust
pub fn import_lenient(s: &mut Session, path: &str, bytes: &[u8], format: Format, lenient: bool) -> Result<Value>
pub fn import_scene(s: &mut Session, params: &Value) -> Result<Value>
```

Commands added: `file.importScene {path, mode?, lenient?}`, `file.export {format, path, sequence?}`, and `file.exportScene {path, sequence?}`.

Native/hybrid imports preserve reports and return merged overlay IDs. Hybrid writes through services, preserves renderer warnings, and groups scene import/linking into one undo step. Existing `.scene` media import remains unchanged. Differing existing overlay files are protected from overwrite. Other interchange formats retain their previous undo behavior.

TDD log—all names below have prefix `scene_interchange_`; all 11 now pass:

| Test | Observed red failure |
|---|---|
| `detection` | `None` instead of `Some(Scene)` |
| `native_simple_and_one_undo` | Stub lacked sequence result |
| `native_refuses_or_reports_dropped_overlay` | Expected refusal; received `{}` |
| `hybrid_overlay_links_and_renders_placeholder` | Stub lacked overlays |
| `rendered_preserves_file_import` | Stub lacked items; existing `file.import` assertions passed |
| `export_valid_scene_and_registry` | Unknown command `file.export` |
| `hostile_params` | Accepted null |
| `xml_lenient_reaches_import_options` | Lenient import still refused overlay |
| `write_failure_preserves_project` | Expected error; received `{}` |
| `export_hostile_params_do_not_write` | Accepted invalid export path |
| `hybrid_preserves_existing_overlay_file` | Overwrote existing content instead of refusing |

The full suite also exposed an FCP7 undo regression; restricting grouping to scenes fixed it.

Final VM results:

- Clippy `-D warnings`, engine/UI/CLI, all targets: **passed**.
- Engine tests: **660 passed, 1 failed, 3 ignored**. All 11 new tests passed.
- `cargo xtask layers`: **passed**.
- Changed Rust files formatted; `git diff --check` clean. No commits, pushes, dependencies, or git-config changes.

Integrator work required outside scope:

1. Preserve `Error::Invalid` through `EngineError` and control/MCP serializers, emitting `diagnostics` with severity, rule (`Diagnostic.code`), path, line, and message. Errors currently remain strings; the structured-diagnostics requirement is unfinished.
2. Add `file.importScene` to the missing-path dialog dispatch in `crates/ui-egui/src/menus.rs`. Its registry entry and dialog handler exist, but menu invocation does not yet open that dialog.
3. Update `crates/engine/src/tests.rs:751` to include `scene` in the expected format list. This is the sole remaining test failure.

Task closed as blocked; all changes remain uncommitted in the assigned worktree.

**Scope is widened** for this round. You may now also edit:
- the engine error type and its JSON (`crates/engine/src/lib.rs` or wherever `EngineError` lives), and the control
  channel / MCP error serializers (`crates/automation/**`, `apps/filmcraft-cli/**` only where errors are turned into
  JSON), so `Error::Invalid(Vec<Diagnostic>)` reaches the client as a `diagnostics` array (severity, rule = code,
  path, line, message) next to the existing message string. Do not change the shape of any other error.
- `crates/ui-egui/src/menus.rs`: route `file.importScene` with no path through the file dialog.
- `crates/engine/src/tests.rs`: the expected format list (add `scene`).

Integrator decisions:
- Remove the new generic `file.export` command (it duplicates `file.exportInterchange`). Keep `file.exportScene`
  (it matches `file.exportFcpxml`/`file.exportOtio`), and point your export test at `file.exportScene` and at
  `file.exportInterchange {format:"scene"}`.
- Review item 2: strip the extension exactly once; `shot.v1.scene` and `shot.v2.scene` get distinct overlay
  directories and sequence names. Regression test with both files.
- Review item 3: when the overlay files are not on a real filesystem (in-memory/virtual services), do not hand
  their paths to the external renderer: link them so they show the placeholder slate with a reason, and add a
  warning to the result. With real filesystem-backed services, behaviour is unchanged. Test both without depending
  on scene-render being installed.

Fix every item below and the three former blockers with strict TDD (regression test first, see it fail on the VM,
fix, see it pass; log red→green). Then rerun on the VM:
`sh -c 'cargo clippy -p filmcraft-engine -p filmcraft-ui-egui -p filmcraft-cli -p filmcraft-automation --all-targets -- -D warnings && cargo test -p filmcraft-engine -p filmcraft-automation && cargo xtask layers'`
(add `-p filmcraft-cli` tests if you touched it) and rustfmt every changed file. All engine tests must pass.
Reply with the same schema; `HANDOFF` when done.

## Review
Static review only; no files edited or cargo run. `file.export` is redundant with `file.exportInterchange`: both use the same handler and enablement check, so they currently agree.

Prioritized findings:

1. **[P2] Scene import menu never opens its dialog** — [lib.rs:1296](/home/admin/src/filmcraft-wt/engine2/crates/ui-egui/src/lib.rs:1296). The new picker branch is unreachable from the menu because the dialog-routing condition in [menus.rs:575](/home/admin/src/filmcraft-wt/engine2/crates/ui-egui/src/menus.rs:575) omits `file.importScene`. Clicking Scene… executes the engine command with `{}` and returns “need string `path`.” **Fix:** route `file.importScene` without a path through `app.file_dialog`.

2. **[P2] Double extension stripping makes distinct scene files collide** — [interchange.rs:209](/home/admin/src/filmcraft-wt/engine2/crates/engine/src/interchange.rs:209). `options()` supplies `file_stem()`, but the scene importer and overlay generator apply `common::file_stem` again. Consequently, `shot.v1.scene` and `shot.v2.scene` both generate `shot Scene Overlays/<position>-<id>.scene`. With corresponding overlay IDs and differing content, importing the second document fails the existing-file check. Sequence names also lose the version suffix. **Fix:** supply the full filename to the scene importer, stripping the extension exactly once; preserve other formats’ naming behavior.

3. **[P2] Virtual overlays are linked to a renderer that cannot read them** — [interchange.rs:118](/home/admin/src/filmcraft-wt/engine2/crates/engine/src/interchange.rs:118). On a native host using in-memory services, overlay writes exist only in the service. If `scene-render` is installed, `open_scene_bytes` nevertheless selects the external renderer, which later receives the virtual filesystem path as its input and working directory. Import reports successful linking without a warning, but rendering fails or shows unreadable media. The new tests hide this case by forcing renderer discovery to fail. **Fix:** stage the document and dependencies for external rendering, or explicitly use a placeholder with a warning when the host cannot provide filesystem-backed paths.

# FIX ROUND 2 (continuation of your task above)

Your round-1 reply (BLOCKED):
Implemented: scene menu routing, single extension stripping, removal of duplicate `file.export`, structured engine/MCP/CLI diagnostics, initial virtual-overlay placeholders, and the expected format-list update. All changes remain uncommitted.

Files changed:

- Engine: `crates/engine/src/{commands.rs,interchange.rs,lib.rs,scene_render.rs,scene_interchange_tests.rs,tests.rs}`.
- Automation: `crates/automation/src/{lib.rs,bridge.rs,server.rs}`.
- UI: `crates/ui-egui/src/{lib.rs,menus.rs}`.
- CLI: `apps/filmcraft-cli/src/main.rs`.
- Documentation: `docs/control-protocol.md`.

Public APIs added, including the previous round:

```rust
pub fn import_lenient(s: &mut Session, path: &str, bytes: &[u8], format: Format, lenient: bool) -> Result<Value>
pub fn import_scene(s: &mut Session, params: &Value) -> Result<Value>
impl EngineError { pub fn to_json(&self) -> Value }
impl AutomationError { pub fn to_json(&self) -> Value }
```

New error variants retain diagnostic payloads: `EngineError::InvalidScene { message, diagnostics }` and `AutomationError::AppDiagnostics { message, diagnostics }`. Other error shapes remain unchanged. `file.exportScene` and `file.exportInterchange {format:"scene"}` remain; `file.export` was removed.

Scene imports now preserve `shot.v1` and `shot.v2` sequence names and distinct sanitized overlay directories. Virtual overlays initially link to a slate with a filesystem warning, without probing the renderer. Filesystem overlays still render successfully using the test’s fake executable. Existing rendered-media import behavior and all 11 previous scene-interchange tests pass.

TDD log—new names below have prefix `scene_fix_`:

| Test | Observed red failure | Latest |
|---|---|---|
| `export_registry_has_no_duplicate` | Duplicate command present | Green |
| `versioned_names_do_not_collide` | `shot` instead of `shot.v1` | Green |
| `invalid_diagnostics_survive_engine` | Missing diagnostics array | Green |
| `virtual_overlays_never_spawn_renderer` | Empty warnings | Green |
| `import_menu_opens_dialog` | “need string path” | Green |
| `mcp_single_and_batch_diagnostics` | Error text was not diagnostic JSON | Green |
| `bridge_preserves_diagnostics` | Diagnostics array lost | Green |
| `cli_run_error_keeps_diagnostics` | Diagnostics array lost | Green |
| `control_keeps_diagnostics` | Diagnostics array lost | Red—scope pending |
| `virtual_overlays_stay_placeholder_after_cache_clear` | Renderer spawn attempted | Red—scope pending |
| `virtual_overlays_stay_placeholder_for_export` | Renderer spawn attempted | Red—scope pending |

The existing `export_interchange_refuses_an_unknown_format` test was also observed red and is now green. `filesystem_overlays_use_renderer` passed before and after the fixes.

Final VM results:

- Clippy, all four packages and all targets, `-D warnings`: **passed**.
- Engine: **666 passed, 2 failed, 3 ignored**; failures are the two reopening tests above.
- Automation: **22 passed**.
- CLI: **30 passed** across unit and integration suites.
- Targeted UI tests: **1 passed, 1 failed**; control diagnostics remain red.
- Layers: **passed**.
- All changed Rust files formatted; `git diff --check` clean. No dependencies, commits, pushes, or git-config changes. Coordination locks released.

The remaining changes are prepared in this unapplied [patch](/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/agents/engine2/pending-scope.patch). [Gate log](/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/agents/engine2/fix-round1-gates1.log).

Please approve the pending narrow edits to `crates/ui-egui/src/control.rs` and `crates/engine/src/media_pool.rs`. Your explicit file scope excludes those paths; they are needed to preserve desktop diagnostics and prevent virtual overlays from reaching the renderer after cache invalidation or during export. The drafted patch has not been applied or validated.

## Integrator decisions
1. **Scope approved:** you may edit `crates/ui-egui/src/control.rs` and `crates/engine/src/media_pool.rs`, but only for the two
   fixes in `agents/engine2/pending-scope.patch`: structured diagnostics on the control channel, and virtual overlays never
   reaching the renderer after a cache clear or during export. Nothing else in those files.
2. Apply the patch, then check it against the code as it stands. Two things in particular:
   - `services.export_in_memory()` must really mean "virtual (in-memory) overlay storage". If it means something else (for example
     an export to memory that still has real files on disk), add a precise predicate instead of reusing it. Don't break
     filesystem overlays during export: `filesystem_overlays_use_renderer` must stay green, and add an export-path variant of it.
   - On the control channel, a failed `engine.execute` must still carry `ok:false` plus a human-readable `error` string next to
     `diagnostics`, so existing clients that read only `error` keep working. Add that assertion to `control_keeps_diagnostics`.
3. TDD: the three red tests (`control_keeps_diagnostics`, `virtual_overlays_stay_placeholder_after_cache_clear`,
   `virtual_overlays_stay_placeholder_for_export`) must go green. Show them red, then green, in your log.
4. Gate on the VM (never on the workstation):
   `cargo clippy -p filmcraft-engine -p filmcraft-ui-egui -p filmcraft-cli -p filmcraft-automation --all-targets -- -D warnings && cargo test -p filmcraft-engine -p filmcraft-automation -p filmcraft-cli && cargo test -p filmcraft-ui-egui --lib control && cargo xtask layers`
   Don't run the full ui-egui test suite: VM disk is tight.
5. Don't commit. Reply HANDOFF with the gate result, or BLOCKED with the exact reason.
