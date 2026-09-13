//! Sample-clocked timer arithmetic for the fixed 48 kHz native stream.
//!
//! The compatibility radio core records timer state in milliseconds while
//! callbacks can contain any bounded number of native PCM frames.  These pure
//! helpers preserve elapsed time across callback partitions without relying on
//! a fixed callback duration.

/// Native stream rate retained by the current CM119-compatible radio core.
pub(crate) const NATIVE_SAMPLE_RATE_HZ: u32 = 48_000;

/// Native PCM frames in one whole millisecond at [`NATIVE_SAMPLE_RATE_HZ`].
pub(crate) const FRAMES_PER_MILLISECOND: u32 = NATIVE_SAMPLE_RATE_HZ / 1_000;

/// Convert one bounded native-frame span to whole elapsed milliseconds.
///
/// `remainder` retains sub-millisecond native frames between calls.  The
/// `u32` input bound makes the quotient representable as `i32`, including an
/// accidentally oversized persisted remainder, while preserving the existing
/// compatibility arithmetic exactly.
#[must_use]
pub(crate) fn elapsed_ms(remainder: &mut u32, native_frames: u32) -> i32 {
    let elapsed = u64::from(*remainder) + u64::from(native_frames);
    let frames_per_millisecond = u64::from(FRAMES_PER_MILLISECOND);
    let milliseconds = (elapsed / frames_per_millisecond) as i32;

    *remainder = (elapsed % frames_per_millisecond) as u32;
    milliseconds
}

/// Consume elapsed milliseconds from a timer and return time after expiry.
///
/// A nonpositive timer or elapsed duration leaves the timer unchanged and
/// returns the supplied duration.  Otherwise, expiry clears the timer and
/// returns its residual duration so a successor state begins at the same PCM
/// sample time.
#[must_use]
pub(crate) fn consume(timer: &mut i32, milliseconds: i32) -> i32 {
    if *timer <= 0 || milliseconds <= 0 {
        return milliseconds;
    }
    if milliseconds >= *timer {
        let residual = milliseconds - *timer;
        *timer = 0;
        return residual;
    }
    *timer -= milliseconds;
    0
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{FRAMES_PER_MILLISECOND, NATIVE_SAMPLE_RATE_HZ, consume, elapsed_ms};

    #[test]
    fn callback_partitions_preserve_elapsed_time_and_expiration_sample() {
        let mut whole_remainder = 0;
        let whole_elapsed = elapsed_ms(&mut whole_remainder, 960);
        let mut whole_timer = 17;
        let whole_residual = consume(&mut whole_timer, whole_elapsed);

        let mut split_remainder = 0;
        let mut split_timer = 17;
        let mut split_residual = 0;
        for frames in [1, 47, 480, 432] {
            split_residual = consume(&mut split_timer, elapsed_ms(&mut split_remainder, frames));
        }

        assert_eq!(whole_elapsed, 20);
        assert_eq!(whole_remainder, split_remainder);
        assert_eq!(whole_timer, split_timer);
        assert_eq!(whole_residual, split_residual);
    }

    #[test]
    fn fractional_remainder_is_carried_exactly_between_callbacks() {
        let mut remainder = FRAMES_PER_MILLISECOND - 1;

        assert_eq!(elapsed_ms(&mut remainder, 0), 0);
        assert_eq!(remainder, FRAMES_PER_MILLISECOND - 1);
        assert_eq!(elapsed_ms(&mut remainder, 1), 1);
        assert_eq!(remainder, 0);
        assert_eq!(elapsed_ms(&mut remainder, FRAMES_PER_MILLISECOND - 1), 0);
        assert_eq!(remainder, FRAMES_PER_MILLISECOND - 1);
        assert_eq!(NATIVE_SAMPLE_RATE_HZ, 48_000);
    }

    #[test]
    fn nonpositive_timer_or_duration_is_a_noop() {
        let mut expired = 0;
        assert_eq!(consume(&mut expired, 7), 7);
        assert_eq!(expired, 0);

        let mut disabled = -3;
        assert_eq!(consume(&mut disabled, 7), 7);
        assert_eq!(disabled, -3);

        let mut active = 7;
        assert_eq!(consume(&mut active, 0), 0);
        assert_eq!(active, 7);
        assert_eq!(consume(&mut active, -2), -2);
        assert_eq!(active, 7);
    }

    #[test]
    fn expiration_returns_only_the_residual_duration() {
        let mut exact = 10;
        assert_eq!(consume(&mut exact, 10), 0);
        assert_eq!(exact, 0);

        let mut late = 10;
        assert_eq!(consume(&mut late, 14), 4);
        assert_eq!(late, 0);

        let mut early = 10;
        assert_eq!(consume(&mut early, 9), 0);
        assert_eq!(early, 1);
    }
}
