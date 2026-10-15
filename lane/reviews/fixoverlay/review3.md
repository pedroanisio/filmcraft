Five remaining defects. Review was read-only; no cargo commands were run.

1. **P1 — Audio recovery ignores repeat expansion.** [import.rs:233](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import.rs:233)  
   With `<repeat count="0"><layer asset="v"/></repeat>` and `v` having audio, the resolver still visits the template layer, and this branch emits its soundtrack despite there being no rendered instance. With `count="2" timeStep="2"`, it emits only one soundtrack and loses the delayed copy. Scene-render mixes expanded layer occurrences. **Minimal fix:** map audio from actual occurrences and their clocks; do not treat raw descendants of expanding containers as single native occurrences.

2. **P1 — The audio fix still drops ordinary video audio when `threeD="true"`.** [import.rs:233](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import.rs:233)  
   `<layer asset="v" threeD="true"/>`, where `v` has `hasAudio="true"`, is classified as `Unmappable`, so it never enters audio recovery and reaches the covered-node suppression at line 289. Its overlay is video-only. **Minimal fix:** determine audio eligibility and source timing independently of visual `NodeClass`; retain the soundtrack of supported media assets even when their visual representation requires an overlay.

3. **P1 — Expression dependency detection still omits supported built-ins.** [overlay.rs:175](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:175)  
   An overlay expression `10 * beat()` with `<beatGrid bpm="120"/>` loses the grid: at one second, its value becomes `0` instead of `20`. Likewise, `audioAmplitude('music')` loses its track, and `geo(...)` / `geoVisible(...)` lose their map assets unless another reference retains them. All are documented in the XSD. **Minimal fix:** retain the grid for `beat()` and follow audio/map arguments transitively, handling dynamic targets conservatively.

4. **P1 — XML comments can hide expression dependencies from the scanner.** [overlay.rs:275](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:275)  
   `<expression property="x">0 + <!-- gap --> prop('other.x')</expression>` scans only the first text node, `0 + `. Serialization preserves both text portions, producing an expression referencing the omitted sibling `other`; evaluation then fails. **Minimal fix:** concatenate all text children, excluding comments, before scanning, and charge their combined length against the budget.

5. **P1 — Retained ancestors lose transform constraints and their dependencies.** [overlay.rs:251](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:251)  
   Put an overlay object inside a normal group with `<transformConstraint type="copy-position" target="other"/>`, where sibling `other` has `x="100"`. The group survives as an ancestor, but its constraint is discarded by this child whitelist. The overlay consequently loses the inherited constrained position. **Minimal fix:** retain ancestor `transformConstraint` children and follow their `target` references; preferably derive retained behavior children from the schema.

BLOCK