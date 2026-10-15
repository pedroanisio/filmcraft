# You are a member of the FilmCraft phase B team (agent `engine`)

FilmCraft is a pure-Rust video editor. Phase B makes it render sr-core scene documents (`.scene`) by spawning the
external `scene-render` program. Read these first:
- `AGENTS.md` and `CLAUDE.md` in your worktree. Their rules are binding.
- `docs/architecture.md`.
- The phase B plan: `/home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/PLAN-B.md`.

The crate you work in is `crates/scenerender` (`filmcraft-scenerender`, layer L3). Its skeleton is committed:
`lib.rs` declares the modules and `Quality`, and each module file holds only its doc comment.

## Your environment
- Your worktree is `/home/admin/src/filmcraft-wt/engine`, on git branch `b/engine`. Other agents work in parallel on
  other modules of the same crate in their own worktrees.
- **Only edit the files in your scope** (listed below). If you need a change outside it, describe it in your reply
  instead of making it.
  - `lib.rs` is shared: add nothing to it except `pub use` lines or a `#[cfg(test)] mod …;` line for your own modules.
- **Do not commit**, do not change git config, do not push.
- **NEVER run `cargo`, `rustc` or any build or test on this workstation.** It is forbidden: the machine cannot
  afford it. The only local tool you may run on Rust code is
  `rustfmt --edition 2024 --config-path rustfmt.toml <file.rs>` on files you changed.
- **Every build and test runs on the lab build VM** through this wrapper. It ships your worktree, uncommitted and
  untracked files included, to `~/filmcraft-engine` on the VM and runs the command there:

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh engine cargo test -p filmcraft-scenerender
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh engine sh -c 'cargo clippy -p filmcraft-scenerender --all-targets -- -D warnings && cargo test -p filmcraft-scenerender'

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

## Overrides for agent `engine` (these replace the crate-specific parts above)
- Your work is in **`crates/engine`**, not `crates/scenerender`. `crates/scenerender` is committed and finished, and
  **read-only for you**. The "lib.rs is shared" and "skeleton" notes above do not apply.
- The "No new dependencies" rule above is about the scenerender manifest. For you: you may add
  `filmcraft-scenerender = { workspace = true }` to `crates/engine/Cargo.toml`, plus dev-dependencies that already
  exist in the workspace (for example `image`, `tempfile` if present). Anything new to the workspace: reply BLOCKED.
- Your VM commands (the engine crate is large, so filter tests while iterating):

      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh engine cargo test -p filmcraft-engine scene
      /home/admin/src/render-frontend-js/PALS-Notes/projects/ops/filmcraft-scene/vm-agent.sh engine sh -c 'cargo clippy -p filmcraft-engine --all-targets -- -D warnings && cargo test -p filmcraft-engine && cargo xtask layers'

  Finish with the second command green.
- Engine code follows the same never-crash rules. Engine tests live where the existing engine tests for the area
  live (check `crates/engine/src/**/tests*.rs` and `crates/engine/tests/`); match that.

**B4: wire `filmcraft-scenerender` into the engine (import, pool, prefs, commands, export).**

`crates/scenerender` is finished and committed; **it is read-only for you**. Read its `lib.rs` and `source.rs` first.
The API you call:
- `filmcraft_scenerender::{Env, Opened, open, probe, Quality}`
- `discover::{Config, Status}`
- `store::Store` (`Store::new(media_cache_dir)`, `clear() -> bytes`, `size`)

If you find a real bug there, don't fix it: describe it in your reply.

Scope (you may edit):
- `crates/engine/**`, including its `Cargo.toml`. Add `filmcraft-scenerender = { workspace = true }`; it is already a
  workspace dependency, and the engine is above L3, so layering allows it.
- `crates/media/src/lib.rs`, only to make `.scene` importable (see below).
- `docs/control-protocol.md`
- `Cargo.lock`: the one line that adds `"filmcraft-scenerender"` to filmcraft-engine's dependency list, if cargo
  writes it.

What the engine has today (read these):
- `crates/engine/src/media_pool.rs`:
  - `MediaPool::open_file` (routes by path through `services.reader` / `read_file`), `resolve` (the cache keyed by
    `media_key`), `source_for`, `full_res_source`, `provider`, `full_res_provider`
  - `PoolProvider` (`recording`, `offline_used`)
  - `SlateSource`, `OfflineReason`
- `crates/engine/src/commands.rs`:
  - `file.import` (about line 1331), `import_streamed`, `import_bytes`, `import_source`
  - Note: without a host reader, `file.import` reads the bytes and calls `import_bytes` → `open_bytes(name, bytes)`,
    which **has no path**. A scene needs its path (the asset base), so it must be routed before that.
- `crates/engine/src/settings.rs`:
  - `PluginPrefs` (line 785; only `developer_mode` today)
  - the `"plugins"` `Category` in `categories()` (line 1311), `Kind::Path`, `Kind::Choice`
  - `Session::media_cache_dir()`, `apply_media_cache`
- `crates/engine/src/lib.rs`: `start_autosave` and `set_prefs`, where prefs side effects are applied (hardware decoding,
  media cache).
- Export: `export_tools.rs` around line 482 builds the export provider with `full_res_provider(...)`. Find every
  export path, including the export queue and `file.exportMedia`, and check which one renders the final file.

## Behaviour
1. **Import.**
   - `.scene` becomes an importable media extension: add `"scene"` to the right extension list in
     `filmcraft_media` (or a new `SCENE_EXTENSIONS`), so `is_importable` and the file dialogs' filters accept it.
     Check every user of those lists so nothing else starts trying to decode a scene as video.
   - `file.import` of a `.scene` path creates one media item through `filmcraft_scenerender::open(path, bytes, &env)`.
     This applies with or without a host reader. The item is `MediaRef::File { path }` with the info from the
     header, so projects stay compatible.
   - When `Opened.warning` is `Some`, the result carries it, e.g. a `"warnings": [..]` array. Check what the result
     already uses and stay consistent.
   - An invalid scene is a per-path error, like any undecodable file.
2. **Pool.** `MediaPool::resolve` (that is, `source_for`, `full_res_source` and the providers) opens `.scene` paths
   through `filmcraft_scenerender::open` with the pool's current scene env:
   - the `Config` from the pref
   - `cache_dir` = `Session::media_cache_dir()`
   - quality = the preview-quality pref
   Store that env in the pool, e.g. `RwLock<SceneEnv>`, set from the session. Failures fall back to the slate,
   exactly as other files do.
3. **Prefs.**
   - `PluginPrefs.scene_render_path: String`, a `Kind::Path` row; empty means auto (env `FILMCRAFT_SCENE_RENDER`, then
     `PATH`).
   - `PluginPrefs.scene_render_preview_quality: String`, a `Kind::Choice` of `draft` | `preview`, defaulting to
     `preview`.
   - Both rows go in the `"plugins"` category. Serde defaults must keep old preference files loading.
   - Replace the "No plugins are installed" note with an accurate one: scene-render is a separate program that
     FilmCraft spawns to render `.scene` files and never ships.
   - Applied at `start_autosave` and in `set_prefs`. **When the scene env changes, drop the cached scene sources**
     (and their proxies, if any) and `bump_view`, so items reopen with the new renderer or quality. Nothing else is
     dropped.
4. **Export at `Final` quality.**
   - The file the export writes must get scene frames at `Quality::Final`, while playback and render previews keep
     the preview quality.
   - Design it minimally. For example, `full_res_provider(..).final_scenes()` (a builder like `.recording()`) could
     resolve scene items through a second, final-quality source cache in the pool.
   - Write an engine test that proves an export reaches the renderer with `--quality final`, using the fake renderer
     (see Tests).
   - If an export used a placeholder (renderer missing), the export result or report says so. Reuse the existing
     offline-media reporting if it fits.
5. **Commands** (engine `cmd!` registry). Each gets a label, a menu path if it fits the existing menus (Help or File;
   otherwise none) and a params schema:
   - `scene.renderer.status` `{}` → `{found, path, version, schema, reason}`. This is `discover::Status` serialised
     (camelCase), from `filmcraft_scenerender::probe(&config)` for the current pref.
   - `scene.renderer.clearCache` `{}` → `{bytes}`. It clears `<media cache>/Scene Renders` through `Store::clear`. With
     no media cache dir it returns an error, not a panic. It also drops the cached scene sources so their in-memory
     frames go too.
   - Hostile-parameter tests for both: extra or garbage params must not crash; check the pattern other commands'
     tests use.
   - Document both in `docs/control-protocol.md`, in the format of that file.

## Tests (TDD: red first, then green, as in the common rules)
1. Importing a `.scene` with no renderer gives one item with the header's size, fps and duration, a placeholder frame
   (not the offline slate) and a warning in the result.
   - Make "no renderer" deterministic: set `scene_render_path` to a path that doesn't exist. Check what `locate` does
     with an explicit path that is missing, and if it falls back to `PATH`, find another deterministic route.
   - Never depend on whether scene-render is installed on the VM.
2. Unix only: with a fake renderer, frames come back through `MediaPool::source_for` / a provider at the header's size.
   - The crate's `FakeRenderer` is `pub(crate)` and test-only, so it is not reachable from the engine. Write a tiny
     engine-side test helper instead: a sh script in a temp dir that answers `--version` (`scene-render 0.5.0`),
     `capabilities` (the JSON format in `crates/scenerender/src/discover.rs` tests), and `render` by copying a PNG
     you generate to each frame path.
   - Point the pref at it.
3. Export reaches the renderer with `--quality final`, while playback uses preview. The fake logs its argv to a file.
4. Changing the pref drops the cached scene source; a different `Arc` comes back.
5. `scene.renderer.status` and `scene.renderer.clearCache`, including hostile params and the no-cache-dir error.
6. Old preference JSON without the new fields loads with the defaults.

Run engine tests with a filter while iterating, e.g. `cargo test -p filmcraft-engine scene`. Before you finish, run the
full `cargo clippy -p filmcraft-engine --all-targets -- -D warnings && cargo test -p filmcraft-engine` on the VM, and
`cargo xtask layers`.
