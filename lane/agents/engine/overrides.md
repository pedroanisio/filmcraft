
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
