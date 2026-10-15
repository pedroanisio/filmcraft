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
