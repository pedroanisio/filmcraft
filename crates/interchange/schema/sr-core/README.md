# sr-core scene schema (vendored)

The sr-core scene document schema, 1.7.0, copied unchanged from sr-core `schema/` at commit
`babeccf0ca0cb2603b8f6a1d3cb07388b7d7a2cc` (2026-10-12). MIT, © Pedro Anisio Silva; see `LICENSE`.

| File | What |
|---|---|
| `scene-render.xsd` | W3C XML Schema: the structure of a scene document |
| `scene-render.sch` | ISO Schematron: the rules the XSD cannot express |
| `SHA256SUMS` | upstream checksums; `cargo test -p filmcraft-interchange vendored_schema` checks the files against them |

Never edit these files. To update, copy all three from a newer sr-core `schema/`, bump the commit here, in
the `.attribution` sidecars and in `ATTRIBUTION.md`, and fix whatever the tests then report.
