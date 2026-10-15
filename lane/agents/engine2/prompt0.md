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
