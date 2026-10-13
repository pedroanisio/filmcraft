//! Track assignment preserving back-to-front paint order.

#![cfg_attr(not(test), deny(clippy::indexing_slicing))]

use filmcraft_time::Tick;

use std::collections::BTreeMap;

/// Half-open occupied interval, including any transition handles.
#[derive(Clone, Copy, Debug)]
pub struct Span {
    pub start: Tick,
    pub end: Tick,
}

/// Lowest legal track for every item in paint order. Empty/reversed spans occupy nothing on track 0.
/// Uses O(n log n) time and O(n) space, including arbitrary time ordering and fully overlapping input.
/// Track indices saturate at u32::MAX if more than 2^32 mutually overlapping items are supplied.
pub fn assign(items: &[Span]) -> Vec<u32> {
    // Values are the number of occupied levels at each breakpoint. Replacing an interval
    // erases every interior breakpoint; each inserted breakpoint is erased at most once.
    // Thus all range scans together cost O(n), with O(n log n) tree work and O(n) storage.
    let mut levels: BTreeMap<Tick, u32> = BTreeMap::new();
    let mut out = Vec::with_capacity(items.len());
    for item in items {
        if item.end <= item.start {
            out.push(0);
            continue;
        }
        let before = levels.range(..=item.start).next_back().map_or(0, |(_, v)| *v);
        let after = levels.range(..=item.end).next_back().map_or(0, |(_, v)| *v);
        let mut track = before;
        let mut remove = Vec::new();
        for (&time, &level) in levels.range(item.start..item.end) {
            track = track.max(level);
            remove.push(time);
        }
        for time in remove {
            levels.remove(&time);
        }
        levels.insert(item.start, track.saturating_add(1));
        levels.entry(item.end).or_insert(after);
        out.push(track);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn spans(pairs: &[(i64, i64)]) -> Vec<Span> {
        pairs.iter().map(|&(a, b)| Span { start: Tick(a), end: Tick(b) }).collect()
    }

    #[test]
    fn scene_stack_paint_order_and_handles() {
        // C touches A but overlaps B, so it must be above B.
        assert_eq!(assign(&spans(&[(0, 5), (0, 10), (5, 10)])), [0, 1, 2]);
        // A centered two-second transition extends each side one second.
        assert_eq!(assign(&spans(&[(0, 6), (4, 10)])), [0, 1]);
        assert_eq!(assign(&spans(&[(5, 5), (9, 2), (0, 5), (5, 10)])), [0, 0, 0, 0]);
        // Input need not be ordered in time.
        assert_eq!(assign(&spans(&[(10, 20), (0, 15), (0, 5)])), [0, 1, 2]);
    }

    proptest! {
        #[test]
        fn scene_stack_properties(pairs in prop::collection::vec((-100i64..100, -100i64..100), 0..200)) {
            let items = spans(&pairs);
            let tracks = assign(&items);
            for (i, a) in items.iter().enumerate() {
                prop_assert!((tracks[i] as usize) < items.len());
                for (j, b) in items.iter().enumerate().skip(i + 1) {
                    if a.start < a.end && b.start < b.end && a.start < b.end && b.start < a.end {
                        prop_assert!(tracks[j] > tracks[i]);
                        prop_assert_ne!(tracks[j], tracks[i]);
                    }
                }
            }
        }
    }

    #[test]
    fn scene_stack_100k_bounded() {
        let start = std::time::Instant::now();
        // All-overlapping input is the worst case for a scan through tracks.
        let items = vec![Span { start: Tick(0), end: Tick(10) }; 100_000];
        let tracks = assign(&items);
        assert_eq!(tracks.last(), Some(&99_999));
        let disjoint: Vec<_> = (0..100_000).map(|i| Span { start: Tick(i * 2), end: Tick(i * 2 + 1) }).collect();
        assert!(assign(&disjoint).iter().all(|&t| t == 0));
        assert!(start.elapsed().as_secs() < 30);
    }
}
