**S7 (export half): `scene::export`, a FilmCraft sequence as an sr-core scene document at the lowest version that
holds it.**

Scope (you may edit):
- `crates/interchange/src/scene/export.rs`, or `scene/export/**`, and its test module.
- New fixtures under `crates/interchange/tests/fixtures/scene/export/`.
- A new integration test `crates/interchange/tests/scene_export.rs`.
- `scene/mod.rs`: your `pub mod` / `pub use` lines only.
- `crates/interchange/src/lib.rs`: **only** the minimum to reach your exporter:
  - a `Format::Scene` variant (name "sr-core scene", extension "scene");
  - its arm in `Format::ALL`/`name`/`extension`/`from_extension`;
  - sniffing in `detect` for a `<scene version="…">` root;
  - the `export()` dispatch arm.
  - Import dispatch for `Format::Scene` returns
    `Err(Error::Other("sr-core scene import is not implemented yet".into()))`. Another agent is writing the
    importer.
  - Check every `match` on `Format` in the workspace (`grep -rn "Format::" crates/`). Wherever a new variant
    breaks a match outside your scope (engine, ui-egui, cli), list the file and line in your reply instead of
    editing it. Do not add a wildcard arm to make it compile. Also tell me if the engine's export or import menus
    would now list `Scene` automatically.

**Another agent is writing `scene::resolve` (S4) in parallel.** Do not depend on it. The round-trip tests
(export → import) come later, after the importer exists. Write your exporter so it can be read back with the
semantics in the plan's "Semantic resolution" section.

## Entry point
Follow the shape of `fcpxml::export` / `otio::export`:
`pub(crate) fn export(p: &Project, seq_id: ItemId, opts: &ExportOptions, report: &mut Report) -> Result<String>`.
If the scene needs options (for example, a forced minimum version), add them to `ExportOptions` with a default that
changes nothing for other formats.

## Mapping
This is the inverse of the plan's import table. Read `filmcraft_project` (sequence, tracks, clip items, media,
keyframes, graphic, caption, markers, transitions, mixer) and the scene XSD to choose the exact elements and
attributes.
- **Sequence settings:** `project` width, height, fps (rational, e.g. `30000/1001`, where the rate is not an integer)
  and duration.
- **Media:** `assets` video, image and audio. The paths come from the media refs, relative when `opts` gives a base,
  the way the other exporters do it. Each asset is written once even when several clips use it.
- **Track order:** clip items become layers in paint order. A lower video track paints first; audio tracks become
  audio layers. Use explicit `start`/`end` in seconds.
  - Write seconds exactly: decimal text that `scene::values::seconds` parses back to the **same `Tick`**. Prove it
    with a property test over ticks on common frame grids (24, 25, 30000/1001, 48000 Hz). If plain decimal cannot
    round-trip, find the representation that does.
- **Trims and speed:** clipIn/clipOut from the source in-point, plus speed and reverse.
- **Motion:** position, scale, rotation and anchor become a transform. Do the anchor conversion explicitly and test
  it with a hand-worked fixture.
- **Basic effects:** opacity becomes opacity; volume and gain become volume (with mute when muted).
- **Keyframes:** they become `animate`/`key` with `timeBase` set explicitly, plus the interpolation mapping.
  Interpolation belongs to the outgoing segment, as the scene format defines it.
- **Graphics:** text graphic layers become text, and shape layers become shapes.
- **Transitions:** track transitions become `transition`, through a type table. Unknown types are reported and
  written as a cross dissolve, or skipped (your call, but say which).
- **Markers and captions:** sequence markers become `markers`; caption tracks become captions.
- **Nested sequences** become groups.
- **Everything else:** effects without an equivalent, masks, multicam, adjustment layers, and so on. Count them in
  the Report and drop them. Never fail the export for them.

## Version selection
The output declares the **lowest** `scene/@version` (1.0 to 1.7) that allows every element and attribute it writes.
- Derive the gates from the Schematron version rules (`V*` in `scene-render.sch`) and the XSD version notes. Build a
  table of gated constructs that your exporter can emit, with their minimum versions.
- **Version-gate tests:** for each gated construct you can emit, export a sequence that needs it, then check:
  - the declared version is exactly its minimum;
  - `scene::validate` passes;
  - the same document with the version decremented fails `validate` with that gate's rule.
- A sequence with only plain clips exports as `1.0`.

## Tests (required)
- **Every exporter test** reads the output back with `scene::read` and runs `scene::validate`: zero errors.
- **Expected-value fixtures:** small FilmCraft sequences built in the test, each with the exact expected XML (or the
  exact expected attribute values) worked out by hand. Cover:
  - one clip
  - a trimmed clip at speed 2
  - two tracks overlapping, for paint order
  - motion with an anchor
  - opacity keyframes with ease
  - a cross dissolve
  - markers
  - a caption track
  - text and a rect
  - a nested sequence
  - the 29.97 fps rate
- **Hostile:** an empty sequence, a missing sequence id gives `Error::NoSequence`, absurd clip values (huge ticks,
  zero-length clips, speed 0) never panic, and names and paths that need XML escaping (`&<>"'`, control
  characters) are escaped and survive `read`.
