# filmcraft-vpkg

Pure, in-memory scene video packages, layer L3. Reads formats 1.0 and 1.1 and later
1.x with warnings; writes 1.1. No filesystem, environment, clock, downloads,
processes or new external dependencies in the library. Engine integration is a
later task.

```rust
use filmcraft_vpkg::{FileInput, Limits, PackInput, pack, read};
use filmcraft_vpkg::manifest::Metadata;

let input = PackInput {
    primary: FileInput {
        path: "main.scene".into(),
        bytes: br#"<scene version="1.7"><project width="320" height="180"
            fps="24" duration="1"/><composition/></scene>"#.to_vec(),
        role: "scene".into(),
    },
    files: vec![],
    metadata: Metadata {
        id: "example".into(), version: "1.0.0".into(), title: "Example".into(),
        created: Some("2026-10-14".into()),
    },
    limits: Limits::default(),
    source_date_epoch: None,
};
let archive = pack(&input)?;
let package = read(&archive, &input.limits)?;
assert_eq!(package.primary_scene, "main.scene");
# Ok::<(), filmcraft_vpkg::Error>(())
```

## API

- `read(bytes: &[u8], limits: &Limits) -> Result<Package, Error>`
- `pack(input: &PackInput) -> Result<Vec<u8>, Error>`
- `zip::read(bytes: &[u8], limits: &Limits) -> Result<Vec<zip::Entry>, Error>`
- `zip::write(entries: &[zip::Input<'_>], limits: &Limits, epoch: Option<u64>) -> Result<Vec<u8>, Error>`
- `zip::safe_path(path: &str) -> bool`
- `manifest::read(bytes: &[u8], limits: &Limits) -> Result<manifest::Manifest, Error>`

`Package.entries` owns the verified bytes once, including the three metadata
entries. Paths are archive-relative (`project/main.scene`); `primary_scene` and
manifest paths are project-relative (`main.scene`). The ZIP input buffer is never
retained. `Manifest.raw` preserves the full JSON; `Manifest.data` contains the
typed fields FilmCraft consumes. Serialize `raw` to preserve extension keys;
serializing `data` alone is deliberately a partial view.

Errors contain bounded `Diagnostic { severity, code, path, message }` values.
`Severity` is shared with scene diagnostics. `Package.warnings` includes version,
unhashed 1.0 metadata, remote input and undeclared font warnings. No partially
verified `Package` is returned. Hash checks continue across listed files after
an entry mismatch; structurally unreadable ZIP or manifest input fails early.

## Validation and limits

The unchanged bundled `schema/vpkg-1.schema.json` comes from sr-core revision
`babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc` (format 1.1). The schema interpreter
supports every keyword that schema uses: `$ref`, `type`, `const`, `enum`,
`required`, `properties`, `additionalProperties`, `items`, `minItems`, `minLength`,
`minimum` and its eight patterns, hand-coded without regex. Unknown patterns fail
closed. Unknown properties and enumeration values warn only in later 1.x.
Exactly one primary, unique scene/file/fetch paths and step IDs, known needs,
render scene membership, command argv presence, and acyclic pipelines are checked.
Pipeline traversal is iterative. A packed manifest must contain `files`.

This is not a general JSON Schema engine: other keywords and external schema
references are not implemented. The package's embedded schema is hash-checked,
never loaded as executable validation policy or fetched. `created`, homepage,
license text and optional metadata are checked only as specified by the bundled
schema; e.g. `created` is a string, not a calendar-date constraint. The package
version pattern is the schema's simplified semver pattern. Non-integral JSON
numbers and out-of-range integers cannot supply typed file sizes. Summary counts
in `packed` are informational; actual entry lengths and hashes are authoritative.

Default budgets: 10,000 entries, 256 MiB total expanded bytes, 64 MiB per entry,
1,024 bytes per path, 512 MiB conservative working memory, 2 MiB per manifest/XML
or nested dependency document, and 4 million validation/matching work units.
The memory budget includes borrowed input, retained byte capacities and
conservative allowances for JSON, XML, names, ZIP tables and compression scratch.
Limits are ceilings, not promises to admit every input below an individual cap.
Inflation is bounded by actual output and aggregate remaining memory, then its
length, consumed compressed bytes and CRC are checked. ZIP output and stored
payload allocations are fallible. Diagnostics are capped at 128 per phase and
512 characters per field. XML depth is at most 64; serde JSON's default depth
limit remains enabled. Work queues use seen sets, including cyclic includes and
asset graphs; matching has a shared work budget.

ZIP64 is unsupported (including extra fields, count/size sentinels and ZIP64 end
records). ZIP methods 0 and 8, optional signed/unsigned data descriptors, UTF-8
names, and bounded EOCD comments are supported. Non-ASCII names require bit 11
in both headers; unflagged names must be ASCII. The writer sets bit 11. Encrypted/multi-disk archives,
local/central mismatches, bad CRCs, extra compressed bytes, overlapping entries,
symlinks, duplicates and case collisions are rejected. ZIPs must be contiguous,
without self-extracting prefixes, unclaimed data or directory records, and their
central-directory order must match physical order. Metadata order is mandatory:
`vpkg.json`, `vpkg.schema.json`, `README.md`. The writer sorts project files.

Names reject absolute paths, parent/dot/empty segments, backslashes, colons,
control characters, trailing dots/spaces, and file/directory prefix collisions.
Case collision and parent-prefix checking uses Unicode lowercase then uppercase
(including ß/ẞ and expanding pairs),
not filesystem-specific Unicode normalization. A later extraction service must
still enforce containment against existing filesystem links and platform aliases;
this library never extracts anything.

## Reference and writer policy

Attribute classification is adapted exactly from `sr_core/vpkg/refs.py`'s input
and cache tables; output/poster/thumbnail/still paths and destination URIs are
not required inputs. Inputs resolve relative to the document holding them. Local
URI escapes decode before containment checks, except image-sequence patterns.
`data:` URIs are embedded bytes, not files. References may climb from a subfolder
within the project, but must not escape its root. Absolute/file URIs are refused;
callers must supply local portable references rather than asking this crate to
copy and rewrite external paths.

Primary, variant and packaged archive scenes are parsed through the bounded scene
reader. Drafts and unknown future roles are exempt. Includes can be XML fragments;
they are wrapped for the same bounded parser. Every reachable include is scanned,
with cycles deduplicated. glTF/GLB buffers/images, OBJ libraries, MTL maps and
MaterialX filename inputs are followed. Unlike Python's permissive dependency
scanner, malformed dependency documents fail instead of hiding missing inputs.
Image-sequence `%Nd` patterns require at least N digits; `#` runs require exactly
that many. Patterns apply to the filename. Known divergence from `refs.py`:
`%0Nd`/`%Nd` and `#` matching accepts ASCII digits only, including pattern widths.
Python’s `\d` also matches Unicode decimal digits, so FilmCraft is stricter.

Missing inputs can be covered by a pinned fetch path or a pipeline output glob,
including render `to`, `{time}`/`{frame}`, `*`, `?`, `**`, and `**/`. Globs use
`pack.py`'s gitignore-like matching, not shell expansion. Unpackaged primary or
variant scenes need an explicit fetch/producer. Optional cache files need not
exist. Read warns on remote inputs like `archive.py`; pack rejects them. No
remote content is accessed. This follows the Python reference where the broad
"reference closure" wording does not spell out remote input behavior.

`pack` accepts one primary plus explicit files and roles. It requires positive
integer dimensions, valid rational fps, and a finite positive duration; duration
is metadata only, with no edit-time arithmetic. Audio includes audio assets in
included documents; captions and outputs come from the primary document. The
actual scene version is recorded (the reference packer's hard-coded `1.1` is not
copied). README generation is a minimal deterministic adaptation of `readme.py`.
The README names the bundled sr-core schema and includes the complete upstream
MIT copyright, permission and warranty notice from the vendored license. The
final metadata hashes cover exactly the shipped README (including that notice)
and schema bytes.

Compression extensions match `pack.py` (e.g. PNG/MP4 stored, WAV/XML deflated).
The fixed MIME table adds `.scene` as XML. Modes are 0644, or 0755 for files with
role `script`; there is no host executable-bit lookup. Times default to
1980-01-01; explicit `source_date_epoch` is UTC, clamped below 1980 and rejected
beyond 2107, rounded down to ZIP's two-second precision. No current time is used.

Font discovery/resolution, script portability lint, downloading fetches, building
or running pipelines, scanning arbitrary script dependencies, directory walking,
and path rewriting are out of scope: these need host state, executable behavior,
or author policy. Undeclared font families warn during read. Supplied font files
and declared font references still receive the same integrity/closure checks.
This crate does not perform XSD/Schematron scene validation or render the scenes;
it uses the bounded scene parser for package reference verification.

## Tests and provenance

All Rust commands run through `vm-agent.sh vpkg` on the build VM. `TDD.md` records
red/green runs. Foundation tests hand-write ZIP records independently of the ZIP
writer. Package tests include generated-byte round trips and mutation checks
under `catch_unwind`. Three tiny oracle fixtures (minimal, includes, fetch) were
created with `python3 -m sr_core.vpkg pack`; their sidecars describe licensing.
`tests/generate_fixtures.py` reproduces them from synthetic documents.

The opt-in `python_verifies_written_package` test runs Python when explicitly
selected with `--ignored` and `SR_CORE_DIR` names an sr-core checkout on the VM.
Both oracle utilities cover ASCII paths and a non-ASCII scene/asset pair.
The ignored `emit_oracle_package` test prints `VPKG_HEX=...` and
`VPKG_UTF8_HEX=...`, allowing verification on a separate machine without shared
filesystems. Python's `zipfile.testzip()`
and `python3 -m sr_core.vpkg verify` can check the decoded file.

Adapted MIT sources at the pinned revision: `sr_core/vpkg/archive.py`,
`manifest.py`, `pack.py`, `readme.py`, and `refs.py`. ZIP binary parsing/writing is
original; compression policy and timestamp conventions come from `pack.py`.
Copyright and MIT permission notices are retained in the adapted source headers;
the complete upstream license also ships at
`../interchange/schema/sr-core/LICENSE`. See the root `ATTRIBUTION.md`.
