//! Legacy-compatible DCS transmitter turn-off state transitions.
//!
//! The compatibility transmitter state machine retains the decision to start
//! a DCS tail, waveform generation, PTT, and hardware ownership. This small
//! primitive advances only the scalar ACTIVE/TOC state and timer selected by
//! that caller, so an unavailable descriptor can fall back to the exact C
//! transition without moving any audio or device behavior.

use std::ffi::c_int;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK, timer};

/// Legacy transmitter state while normal DCS program audio is active.
pub const STATE_ACTIVE: i32 = 1;
/// Legacy transmitter state while the selected DCS turn-off tail is active.
pub const STATE_TOC: i32 = 2;

/// Immutable DCS turn-off duration selected by the compatibility caller.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DcsTurnoffConfig {
    /// DCS turn-off duration in whole milliseconds.
    pub turnoff_duration_ms: i32,
}

/// Native-callback input for one selected DCS turn-off transition.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DcsTurnoffInput {
    /// Whole native PCM milliseconds elapsed in this callback span.
    pub elapsed_ms: i32,
    /// One when the compatibility transmitter input PTT is asserted.
    pub tx_ptt_in: u32,
    /// One only when the compatibility caller has selected a new DCS tail.
    pub begin_turnoff: u32,
}

/// Caller-owned scalar state for a DCS transmitter turn-off transition.
///
/// The operation does not enter the finishing state itself. Instead it
/// reports the residual duration that the compatibility state machine passes
/// to its retained finishing helper. PTT, DCS waveform phase, and hardware
/// stay owned by that caller. Failed calls leave this POD unchanged.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct DcsTurnoffState {
    /// One of @ref STATE_ACTIVE or @ref STATE_TOC.
    pub tx_state: i32,
    /// Remaining DCS turn-off duration in whole milliseconds.
    pub dcs_turnoff_remaining_ms: i32,
    /// Compatibility transmit-hang time cleared when a tail begins.
    pub tx_hang_remaining_ms: i32,
    /// One when the compatibility caller must enter its finishing helper.
    pub finish_requested: u32,
    /// Native PCM milliseconds remaining after DCS-tail expiry.
    pub finish_elapsed_ms: i32,
}

/// Validate immutable DCS tail policy before a stream starts.
pub(crate) fn validate_config(config: &DcsTurnoffConfig) -> Result<(), c_int> {
    if config.turnoff_duration_ms <= 0 {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

fn validate_input(input: &DcsTurnoffInput) -> Result<(), c_int> {
    if input.elapsed_ms < 0 || input.tx_ptt_in > 1 || input.begin_turnoff > 1 {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

fn validate(config: &DcsTurnoffConfig, input: &DcsTurnoffInput) -> Result<(), c_int> {
    validate_config(config)?;
    validate_input(input)
}

/// Advance one selected DCS transmitter turn-off transition.
///
/// Entry consumes the callback duration immediately, matching the historical
/// C timer order. A rekey during TOC returns directly to ACTIVE and discards
/// the obsolete tail. Expiry reports, but does not perform, the retained C
/// finishing transition so that PTT and waveform behavior remain unchanged.
pub(crate) fn advance(
    config: &DcsTurnoffConfig,
    input: &DcsTurnoffInput,
    state: &mut DcsTurnoffState,
) -> Result<(), c_int> {
    validate(config, input)?;

    let mut next = *state;
    next.finish_requested = 0;
    next.finish_elapsed_ms = 0;

    match next.tx_state {
        STATE_ACTIVE => {
            if input.tx_ptt_in != 0 || input.begin_turnoff == 0 {
                return Err(RADIO_INVALID_ARGUMENT);
            }
            next.tx_state = STATE_TOC;
            next.dcs_turnoff_remaining_ms = config.turnoff_duration_ms;
            next.tx_hang_remaining_ms = 0;
            let residual = timer::consume(&mut next.dcs_turnoff_remaining_ms, input.elapsed_ms);
            if next.dcs_turnoff_remaining_ms == 0 {
                next.finish_requested = 1;
                next.finish_elapsed_ms = residual;
            }
        }
        STATE_TOC => {
            if input.begin_turnoff != 0 || next.dcs_turnoff_remaining_ms <= 0 {
                return Err(RADIO_INVALID_ARGUMENT);
            }
            if input.tx_ptt_in != 0 {
                next.dcs_turnoff_remaining_ms = 0;
                next.tx_state = STATE_ACTIVE;
            } else {
                let residual = timer::consume(&mut next.dcs_turnoff_remaining_ms, input.elapsed_ms);
                if next.dcs_turnoff_remaining_ms == 0 {
                    next.finish_requested = 1;
                    next.finish_elapsed_ms = residual;
                }
            }
        }
        _ => return Err(RADIO_INVALID_ARGUMENT),
    }

    *state = next;
    Ok(())
}

/// C ABI entry for one DCS transmitter turn-off state transition.
///
/// The function copies state before validation and advancement. It allocates
/// nothing, locks nothing, renders no PCM, and performs no I/O. A rejected
/// call leaves caller storage untouched for the exact compatibility fallback.
pub(crate) extern "C" fn radio_dcs_turnoff_advance(
    config: *const DcsTurnoffConfig,
    input: *const DcsTurnoffInput,
    state: *mut DcsTurnoffState,
) -> c_int {
    let Some(config) = NonNull::new(config.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(input) = NonNull::new(input.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };

    let config = unsafe { config.as_ref() };
    let input = unsafe { input.as_ref() };
    let mut next = unsafe { *state.as_ref() };
    match advance(config, input, &mut next) {
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
        DcsTurnoffConfig, DcsTurnoffInput, DcsTurnoffState, STATE_ACTIVE, STATE_TOC,
        radio_dcs_turnoff_advance,
    };
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    fn config() -> DcsTurnoffConfig {
        DcsTurnoffConfig {
            turnoff_duration_ms: 180,
        }
    }

    fn input(elapsed_ms: i32, tx_ptt_in: u32, begin_turnoff: u32) -> DcsTurnoffInput {
        DcsTurnoffInput {
            elapsed_ms,
            tx_ptt_in,
            begin_turnoff,
        }
    }

    #[test]
    fn entry_clears_hang_and_consumes_the_current_callback() {
        let mut state = DcsTurnoffState {
            tx_state: STATE_ACTIVE,
            dcs_turnoff_remaining_ms: 9,
            tx_hang_remaining_ms: 42,
            finish_requested: 99,
            finish_elapsed_ms: 77,
        };

        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &input(20, 0, 1), &mut state),
            RADIO_OK
        );
        assert_eq!(state.tx_state, STATE_TOC);
        assert_eq!(state.dcs_turnoff_remaining_ms, 160);
        assert_eq!(state.tx_hang_remaining_ms, 0);
        assert_eq!(state.finish_requested, 0);
        assert_eq!(state.finish_elapsed_ms, 0);
    }

    #[test]
    fn rekey_cancels_an_obsolete_dcs_tail_without_touching_hang() {
        let mut state = DcsTurnoffState {
            tx_state: STATE_TOC,
            dcs_turnoff_remaining_ms: 160,
            tx_hang_remaining_ms: 13,
            ..DcsTurnoffState::default()
        };

        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &input(20, 1, 0), &mut state),
            RADIO_OK
        );
        assert_eq!(state.tx_state, STATE_ACTIVE);
        assert_eq!(state.dcs_turnoff_remaining_ms, 0);
        assert_eq!(state.tx_hang_remaining_ms, 13);
        assert_eq!(state.finish_requested, 0);
    }

    #[test]
    fn expiry_reports_the_residual_for_the_retained_finishing_helper() {
        let mut state = DcsTurnoffState {
            tx_state: STATE_TOC,
            dcs_turnoff_remaining_ms: 15,
            ..DcsTurnoffState::default()
        };

        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &input(20, 0, 0), &mut state),
            RADIO_OK
        );
        assert_eq!(state.tx_state, STATE_TOC);
        assert_eq!(state.dcs_turnoff_remaining_ms, 0);
        assert_eq!(state.finish_requested, 1);
        assert_eq!(state.finish_elapsed_ms, 5);
    }

    #[test]
    fn callback_partitions_preserve_the_dcs_tail_deadline() {
        let mut whole = DcsTurnoffState {
            tx_state: STATE_ACTIVE,
            ..DcsTurnoffState::default()
        };
        let mut split = whole;

        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &input(40, 0, 1), &mut whole),
            RADIO_OK
        );
        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &input(20, 0, 1), &mut split),
            RADIO_OK
        );
        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &input(20, 0, 0), &mut split),
            RADIO_OK
        );
        assert_eq!(whole, split);
        assert_eq!(whole.dcs_turnoff_remaining_ms, 140);
    }

    #[test]
    fn invalid_snapshots_are_transactional() {
        let invalid_input = input(20, 2, 1);
        let zero_duration = DcsTurnoffConfig {
            turnoff_duration_ms: 0,
        };
        let original = DcsTurnoffState {
            tx_state: STATE_ACTIVE,
            dcs_turnoff_remaining_ms: 7,
            tx_hang_remaining_ms: 8,
            finish_requested: 1,
            finish_elapsed_ms: 9,
        };
        let mut state = original;

        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &invalid_input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, original);
        assert_eq!(
            radio_dcs_turnoff_advance(&zero_duration, &input(20, 0, 1), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, original);
        assert_eq!(
            radio_dcs_turnoff_advance(std::ptr::null(), &invalid_input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_dcs_turnoff_advance(&config(), std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_dcs_turnoff_advance(&config(), &invalid_input, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, original);
    }
}
