//! Exact frame and chunk arithmetic for scene-render.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

use filmcraft_time::{FrameRate, TICKS_PER_SECOND, Tick};
use std::ops::Range;

fn ratio(fps: FrameRate) -> Option<(i128, i128)> {
    if fps.num <= 0 || fps.den <= 0 {
        return None;
    }
    Some((i128::from(fps.num), i128::from(fps.den).checked_mul(i128::from(TICKS_PER_SECOND))?))
}

/// Number of frames covering a duration, rounding to the nearest frame within one nanosecond.
/// Invalid rates and nonpositive durations give zero; counts beyond u64 saturate.
pub fn frame_count(duration: Tick, fps: FrameRate) -> u64 {
    let count = || -> Option<i128> {
        let (num, denominator) = ratio(fps)?;
        let product = i128::from(duration.0.max(0)).checked_mul(num)?;
        let whole = product.checked_div(denominator)?;
        let remainder = product.checked_rem(denominator)?;
        let distance_up = denominator.checked_sub(remainder)?;
        let nearest_up = remainder >= distance_up;
        let distance = remainder.min(distance_up);
        // Compare in numerator units, avoiding an overflowing distance * 1e9.
        let tolerance = num.checked_mul(i128::from(TICKS_PER_SECOND))?.checked_div(1_000_000_000)?;
        let increment = if distance <= tolerance { nearest_up } else { remainder != 0 };
        whole.checked_add(i128::from(increment))
    };
    count().map_or(0, |n| u64::try_from(n).unwrap_or(u64::MAX))
}

/// Frame at a time, floored and clipped to `count - 1`; negative time, invalid rates or no frames give zero.
pub fn frame_at(t: Tick, fps: FrameRate, count: u64) -> u64 {
    let index = || -> Option<i128> {
        let (num, denominator) = ratio(fps)?;
        i128::from(t.0.max(0)).checked_mul(num)?.checked_div(denominator)
    };
    index().map_or(0, |n| u64::try_from(n).unwrap_or(u64::MAX)).min(count.saturating_sub(1))
}

/// Start of frame `k`, rounded up only when its rational time falls between ticks.
/// Returns None for invalid rates, overflow, or a sub-tick frame containing no representable tick.
pub fn frame_time(k: u64, fps: FrameRate) -> Option<Tick> {
    ratio(fps)?;
    let num = i128::from(fps.num);
    let product = i128::from(k).checked_mul(i128::from(fps.den))?;
    // Divide before scaling so large cancelling rate terms do not overflow.
    let seconds = product.checked_div(num)?.checked_mul(i128::from(TICKS_PER_SECOND))?;
    let fraction = product.checked_rem(num)?.checked_mul(i128::from(TICKS_PER_SECOND))?;
    let ticks = seconds.checked_add(fraction.checked_div(num)?)?.checked_add(i128::from(fraction.checked_rem(num)? != 0))?;
    let tick = Tick(i64::try_from(ticks).ok()?);
    let (rate_num, denominator) = ratio(fps)?;
    let actual = ticks.checked_mul(rate_num)?.checked_div(denominator)?;
    (actual == i128::from(k)).then_some(tick)
}

/// Aligned chunk containing the clipped frame index; zero length means one frame.
/// The range is empty only when `count` is zero.
pub fn chunk(k: u64, len: u64, count: u64) -> Range<u64> {
    if count == 0 {
        return 0..0;
    }
    let len = i128::from(len.max(1));
    let index = i128::from(k.min(count.saturating_sub(1)));
    let start = index.checked_div(len).and_then(|n| n.checked_mul(len)).unwrap_or(0);
    let end = start.checked_add(len).unwrap_or(i128::from(count)).min(i128::from(count));
    u64::try_from(start).unwrap_or(0)..u64::try_from(end).unwrap_or(count)
}

#[cfg(test)]
mod tests {
    use super::*;
    use filmcraft_time::TICKS_PER_SECOND;
    use proptest::prelude::*;

    #[test]
    fn counts_whole_and_partial_frames() {
        for fps in [FrameRate::FPS_24, FrameRate::FPS_25, FrameRate::FPS_29_97, FrameRate::FPS_60] {
            let boundary = fps.tick_of(37);
            assert_eq!(frame_count(boundary, fps), 37);
            assert_eq!(frame_count(boundary + Tick(1000), fps), 38);
            assert_eq!(frame_count(boundary - Tick(1000), fps), 37);
            assert_eq!(frame_count(Tick(-1), fps), 0);
            assert_eq!(frame_count(Tick::ZERO, fps), 0);
        }
    }

    #[test]
    fn count_tolerance_is_one_nanosecond_in_time() {
        for fps in [FrameRate::FPS_24, FrameRate::FPS_25, FrameRate::FPS_29_97, FrameRate::FPS_60] {
            let boundary = fps.tick_of(2);
            assert_eq!(frame_count(boundary + Tick(254), fps), 2);
            assert_eq!(frame_count(boundary + Tick(255), fps), 3);
            assert_eq!(frame_count(boundary - Tick(254), fps), 2);
            assert_eq!(frame_count(Tick(254), fps), 0);
            assert_eq!(frame_count(Tick(255), fps), 1);
        }
    }

    #[test]
    fn lookup_floors_and_clips() {
        let fps = FrameRate::FPS_25;
        assert_eq!(frame_at(Tick(-1), fps, 100), 0);
        assert_eq!(frame_at(fps.tick_of(3) - Tick(1), fps, 100), 2);
        assert_eq!(frame_at(fps.tick_of(3), fps, 100), 3);
        assert_eq!(frame_at(Tick(i64::MAX), fps, 4), 3);
        assert_eq!(frame_at(Tick(i64::MAX), fps, 0), 0);
    }

    #[test]
    fn starts_are_exact_or_rounded_up_without_overflow() {
        for fps in [FrameRate::FPS_24, FrameRate::FPS_25, FrameRate::FPS_29_97, FrameRate::FPS_60] {
            assert_eq!(frame_time(37, fps), Some(fps.tick_of(37)));
        }
        let exotic = FrameRate::new(9379, 250);
        assert_eq!(frame_time(1, exotic), Some(Tick(6_770_871_095)));
        assert_eq!(frame_time(u64::MAX, FrameRate::FPS_24), None);
        assert_eq!(frame_time(2, FrameRate { num: i64::MAX, den: i64::MAX }), Some(Tick(2 * TICKS_PER_SECOND)));
        assert_eq!(frame_time(1, FrameRate { num: i64::MAX, den: 1 }), None);
    }

    #[test]
    fn invalid_rates_are_total() {
        for fps in [FrameRate { num: 0, den: 1 }, FrameRate { num: 25, den: 0 }, FrameRate { num: -1, den: 1 }, FrameRate { num: 1, den: -1 }] {
            assert_eq!(frame_count(Tick(i64::MAX), fps), 0);
            assert_eq!(frame_at(Tick(i64::MAX), fps, u64::MAX), 0);
            assert_eq!(frame_time(0, fps), None);
        }
        let fast = FrameRate { num: i64::MAX, den: 1 };
        assert_eq!(frame_count(Tick(i64::MAX), fast), u64::MAX);
        assert_eq!(frame_at(Tick(i64::MAX), fast, u64::MAX), u64::MAX - 1);
    }

    #[test]
    fn chunks_align_clip_and_handle_zero_and_maximum() {
        assert_eq!(chunk(17, 8, 100), 16..24);
        assert_eq!(chunk(99, 8, 100), 96..100);
        assert_eq!(chunk(u64::MAX, 8, 100), 96..100);
        assert_eq!(chunk(3, 0, 100), 3..4);
        assert_eq!(chunk(1, 3, 0), 0..0);
        assert_eq!(chunk(u64::MAX, u64::MAX - 1, u64::MAX), (u64::MAX - 1)..u64::MAX);
    }

    proptest! {
        #![proptest_config(ProptestConfig { rng_seed: proptest::test_runner::RngSeed::Fixed(0xB1), ..ProptestConfig::default() })]
        #[test]
        fn frame_start_roundtrips(k in 0u64..1_000_000, num in 1i64..1_000_001, den in 1i64..1_000_001) {
            let fps = FrameRate { num, den };
            if let Some(t) = frame_time(k, fps) {
                prop_assert_eq!(frame_at(t, fps, u64::MAX), k);
                if k > 0 { prop_assert_eq!(frame_at(t - Tick(1), fps, u64::MAX), k - 1); }
            } else {
                prop_assert!(i128::from(k) * i128::from(den) * i128::from(TICKS_PER_SECOND) / i128::from(num) > i128::from(i64::MAX));
            }
        }

        #[test]
        fn lookup_is_monotonic(a in any::<i64>(), b in any::<i64>(), num in 1i64..=i64::MAX, den in 1i64..=i64::MAX, count in any::<u64>()) {
            let fps = FrameRate { num, den };
            prop_assert!(frame_at(Tick(a.min(b)), fps, count) <= frame_at(Tick(a.max(b)), fps, count));
            prop_assert!(frame_at(Tick(a), fps, count) <= count.saturating_sub(1));
        }

        #[test]
        fn chunks_cover_clipped_index(k in any::<u64>(), len in any::<u64>(), count in 1u64..=u64::MAX) {
            let range = chunk(k, len, count);
            prop_assert!(range.contains(&k.min(count - 1)));
            prop_assert!(range.end <= count);
            prop_assert_eq!(range.start % len.max(1), 0);
        }
    }
}
