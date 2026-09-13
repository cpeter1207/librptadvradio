//! Legacy-compatible transmitter completion cleanup.
//!
//! This operation owns the small state update that follows a completed
//! transmitter drain.  It deliberately does not key hardware, clear the
//! compatibility display string, select CTCSS/DCS, or render PCM.  Those
//! responsibilities remain with the compatibility adapter, which can retain
//! its established C cleanup when an older descriptor lacks this member.

use std::ffi::c_int;
use std::mem::size_of;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

/// Legacy idle transmitter state after the final drain has completed.
pub const STATE_IDLE: i32 = 0;
/// Historical CTCSS renderer request that disables output on its next pass.
const CTCSS_OPTION_DISABLE: u32 = 3;

/// Immutable configuration for one transmitter completion cleanup.
///
/// The compatibility caller supplies the configured blanking duration exactly
/// as it stores it.  In particular, this primitive does not normalize a
/// legacy negative value; retaining it lets the C fallback and portable path
/// produce the same observable timer state.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxCompleteConfig {
    /// Size supplied by the caller for append-only ABI validation.
    pub struct_size: u32,
    /// Receiver blanking duration armed immediately after transmitter release.
    pub txrx_blanking_time_ms: i32,
}

/// Caller-owned state updated when a transmitter finishing drain completes.
///
/// This compact POD contains only scalar state written by the historical
/// completion branch.  Device PTT, CTCSS display strings, waveform state, and
/// all output I/O intentionally remain outside this operation.  A rejected
/// call leaves caller storage unchanged.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TxCompleteState {
    /// Legacy transmitter state after cleanup.
    pub tx_state: i32,
    /// Logical transmitter PTT output after cleanup.
    pub tx_ptt_out: u32,
    /// One-shot CTCSS renderer option after cleanup.
    pub tx_ctcss_option: u32,
    /// Receiver blanking time armed after cleanup.
    pub txrx_blanking_timer_ms: i32,
    /// Fractional native-sample remainder for the blanking timer.
    pub txrx_blanking_sample_remainder: u32,
    /// Historical indication that CTCSS output state is ready for the renderer.
    pub tx_ctcss_ready: u32,
}

/// Validate immutable post-transmit cleanup policy before a stream starts.
pub(crate) fn validate_config(config: &TxCompleteConfig) -> Result<(), c_int> {
    if config.struct_size < size_of::<TxCompleteConfig>() as u32 {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

pub(crate) fn complete(
    config: &TxCompleteConfig,
    state: &mut TxCompleteState,
) -> Result<(), c_int> {
    validate_config(config)?;

    // Copy before publishing the result so an invalid ABI snapshot cannot
    // perturb compatibility state.  Every field is intentionally overwritten:
    // the original C branch did the same after its `hit` condition.
    let mut next = *state;
    next.tx_ptt_out = 0;
    next.tx_ctcss_option = CTCSS_OPTION_DISABLE;
    next.txrx_blanking_timer_ms = config.txrx_blanking_time_ms;
    next.txrx_blanking_sample_remainder = 0;
    next.tx_state = STATE_IDLE;
    next.tx_ctcss_ready = 1;
    *state = next;
    Ok(())
}

/// C ABI entry for the post-drain transmitter completion cleanup.
///
/// The call allocates nothing, locks nothing, and performs no device I/O.
/// Failure leaves the caller state unchanged so the compatibility adapter can
/// run its retained cleanup branch unchanged.
pub(crate) extern "C" fn radio_tx_complete(
    config: *const TxCompleteConfig,
    state: *mut TxCompleteState,
) -> c_int {
    let Some(config) = NonNull::new(config.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };

    let config = unsafe { config.as_ref() };
    let mut next = unsafe { *state.as_ref() };
    match complete(config, &mut next) {
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
    use super::{STATE_IDLE, TxCompleteConfig, TxCompleteState, radio_tx_complete};
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    fn config(blanking_time_ms: i32) -> TxCompleteConfig {
        TxCompleteConfig {
            struct_size: std::mem::size_of::<TxCompleteConfig>() as u32,
            txrx_blanking_time_ms: blanking_time_ms,
        }
    }

    #[test]
    fn completion_matches_the_legacy_scalar_cleanup() {
        let mut state = TxCompleteState {
            tx_state: 5,
            tx_ptt_out: 1,
            tx_ctcss_option: 1,
            txrx_blanking_timer_ms: 9,
            txrx_blanking_sample_remainder: 17,
            tx_ctcss_ready: 0,
        };

        assert_eq!(radio_tx_complete(&config(125), &mut state), RADIO_OK);
        assert_eq!(state.tx_state, STATE_IDLE);
        assert_eq!(state.tx_ptt_out, 0);
        assert_eq!(state.tx_ctcss_option, 3);
        assert_eq!(state.txrx_blanking_timer_ms, 125);
        assert_eq!(state.txrx_blanking_sample_remainder, 0);
        assert_eq!(state.tx_ctcss_ready, 1);
    }

    #[test]
    fn completion_preserves_the_legacy_unvalidated_blanking_value() {
        let mut state = TxCompleteState::default();

        assert_eq!(radio_tx_complete(&config(-1), &mut state), RADIO_OK);
        assert_eq!(state.txrx_blanking_timer_ms, -1);
    }

    #[test]
    fn invalid_configuration_and_nulls_do_not_commit_state() {
        let mut invalid = config(20);
        let initial = TxCompleteState {
            tx_state: 5,
            tx_ptt_out: 1,
            tx_ctcss_option: 2,
            txrx_blanking_timer_ms: 11,
            txrx_blanking_sample_remainder: 7,
            tx_ctcss_ready: 0,
        };
        let mut state = initial;

        invalid.struct_size = 0;
        assert_eq!(
            radio_tx_complete(&invalid, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
        assert_eq!(
            radio_tx_complete(std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_tx_complete(&config(20), std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
    }
}
