//! Post-transmit receive-blanking timing.
//!
//! The compatibility radio engine owns the mutable signed-16 capture buffer
//! and clears its left channel before the receive frontend.  This module owns
//! only the pure timer arithmetic that determines how many native frames are
//! protected.  Keeping PCM mutation in the caller preserves the established
//! hardware boundary and lets a rejected descriptor call use the exact C
//! fallback without changing callback ordering.

use std::ffi::c_int;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK, timer::FRAMES_PER_MILLISECOND};

/// Input snapshot for one post-transmit receive-blanking advance.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxBlankingInput {
    /// Whole milliseconds elapsed during this native callback.
    pub elapsed_ms: i32,
    /// Native frames supplied in this callback.
    pub native_frame_count: u32,
    /// Fractional-frame remainder before elapsed-time advancement.
    pub sample_remainder_before: u32,
}

/// Caller-owned blanking state updated after one protected native span.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxBlankingState {
    /// Remaining receive-blanking duration in whole milliseconds.
    pub remaining_ms: i32,
    /// Number of leading native frames that the caller must mute.
    pub blanked_frame_count: u32,
}

/// Advance the receiver-blanking timer without crossing the C ABI boundary.
///
/// The owned native receive path uses this exact arithmetic directly; the
/// exported compatibility entry below remains the adapter-facing wrapper.
pub(crate) fn advance(input: &RxBlankingInput, state: &mut RxBlankingState) -> Result<(), c_int> {
    if input.elapsed_ms < 0 || state.remaining_ms <= 0 {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // The C implementation multiplies only a positive signed-16 timer today,
    // but a u64 intermediate also preserves its comparison semantics for any
    // restored positive value without allowing an arithmetic wrap.
    let protected_frames = u64::from(state.remaining_ms as u32) * u64::from(FRAMES_PER_MILLISECOND);
    let after_remainder = protected_frames.saturating_sub(u64::from(input.sample_remainder_before));
    let mut next = *state;

    next.blanked_frame_count = after_remainder
        .min(u64::from(input.native_frame_count))
        .try_into()
        .expect("bounded by native_frame_count");
    next.remaining_ms = if input.elapsed_ms >= state.remaining_ms {
        0
    } else {
        state.remaining_ms - input.elapsed_ms
    };
    *state = next;
    Ok(())
}

/// C ABI entry for one post-transmit receive-blanking state transition.
///
/// The caller advances its fractional sample remainder before this call and
/// performs the resulting PCM mute afterward.  This operation allocates
/// nothing, locks nothing, and performs no I/O.  On failure it leaves state
/// unchanged so the compatibility adapter can retain its C arithmetic.
pub(crate) extern "C" fn radio_rx_blanking_advance(
    input: *const RxBlankingInput,
    state: *mut RxBlankingState,
) -> c_int {
    let Some(input) = NonNull::new(input.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };

    let input = unsafe { input.as_ref() };
    let mut next = unsafe { *state.as_ref() };
    match advance(input, &mut next) {
        Ok(()) => {
            unsafe { *state.as_mut() = next };
            RADIO_OK
        }
        Err(result) => result,
    }
}

#[cfg(test)]
mod tests {
    use super::{RxBlankingInput, RxBlankingState, radio_rx_blanking_advance};
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    #[test]
    fn whole_callback_blanks_and_expires_at_the_legacy_boundary() {
        let input = RxBlankingInput {
            elapsed_ms: 20,
            native_frame_count: 960,
            sample_remainder_before: 0,
        };
        let mut state = RxBlankingState {
            remaining_ms: 20,
            blanked_frame_count: 0,
        };

        assert_eq!(radio_rx_blanking_advance(&input, &mut state), RADIO_OK);
        assert_eq!(state.blanked_frame_count, 960);
        assert_eq!(state.remaining_ms, 0);
    }

    #[test]
    fn submillisecond_callback_partition_preserves_the_protected_prefix() {
        let mut state = RxBlankingState {
            remaining_ms: 1,
            blanked_frame_count: 0,
        };
        let first = RxBlankingInput {
            elapsed_ms: 0,
            native_frame_count: 36,
            sample_remainder_before: 0,
        };
        let second = RxBlankingInput {
            elapsed_ms: 1,
            native_frame_count: 36,
            sample_remainder_before: 36,
        };

        assert_eq!(radio_rx_blanking_advance(&first, &mut state), RADIO_OK);
        assert_eq!(state.blanked_frame_count, 36);
        assert_eq!(state.remaining_ms, 1);
        assert_eq!(radio_rx_blanking_advance(&second, &mut state), RADIO_OK);
        assert_eq!(state.blanked_frame_count, 12);
        assert_eq!(state.remaining_ms, 0);
    }

    #[test]
    fn unusual_restored_remainder_uses_the_legacy_zero_prefix_rule() {
        let input = RxBlankingInput {
            elapsed_ms: 1,
            native_frame_count: 48,
            sample_remainder_before: u32::MAX,
        };
        let mut state = RxBlankingState {
            remaining_ms: 1,
            blanked_frame_count: 99,
        };

        assert_eq!(radio_rx_blanking_advance(&input, &mut state), RADIO_OK);
        assert_eq!(state.blanked_frame_count, 0);
        assert_eq!(state.remaining_ms, 0);
    }

    #[test]
    fn zero_span_keeps_an_active_timer_without_muting() {
        let input = RxBlankingInput {
            elapsed_ms: 0,
            native_frame_count: 0,
            sample_remainder_before: 0,
        };
        let mut state = RxBlankingState {
            remaining_ms: 1,
            blanked_frame_count: 99,
        };

        assert_eq!(radio_rx_blanking_advance(&input, &mut state), RADIO_OK);
        assert_eq!(state.blanked_frame_count, 0);
        assert_eq!(state.remaining_ms, 1);
    }

    #[test]
    fn invalid_input_and_nulls_do_not_commit_state() {
        let invalid = RxBlankingInput {
            elapsed_ms: -1,
            native_frame_count: 48,
            sample_remainder_before: 0,
        };
        let original = RxBlankingState {
            remaining_ms: 1,
            blanked_frame_count: 19,
        };
        let mut state = original;

        assert_eq!(
            radio_rx_blanking_advance(&invalid, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, original);
        assert_eq!(
            radio_rx_blanking_advance(std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_rx_blanking_advance(&invalid, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, original);
    }
}
