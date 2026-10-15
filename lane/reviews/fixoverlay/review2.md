Found seven P1 defects. Review was read-only; no cargo commands were run.

1. **P1 — Pruning a sequence loses descendant placement.** [overlay.rs:495](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:495)  
   Consider a sequence containing a three-second layer, then `group g end="2"` whose two children have `end="2"` and reference each other through an expression. Promotion retains `g`, but prunes the preceding layer. Serialization replaces the sequence with a group and writes `g` as `[3,5)`, while its children remain `[0,2)` and disappear through clipping. Scene-render’s sequence placement shifts the entire timeline-sharing subtree. **Minimal fix:** promote through enclosing sequences and retain their placement context, or implement the complete recursive shift semantics.

2. **P1 — Promotion suppresses audio that the overlay cannot reproduce.** [import.rs:231](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import.rs:231)  
   Change `other` in the external-expression fixture to a video with `hasAudio="true"`. Composition promotion covers it, and this `continue` prevents its native audio clip from being created. The replacement is explicitly video-only, so the soundtrack disappears. **Minimal fix:** suppress covered video independently from audio; continue mapping representable audio with its original source mapping and container timing.

3. **P1 — Deferred programs lose resources referenced only by their output.** [overlay.rs:147](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:147)  
   A build program emits `<layer id="g1" asset="picture"/>`, and another node references `p.g1.opacity`. The closure retains the emitter and composition, but drops the `picture` asset when nothing in the original composition references it directly. `program_ids.rs` explicitly allows emitted references to resolve against document IDs; the resulting overlay fails after expansion. **Minimal fix:** when retaining opaque build programs, conservatively preserve their possible document dependencies—using the whole scene within the existing limits if necessary.

4. **P1 — Expression-valued attributes never enter the closure.** [overlay.rs:247](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:247)  
   An overlay object with `condition="param('show')"` loses the `show` parameter. Likewise, a parametric path’s `x="prop('source.x')"` loses its sibling dependency. These attributes have XSD type `expressionString`, not `IDREF`; only `<expression>` text is scanned. **Minimal fix:** inspect each attribute’s schema type and process values deriving from `expressionString`.

5. **P1 — `link/@source` dependencies are omitted.** [overlay.rs:247](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:247)  
   An overlay containing `<link property="x" source="other.x"/>` retains the link but omits sibling `other`. The XSD declares `source` as a string, so generic IDREF handling misses it. `param:`, `audio:`, and `marker:` sources have the same problem. Links on retained ancestors are also excluded by the child filter at line 213. **Minimal fix:** retain ancestor links and explicitly resolve the documented source forms.

6. **P1 — Valid expression syntax bypasses dependency detection.** [overlay.rs:158](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:158)  
   `<expression property="x">(prop)('other.x')</expression>` produces an overlay without `other`: the scanner sees `)` after `prop` and skips the call. Scene-render explicitly supports grouped callees in `program_ids.rs`. **Minimal fix:** use bounded tokenization that recognizes grouped calls and comments, and rejects unresolved dynamic targets rather than silently skipping them.

7. **P1 — Sequence serialization bypasses the work budget quadratically.** [overlay.rs:456](/home/admin/src/filmcraft-wt/fixoverlay/crates/interchange/src/scene/import/overlay.rs:456)  
   A fully retained sequence with 20,000 children calls `pruned_sequence(parent)` for every child. Each call scans all 20,000 selected siblings: approximately **400 million membership checks**, despite fitting the node, byte, and selection-work limits. Serialization charges none of these checks. **Minimal fix:** compute each sequence’s pruning status once and reuse it; charge serialization traversal against the work budget.

BLOCK