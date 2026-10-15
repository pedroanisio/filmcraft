---
name: sr-core-scene-render-grant
description: User authors sr-core (MIT) and scene-render (PolyForm NC) and granted permission to borrow their code into FilmCraft
metadata:
  node_type: memory
  type: project
  originSessionId: 6f1fd2c8-7212-482c-b7e7-abd3e364c42a
  modified: 2026-10-13T13:39:08.179Z
---

The user (Pedro Anisio Silva) is the author of both `/home/admin/src/sr-core` (MIT; scene XSD, conformance kit, SREPs, Python SDK `sr_core`) and `/home/admin/src/scene-render` (Rust engine, licensed PolyForm-Noncommercial). On 2026-10-13 they granted permission to borrow any code from either into FilmCraft (MIT OR Apache-2.0).

**Why:** scene-render's PolyForm NC licence would otherwise forbid copying it into FilmCraft; the grant comes from the copyright holder.

**How to apply:** borrowed scene-render code is allowed, but record provenance (source path + "relicensed by the author for FilmCraft, 2026-10-13") in `ATTRIBUTION.md`, and adapt it to FilmCraft's rules (never-crash, layering, Tick time, no new deps without asking). The sr-core Python SDK (`sr-scene check --schema`, `scenerender-vpkg`) works as an external test oracle (`PYTHONPATH=/home/admin/src/sr-core`). Related: [[filmcraft-scene-interchange-lane]]
