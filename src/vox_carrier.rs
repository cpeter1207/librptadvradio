//! Legacy-compatible VOX carrier-hang state.
//!
//! The historical radio loop reloads its VOX hold timer before consuming the
//! elapsed callback duration.  Its carrier result therefore reflects whether
//! the timer was positive before consumption, not whether it remains positive
//! afterward.  This small scalar operation preserves that ordering while the
//! compatibility adapter retains the envelope detector and all PCM handling.

use std::ffi::c_int;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK, timer};

/// Detector and elapsed-time snapshot for one VOX carrier update.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoxCarrierInput {
    /// One when the retained 8 kHz VOX envelope comparator is active.
    pub detector_active: u32,
    /// Configured legacy VOX hang duration in whole milliseconds.
    pub hang_time_ms: i32,
    /// Whole native PCM milliseconds elapsed in this callback span.
    pub elapsed_ms: i32,
}

/// Caller-owned timer and carrier decision for one VOX detector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct VoxCarrierState {
    /// Remaining legacy VOX carrier hold duration in whole milliseconds.
    pub remaining_ms: i32,
    /// One while the legacy carrier decision is asserted.
    pub carrier_detect: u32,
}

/// Advance the VOX hang state without crossing the C ABI boundary.
///
/// The owned native receive path shares this exact legacy ordering with the
/// compatibility wrapper below.
pub(crate) fn advance(input: &VoxCarrierInput, state: &mut VoxCarrierState) -> Result<(), c_int> {
    if input.detector_active > 1 {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    let mut next = *state;
    if input.detector_active != 0 {
        next.remaining_ms = input.hang_time_ms;
    }

    // Preserve the C branch exactly: it tests the timer before consuming the
    // current callback, so expiry remains reported as carrier for this call.
    if next.remaining_ms > 0 {
        let _ = timer::consume(&mut next.remaining_ms, input.elapsed_ms);
        next.carrier_detect = 1;
    } else {
        next.remaining_ms = 0;
        next.carrier_detect = 0;
    }
    *state = next;
    Ok(())
}

/// C ABI entry for one legacy VOX carrier-hang transition.
///
/// This function allocates nothing, locks nothing, and performs no I/O.  It
/// stages state before publishing it, so rejected snapshots leave the caller's
/// compatibility state untouched for the exact C fallback.
pub(crate) extern "C" fn radio_vox_carrier_advance(
    input: *const VoxCarrierInput,
    state: *mut VoxCarrierState,
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
    use super::{VoxCarrierInput, VoxCarrierState, radio_vox_carrier_advance};
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    fn advance(
        state: &mut VoxCarrierState,
        detector_active: u32,
        hang_time_ms: i32,
        elapsed_ms: i32,
    ) {
        let input = VoxCarrierInput {
            detector_active,
            hang_time_ms,
            elapsed_ms,
        };

        assert_eq!(radio_vox_carrier_advance(&input, state), RADIO_OK);
    }

    #[test]
    fn legacy_expiry_reports_carrier_for_the_current_callback() {
        let mut state = VoxCarrierState {
            remaining_ms: 20,
            carrier_detect: 0,
        };

        advance(&mut state, 0, 40, 20);
        assert_eq!(state.remaining_ms, 0);
        assert_eq!(state.carrier_detect, 1);

        advance(&mut state, 0, 40, 20);
        assert_eq!(state.remaining_ms, 0);
        assert_eq!(state.carrier_detect, 0);
    }

    #[test]
    fn detector_reload_precedes_current_callback_consumption() {
        let mut state = VoxCarrierState {
            remaining_ms: 1,
            carrier_detect: 0,
        };

        advance(&mut state, 1, 40, 20);
        assert_eq!(state.remaining_ms, 20);
        assert_eq!(state.carrier_detect, 1);
    }

    #[test]
    fn callback_partitions_preserve_stable_detector_hang_timing() {
        let mut whole = VoxCarrierState::default();
        let mut split = VoxCarrierState::default();

        advance(&mut whole, 1, 40, 20);
        for elapsed_ms in [0, 0, 20] {
            advance(&mut split, 1, 40, elapsed_ms);
        }
        assert_eq!(split, whole);

        advance(&mut whole, 0, 40, 20);
        for elapsed_ms in [0, 0, 20] {
            advance(&mut split, 0, 40, elapsed_ms);
        }
        assert_eq!(split, whole);

        advance(&mut whole, 0, 40, 20);
        for elapsed_ms in [0, 0, 20] {
            advance(&mut split, 0, 40, elapsed_ms);
        }
        assert_eq!(split, whole);
        assert_eq!(whole.carrier_detect, 0);
    }

    #[test]
    fn legacy_nonpositive_values_keep_their_original_behavior() {
        let mut state = VoxCarrierState {
            remaining_ms: 7,
            carrier_detect: 0,
        };

        advance(&mut state, 0, 40, -2);
        assert_eq!(state.remaining_ms, 7);
        assert_eq!(state.carrier_detect, 1);

        advance(&mut state, 1, -1, 20);
        assert_eq!(state.remaining_ms, 0);
        assert_eq!(state.carrier_detect, 0);
    }

    #[test]
    fn invalid_or_null_arguments_do_not_commit_state() {
        let invalid = VoxCarrierInput {
            detector_active: 2,
            hang_time_ms: 40,
            elapsed_ms: 20,
        };
        let initial = VoxCarrierState {
            remaining_ms: 17,
            carrier_detect: 1,
        };
        let mut state = initial;

        assert_eq!(
            radio_vox_carrier_advance(&invalid, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
        assert_eq!(
            radio_vox_carrier_advance(std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_vox_carrier_advance(&invalid, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
    }
}
