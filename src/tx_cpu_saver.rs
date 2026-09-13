//! Legacy-compatible transmitter CPU-saver state.
//!
//! The compatibility radio loop derives one halt flag from its configured
//! saver setting, logical PTT state, and transmitter state.  The calculation
//! has no PCM, device, or renderer ownership, so it can move independently
//! while the C caller retains the exact early-return behavior.

use std::ffi::c_int;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

/// Immutable transmitter activity snapshot for one CPU-saver update.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxCpuSaverInput {
    /// One when the legacy transmitter CPU saver is enabled.
    pub enabled: u32,
    /// One when logical transmitter input PTT is asserted.
    pub tx_ptt_in: u32,
    /// One when logical transmitter output PTT is asserted.
    pub tx_ptt_out: u32,
    /// One when the retained transmitter state machine is idle.
    pub tx_idle: u32,
}

/// Caller-owned transmitter CPU-saver result.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxCpuSaverState {
    /// One when the compatibility caller must skip transmitter rendering.
    pub halted: u32,
}

pub(crate) fn advance(input: &TxCpuSaverInput, state: &mut TxCpuSaverState) -> Result<(), c_int> {
    if input.enabled > 1 || input.tx_ptt_in > 1 || input.tx_ptt_out > 1 || input.tx_idle > 1 {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // This is algebraically identical to the historical two-branch update:
    // halt only while saving is enabled and every transmit activity indicator
    // is inactive; otherwise clear an existing halt.
    let next = TxCpuSaverState {
        halted: u32::from(
            input.enabled != 0
                && input.tx_ptt_in == 0
                && input.tx_ptt_out == 0
                && input.tx_idle != 0,
        ),
    };
    *state = next;
    Ok(())
}

/// C ABI entry for one legacy transmitter CPU-saver state transition.
///
/// This function allocates nothing, locks nothing, and performs no I/O. It
/// stages state before publishing it, so rejected snapshots leave the caller's
/// compatibility state untouched for the exact C fallback.
pub(crate) extern "C" fn radio_tx_cpu_saver_advance(
    input: *const TxCpuSaverInput,
    state: *mut TxCpuSaverState,
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
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{TxCpuSaverInput, TxCpuSaverState, radio_tx_cpu_saver_advance};
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    fn advance(
        state: &mut TxCpuSaverState,
        enabled: u32,
        tx_ptt_in: u32,
        tx_ptt_out: u32,
        tx_idle: u32,
    ) {
        let input = TxCpuSaverInput {
            enabled,
            tx_ptt_in,
            tx_ptt_out,
            tx_idle,
        };

        assert_eq!(radio_tx_cpu_saver_advance(&input, state), RADIO_OK);
    }

    #[test]
    fn halts_only_for_an_idle_unkeyed_saver_enabled_transmitter() {
        let mut state = TxCpuSaverState::default();

        advance(&mut state, 1, 0, 0, 1);
        assert_eq!(state.halted, 1);

        advance(&mut state, 1, 1, 0, 1);
        assert_eq!(state.halted, 0);
        advance(&mut state, 1, 0, 1, 1);
        assert_eq!(state.halted, 0);
        advance(&mut state, 1, 0, 0, 0);
        assert_eq!(state.halted, 0);
        advance(&mut state, 0, 0, 0, 1);
        assert_eq!(state.halted, 0);
    }

    #[test]
    fn invalid_or_null_arguments_do_not_commit_state() {
        let invalid = TxCpuSaverInput {
            enabled: 2,
            tx_ptt_in: 0,
            tx_ptt_out: 0,
            tx_idle: 1,
        };
        let initial = TxCpuSaverState { halted: 1 };
        let mut state = initial;

        assert_eq!(
            radio_tx_cpu_saver_advance(&invalid, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
        assert_eq!(
            radio_tx_cpu_saver_advance(std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_tx_cpu_saver_advance(&invalid, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
    }
}
