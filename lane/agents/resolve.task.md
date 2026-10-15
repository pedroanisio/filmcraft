**S4: `scene::resolve`, the semantic pass, and `scene::stack`, the paint-order track assignment.**

Scope (you may edit):
- `crates/interchange/src/scene/resolve.rs`, or `scene/resolve/**` if you split it.
- `crates/interchange/src/scene/stack.rs`
- Their test modules.
- New fixtures under `crates/interchange/tests/fixtures/scene/resolve/`.
- A new integration test `crates/interchange/tests/scene_resolve.rs` if you want one.
- `scene/mod.rs`: your `pub mod` / `pub use` lines only.

Both agents' code consumes your output:
- S5/S6, the importer that comes after you, turns it into FilmCraft clips.
- B7 (hybrid rendering) uses it to find the subtrees that cannot be mapped and their paint position.

Design the types so that an importer never has to re-derive timing, layout or paint order from the XML.

## What `resolve` does
`pub fn resolve(doc: &SceneDoc<'_>, limits: &ResolveLimits) -> Result<Resolved>` works on a document that has
**already passed `validate`**; it may still meet values validation does not check, so stay defensive. It covers
every bullet of the plan's "Semantic resolution" section.

**Nodes, paint order and IDs**
- Walk the composition tree (whatever sits under the scene's timeline/composition root; find it in the XSD).
- Give every node a stable index, its parent index and its depth.
- Fold sibling `z` into paint order, then list the nodes in final **paint order** (back to front).
- Apply the defaults: IDs and the attribute defaults the XSD declares.
- Resolve `startMarker`/`endMarker` references.

**The time model**
- Each node gets its **composition interval** `[start, end)` in composition seconds:
  - The end is exclusive.
  - An absent end inherits the parent's end.
  - Parent clocks are honoured: group `timeOffset`/`timeScale`.
  - A `sequence`'s children are auto-placed: previous end + `timeGap` + start offset. `startMarker` children are
    exempt, and default transitions apply.
- A media layer also gets a **source map**: an ordered list of segments
  `{ comp_start, comp_end, src_start, rate }`, where `rate` is source seconds per composition second. It covers:
  - the effective source span (`clipIn`/`clipOut`);
  - speed and `timeStretch`, with negative speed and `reverse` cancelling;
  - `speed=0` as a hold (rate 0);
  - `loop=N` as N **additional** plays, each a segment, capped by `ResolveLimits`;
  - `freezeAt` in layer-local time.
- `timeRemap` supersedes speed, reverse and loop, with no `clipIn` added. Represent it separately, for example
  `SourceMap::Remap { keys }` carrying the composition-time keys. The importer maps it to time-remap keyframes or
  reports it.
- When the media duration is unknown (resolve never reads media), `clipOut` or the node interval sets the duration.
  Record a note saying so.
- Give every node a way to convert local time to composition time, for the importer's keyframes. A keyframe's
  `timeBase` can be composition (the default: the container timeline, including retiming), local or normalized.
  Provide `fn key_time(&self, node, time_base, t) -> Option<f64>`, or precomputed affine pieces, so the importer
  does not re-derive the clock chain.

**Layout**
- Every visual node gets its box in output pixels:
  - `%` is of the parent box;
  - `vw`/`vh` are of the output frame;
  - fitted layers use their fit box.
- It also gets its static transform parameters in pixels: position, anchor (in the source/fit-box frame), scale,
  rotation in degrees, skew.
- Each animated property gets a flag (its animate/key elements are listed for the importer).
- Inherited group opacity is multiplied down.
- Parent transforms are flattened when they are affine and static (compose the matrices,
  `T(x,y)·R·Skew·S·T(−anchor)`). Otherwise, flag them for the report.

**Transitions**
- Resolve each transition's window:
  - sibling `from`/`to`
  - one-sided in/out
  - the cut point
  - center/start/end handles
  - the default ease-in-out
- The output is the two node indices (or one for one-sided), the window `[start, end)` in composition seconds, the
  type name and the curve.

**Mappability classes**
Classify each node: media layer (video, image, audio, imageSequence), text, shape (rect, ellipse, polygon, star,
path), group (isolating/retiming, or flattenable), sequence, or **unmappable**. Unmappable means 3D, particles,
physics, simulation, expressions, programs, effects without an equivalent, and anything else that is not natively
mappable. List the XSD element names you put in each class in a doc comment table.
- For an unmappable node, report the subtree root once, with its interval and paint position. B7 renders such
  subtrees externally.
- Anything inside a mappable node that is itself unmappable, such as an `expression` on a layer's property, sets a
  per-node flag with the element names.

**Notes and failures**
- Notes are structured: `{ node: Option<usize>, code: &'static str, message: String }`. The importer turns them into
  Report entries.
- Every failed safe conversion is an `Err` (`Error::Parse` via `scene::err`): non-finite values, overflow, an
  expansion over the cap, a reference cycle (parent, matte, group), or depth over the cap. Lenient import never
  relaxes these.

The project's width, height, fps (integer or rational, e.g. 30000/1001), duration (trailing blank time kept) and
background paint (solid; gradients flagged) go in `Resolved.project`.

## What `stack` does
`pub fn assign(items: &[Span]) -> Vec<u32>`, where `Span { start: Tick, end: Tick }`.
- The input is in **paint order** (back to front), with transition handles already included in the spans.
- It returns a track index (0 = V1) per item.
- **The rule:** item *n* goes on the lowest track that is greater than the track of **every** earlier item that
  overlaps *n*, and that is free over *n*'s interval.
- Fixture from the plan: A=[0,5), B=[0,10), C=[5,10) gives A→0, B→1, C→2 (C is above B, not beside A).
- Add property tests (proptest):
  - for any overlapping pair i<j, track(j) > track(i);
  - no two items on one track overlap;
  - the track count is ≤ the item count.
- Spans with `end <= start` are invalid input. Decide and document how you handle them (for example, they take
  track 0 and occupy nothing). Never panic.
- It must be O(n·tracks) or better, with no quadratic blow-up on 100k items. Add a bounded-time test with 100k
  items.

## Fixtures (required, each with hand-worked expected values)
**Timing**
- clipIn/clipOut
- speed 2 and 0.5
- negative speed plus reverse cancelling
- speed=0 hold
- loop=2 (three plays)
- freezeAt
- timeRemap
- a group with timeOffset and timeScale around a layer
- absent end inheriting the parent end
- a sequence with timeGap and a startMarker child

**Anchors and layout**
- % against the parent box
- vw/vh
- a fitted layer's anchor
- rotation plus scale composed through a static parent group

**Stacking**
- the A/B/C example
- a transition whose handles push the overlap

**Transitions**
- from/to siblings
- one-sided in
- the center handle and the default curve

**Hostile**
- a loop count that would expand past the cap gives Err
- a parent cycle gives Err
- deep nesting gives Err
- NaN/inf never reach a Tick

Fixtures must pass `scene::validate` with no errors. Assert that in each test before resolving.
