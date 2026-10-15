# FilmCraft ↔ sr-core scene interchange (option A)

Repo: `/home/admin/src/filmcraft`, branch `scene-interchange` (fork: `pedroanisio/filmcraft`, remote `fork`).
Target: sr-core schema **1.7.0** (scene documents 1.0–1.7) and vpkg 1.0/1.1 (SREP 1, SREP 6).
Pinned sources: sr-core `babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc`. The scene-render revision is recorded at S3.

Licence: sr-core is MIT and is vendored with attribution. The owner wrote scene-render and granted permission
(2026-10-13) to borrow its code. Borrowed code records its source in `ATTRIBUTION.md` and is rewritten to
FilmCraft's rules: no unwrap, panic or indexing on untrusted data, layering, and `Tick` time.

Rev 2 folds in the Codex plan review (`20261013T134759Z-filmcraft-4f-5clf`).

## Design
- **Format.** `filmcraft_interchange::Format::Scene` (name "sr-core scene", extension `scene`), sniffed by a
  `<scene version="…">` root. This touches `Format::ALL`, the engine `detect` extension allowlist (`scene`,
  plus `xml` sniffing), `ImportOptions.lenient`, command params, menus and file-dialog filters.
- **Module.** `crates/interchange/src/scene/` (L2, pure, no I/O):
  - `values`: lengthType, rational fps, paint and checked seconds → Tick. The FCPXML `parse_time` is not reused, because it has unchecked mul and casts.
  - `read`: the full document, bounded.
  - `validate`: XSD structure plus Schematron, run on the **full** document before any reduction.
  - `resolve`: the semantic pass.
  - `import`
  - `export`
- **Bounds** apply before and during DOM allocation: input bytes, roxmltree node limit, depth,
  attribute/text length, IDs, diagnostics, expanded loops and total clips. Reference cycles are detected
  (parent, matte, group). Every computation over speed, duration and normalisation is checked.
- **Diagnostics.** A new `Error::Invalid(Vec<Diagnostic>)`, where `Diagnostic` = {severity, rule, path/line, message}.
  - A strict import refuses with every finding listed.
  - `lenient: true` imports anyway and puts the findings in the Report.
  - `lenient` never relaxes malformed XML, resource limits or safe-conversion failures.
  - The engine carries diagnostics in the error JSON.
- **Semantic resolution** (before mapping). It covers:
  - IDs and defaults
  - startMarker/endMarker
  - Time: parent clocks (group timeOffset/timeScale), end exclusive, absent end inherits the parent end.
  - Layer timing:
    - effective source span (clipIn/clipOut)
    - speed/timeStretch, with negative speed and reverse cancelling, and speed=0 as a hold
    - loop=N meaning N *additional* plays
    - freezeAt in layer-local time
    - timeRemap superseding speed/reverse/loop, with no clipIn added
  - Layout units: % of the parent box, vw/vh against the output frame, and the fit box for fitted layers.
  - Sequence auto-placement: previous end + timeGap + start offset, startMarker children exempt, default transitions.
  - Transition windows: sibling from/to, one-sided in/out, cut point, center/start/end handles, default ease-in-out.
  - When the media duration is unknown in the pure importer, clipOut or the node interval sets the duration and the report says so.
- **Time.** Seconds become `Tick` once, after resolution.
  - Snapping to the sequence frame grid uses round-half-even on edges.
  - A clip that collapses to zero frames is dropped and reported.
  - Linked audio keeps the unsnapped sample-accurate edges.
  - The report counts the snaps and the maximum error.
  - Project fps may be an integer or rational (30000/1001).
  - Project duration keeps trailing blank time.
- **Stacking** must keep paint order. Node *n* goes on the lowest track whose index is greater than the track of
  **every** earlier-painted node that overlaps *n* (including transition handles) and that is free over *n*'s
  interval. Sibling `z` is folded into paint order first.

  Example: with A=[0,5), B=[0,10), C=[5,10), C goes on V3, above B.
- **Import mapping**:

  | Scene | FilmCraft |
  |---|---|
  | project w/h/fps/duration | sequence settings |
  | project background (solid paint only; the default is opaque black) | colour-matte bottom track; gradients are reported |
  | `assets/video\|image\|audio\|imageSequence` | media items (paths resolved against base_dir) |
  | `layer` | clip(s), per the resolved timing above; a video asset with audio also gets a linked audio clip |
  | transform | Motion via an explicit anchor conversion: `T(x,y)·R·Skew·S·T(−anchor)` → position = x,y; anchor in the source/fit-box frame; skew is reported; parent transforms are flattened where they are affine and static, otherwise reported |
  | opacity, including inherited group opacity | Opacity |
  | `animate`/`key` | keyframes. timeBase composition (the default; the container timeline, including retiming), local and normalized are all converted to clip time. Interpolation belongs to the outgoing segment, and defaultInterpolation applies. CSS bezier is kept apart from temporal/spatial handles. additive, extrapolate and roving are reported. Repeated clips (loop) carry their keys across the repeats. |
  | `volume` / `mute` | Volume and gain |
  | text asset layers and `shape` (rect, ellipse, polygon, star, path) | graphic clips with text or shape layers |
  | `group` | a nested sequence when it isolates or retimes, otherwise flattened into the parent (keeps non-isolated blending) |
  | `sequence` | flattened after auto-placement (resolution) |
  | `transition` | track transitions through a type table; unsupported curves, params and handles are reported |
  | `markers` | sequence markers |
  | `captions` | caption track |
  | `audioMix` | track volume and pan (basic) |

  Everything not mapped is counted in the Report and not silently dropped. That covers 3D, particles,
  physics, expressions, programs and effects without an equivalent.
- **Export.** Sequence → scene XML at the lowest version that holds what is written, with explicit version-gate tests.
- **Validator.** Selected components borrowed from sr-model: the `xsd/` structure checker, the table generator
  isolated in our build.rs (run on the vendored XSD), and `rules.rs`. sr-model itself is never a dependency, and
  nothing does file reads, asset verification or program expansion.
  - The 14 `xs:pattern` facets are hand-coded, with equivalence tests, and the build fails on any pattern it does not know.
  - Schematron parity is tested against the vendored `.sch` and the conformance expectations.
- **vpkg** is a new crate `filmcraft-vpkg` (L3) with no new external dependency. Raw inflate and deflate are
  extracted from media (private) and export (private) into a lower-layer public module (`filmcraft-bitstream::flate`,
  L0), which removes the L3→L3 edge.
  - **ZIP:** structural validation, CRC-32 (bitstream), duplicate/path checks, control-char rejection, and
    budgets on expanded size and aggregate memory that don't trust the advertised sizes. ZIP64 is rejected with a documented limit.
  - **SREP 1:** the mandatory metadata files and their order, reproducible timestamps and modes, one primary
    scene, listed sizes and hashes, rejection of stray entries, and reference closure. Fonts, includes,
    image sequences and rewritten external paths follow a documented policy.
  - **SREP 6:** README.md and vpkg.schema.json hashes in 1.1, a warning for 1.0, complete non-writing validation before any extraction, and containment checks that follow links.
  - All filesystem work goes through engine services, and the project is merged only after verification succeeds.
- **Oracles** are opt-in and never required in CI. Small semantic and hostile fixtures **are** required in CI.
  - `SR_CORE_DIR` runs the corpus test, `sr-scene check --schema` on exports and `scenerender-vpkg`.
  - `SR_RENDER_BIN` runs optional frame comparison.

## Tasks (one commit each; `cargo xtask ci` green)
- **S1** Vendor `schema/` 1.7.0 (XSD, SCH, SHA256SUMS, LICENSE) unchanged into
  `crates/interchange/schema/sr-core/`, with `.attribution` sidecars and `ATTRIBUTION.md`. Add a pure-Rust
  `filmcraft_bitstream::sha256` with NIST vectors, and a test that the vendored files match SHA256SUMS.
- **S2** `scene::values` and `scene::read`: bounded parsing (all the caps above) and the value parsers. Unit tests
  plus malformed and hostile regression tests.
- **S3** `scene::validate`: XSD structure and Schematron, `Error::Invalid(Vec<Diagnostic>)`, pattern
  equivalence tests, and conformance-case tests (valid, warning and invalid).
- **S4** `scene::resolve`: the semantic pass and stacking, with expected-value fixtures for timing, anchors, stacking and transitions.
- **S5** `Format::Scene` import of project, assets, layers, transforms, keyframes and audio (validate → resolve → map). Report tests.
- **S6** Import of text and shapes, groups and sequences, transitions, markers, captions and audioMix.
- **S7** Export at the lowest version, version-gate tests, expected-value fixtures, and round-trip tests.
- **S8** Engine wiring:
  - detect allowlist, `Format::ALL`, `lenient`, and diagnostics in the error JSON
  - menus and dialog filters
  - hostile-parameter tests
  - `docs/control-protocol.md`
- **S9** Extract raw inflate and deflate into `filmcraft-bitstream::flate`; media and export use it (no behaviour change).
- **S10** `filmcraft-vpkg`: ZIP, manifest 1.0/1.1, SREP 1/6 validation, `file.importVpkg` / `file.exportVpkg`.
- **S11** Corpus and oracle tests. The corpus count comes from the pinned revision, not an assumed 690:
  - valid cases import
  - invalid cases return diagnostics
  - no case panics
  - exports pass `sr-scene check --schema`
  - packages round-trip through `scenerender-vpkg`
- **S12** Docs: `docs/interchange*.md`, `docs/gaps.md`, `ROADMAP.md`, the feature matrix.

## Out of scope
- B: scene-render as an external renderer for FilmCraft.
- C: a native scene engine (3D, particles, physics, expressions evaluated).
- Unmapped features are reported, not emulated.
