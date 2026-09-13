//! Legacy-compatible transmitter finishing-drain transitions.
//!
//! The compatibility radio state machine selects when normal transmitter
//! draining begins.  This narrow primitive owns only the fixed 80 ms normal
//! drain that follows that selection: it does not inspect PTT, generate PCM,
//! select signaling, or interact with a device.  Keeping the operation
//! isolated lets a caller retain its established C path if this append-only
//! descriptor member is unavailable or rejects a snapshot.

use std::ffi::c_int;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK, timer};

/// Legacy transmitter state while the final output drain is in progress.
pub const STATE_FINISHING: i32 = 4;
/// Legacy transmitter state after the final output drain completes.
pub const STATE_COMPLETE: i32 = 5;

/// Number of legacy 20 ms output spans cleared by a normal finish request.
const NORMAL_BUFFER_CLEAR_FRAMES: i32 = 3;
/// Duration of one historical transmitter processing span in milliseconds.
const LEGACY_FRAME_MS: i32 = 20;
/// Largest normal or 55 Hz-tail historical buffer-clear count.
const MAX_COMPATIBILITY_BUFFER_CLEAR_FRAMES: i32 = 8;

/// Elapsed native PCM duration for one normal finishing-drain entry.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxFinishInput {
    /// Whole elapsed milliseconds in the callback that enters the drain.
    pub elapsed_ms: i32,
}

/// Caller-owned transmitter state changed by one normal finishing-drain entry.
///
/// This POD represents only the three fields written by the historical
/// `urp_radio_enter_finishing()` helper.  The caller retains PTT, CTCSS/DCS,
/// transmit hang, blanking, and all hardware interaction.  A failed call does
/// not modify this structure.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxFinishState {
    /// Remaining compatibility output buffers to clear after this callback.
    pub buffer_clear_frames: i32,
    /// Remaining normal finishing drain duration in milliseconds.
    pub finish_remaining_ms: i32,
    /// One of @ref STATE_FINISHING or @ref STATE_COMPLETE after success.
    pub tx_state: i32,
}

pub(crate) fn advance(input: &TxFinishInput, state: &mut TxFinishState) -> Result<(), c_int> {
    if input.elapsed_ms < 0 {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // Copy before changing the caller-owned ABI storage.  Although this entry
    // overwrites every state field, the copy keeps the FFI transaction rule
    // explicit and protects it if validation expands with a future ABI tail.
    let mut next = *state;
    next.buffer_clear_frames = NORMAL_BUFFER_CLEAR_FRAMES;
    next.finish_remaining_ms = (NORMAL_BUFFER_CLEAR_FRAMES + 1) * LEGACY_FRAME_MS;
    next.tx_state = STATE_FINISHING;
    let _ = timer::consume(&mut next.finish_remaining_ms, input.elapsed_ms);
    if next.finish_remaining_ms == 0 {
        next.buffer_clear_frames = 0;
        next.tx_state = STATE_COMPLETE;
    }

    *state = next;
    Ok(())
}

/// Advance one already-entered historical transmitter finishing drain.
///
/// The compatibility state machine calls this only while its transmitter state
/// is `STATE_FINISHING`.  A restored historical buffer count with no timer
/// re-seeds the duration exactly as the retained C continuation does; this is
/// needed for the existing 55 Hz tail and persisted compatibility state.
pub(crate) fn advance_continuation(
    input: &TxFinishInput,
    state: &mut TxFinishState,
) -> Result<(), c_int> {
    if input.elapsed_ms < 0
        || state.tx_state != STATE_FINISHING
        || state.buffer_clear_frames < 0
        || state.buffer_clear_frames > MAX_COMPATIBILITY_BUFFER_CLEAR_FRAMES
        || state.finish_remaining_ms < 0
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    let mut next = *state;
    if next.finish_remaining_ms == 0 && next.buffer_clear_frames > 0 {
        next.finish_remaining_ms = next.buffer_clear_frames * LEGACY_FRAME_MS;
    }
    let _ = timer::consume(&mut next.finish_remaining_ms, input.elapsed_ms);
    if next.finish_remaining_ms == 0 {
        next.buffer_clear_frames = 0;
        next.tx_state = STATE_COMPLETE;
    }

    *state = next;
    Ok(())
}

/// C ABI entry for one normal legacy transmitter finishing-drain transition.
///
/// The fixed normal transition clears three historical output buffers and
/// holds the transmitter through the entry callback plus those three 20 ms
/// spans.  The supplied callback duration is consumed immediately, so a late
/// transition can complete in the same native callback exactly as the legacy
/// C helper does.  The call allocates nothing, locks nothing, and performs no
/// I/O.
pub(crate) extern "C" fn radio_tx_finish_advance(
    input: *const TxFinishInput,
    state: *mut TxFinishState,
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

/// C ABI entry for one already-entered legacy transmitter finishing drain.
///
/// This preserves the historical timer reseed rule and duration-based
/// completion state.  It allocates nothing, locks nothing, and performs no
/// I/O.  Rejected snapshots leave caller state untouched so the compatibility
/// adapter can execute its retained C continuation.
pub(crate) extern "C" fn radio_tx_finish_continue(
    input: *const TxFinishInput,
    state: *mut TxFinishState,
) -> c_int {
    let Some(input) = NonNull::new(input.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };

    let input = unsafe { input.as_ref() };
    let mut next = unsafe { *state.as_ref() };
    match advance_continuation(input, &mut next) {
        Ok(()) => {
            unsafe { *state.as_mut() = next };
            RADIO_OK
        }
        Err(result) => result,
    }
}

#[cfg(test)]
mod tests {
    use super::{
        STATE_COMPLETE, STATE_FINISHING, TxFinishInput, TxFinishState, radio_tx_finish_advance,
        radio_tx_finish_continue,
    };
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    fn input(elapsed_ms: i32) -> TxFinishInput {
        TxFinishInput { elapsed_ms }
    }

    #[test]
    fn normal_entry_preserves_the_legacy_four_span_drain() {
        let mut state = TxFinishState {
            buffer_clear_frames: 99,
            finish_remaining_ms: 9,
            tx_state: 1,
        };

        assert_eq!(radio_tx_finish_advance(&input(20), &mut state), RADIO_OK);
        assert_eq!(state.buffer_clear_frames, 3);
        assert_eq!(state.finish_remaining_ms, 60);
        assert_eq!(state.tx_state, STATE_FINISHING);
    }

    #[test]
    fn entry_consumes_the_full_callback_before_publishing_completion() {
        let mut state = TxFinishState::default();

        assert_eq!(radio_tx_finish_advance(&input(80), &mut state), RADIO_OK);
        assert_eq!(state.buffer_clear_frames, 0);
        assert_eq!(state.finish_remaining_ms, 0);
        assert_eq!(state.tx_state, STATE_COMPLETE);

        assert_eq!(radio_tx_finish_advance(&input(113), &mut state), RADIO_OK);
        assert_eq!(state.buffer_clear_frames, 0);
        assert_eq!(state.finish_remaining_ms, 0);
        assert_eq!(state.tx_state, STATE_COMPLETE);
    }

    #[test]
    fn zero_elapsed_entry_holds_the_complete_legacy_duration() {
        let mut state = TxFinishState::default();

        assert_eq!(radio_tx_finish_advance(&input(0), &mut state), RADIO_OK);
        assert_eq!(state.buffer_clear_frames, 3);
        assert_eq!(state.finish_remaining_ms, 80);
        assert_eq!(state.tx_state, STATE_FINISHING);
    }

    #[test]
    fn rejected_input_leaves_the_caller_state_unchanged() {
        let initial = TxFinishState {
            buffer_clear_frames: 3,
            finish_remaining_ms: 40,
            tx_state: STATE_FINISHING,
        };
        let mut state = initial;

        assert_eq!(
            radio_tx_finish_advance(&input(-1), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
    }

    #[test]
    fn continuation_reseeds_restored_history_and_finishes_on_the_exact_boundary() {
        let mut state = TxFinishState {
            buffer_clear_frames: 3,
            finish_remaining_ms: 0,
            tx_state: STATE_FINISHING,
        };

        assert_eq!(radio_tx_finish_continue(&input(20), &mut state), RADIO_OK);
        assert_eq!(state.finish_remaining_ms, 40);
        assert_eq!(state.buffer_clear_frames, 3);
        assert_eq!(state.tx_state, STATE_FINISHING);
        assert_eq!(radio_tx_finish_continue(&input(20), &mut state), RADIO_OK);
        assert_eq!(state.finish_remaining_ms, 20);
        assert_eq!(radio_tx_finish_continue(&input(20), &mut state), RADIO_OK);
        assert_eq!(state.finish_remaining_ms, 0);
        assert_eq!(state.buffer_clear_frames, 0);
        assert_eq!(state.tx_state, STATE_COMPLETE);
    }

    #[test]
    fn continuation_completes_an_empty_or_invalid_snapshot_transactionally() {
        let mut empty = TxFinishState {
            tx_state: STATE_FINISHING,
            ..TxFinishState::default()
        };
        let invalid = TxFinishState {
            buffer_clear_frames: 9,
            finish_remaining_ms: 0,
            tx_state: STATE_FINISHING,
        };
        let mut state = invalid;

        assert_eq!(radio_tx_finish_continue(&input(1), &mut empty), RADIO_OK);
        assert_eq!(empty.tx_state, STATE_COMPLETE);
        assert_eq!(
            radio_tx_finish_continue(&input(1), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, invalid);
        state.tx_state = STATE_FINISHING;
        assert_eq!(
            radio_tx_finish_continue(&input(-1), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state.tx_state, STATE_FINISHING);
    }

    #[test]
    fn ffi_rejects_null_arguments_without_committing_state() {
        let input = input(20);
        let initial = TxFinishState {
            buffer_clear_frames: 3,
            finish_remaining_ms: 60,
            tx_state: STATE_FINISHING,
        };
        let mut state = initial;

        assert_eq!(
            radio_tx_finish_advance(std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_tx_finish_advance(&input, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
    }
}
