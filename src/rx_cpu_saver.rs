//! Legacy-compatible receiver CPU-saver transition state.
//!
//! The compatibility receive loop has one scalar predicate that halts DSP only
//! while the receiver is idle, unkeyed, and not participating in a signaling
//! mode.  This module owns that predicate and transition label only.  The C
//! adapter still owns the physical DSP-stage enable writes at their historical
//! position immediately before the native receive frontend.

use std::ffi::c_int;
#[cfg(test)]
use std::ptr::NonNull;

use crate::RADIO_INVALID_ARGUMENT;
#[cfg(test)]
use crate::RADIO_OK;

/// No receiver CPU-saver transition is required.
const ACTION_NONE: u32 = 0;
/// The compatibility adapter must enter receiver CPU saving.
const ACTION_ENTER: u32 = 1;
/// The compatibility adapter must leave receiver CPU saving.
const ACTION_LEAVE: u32 = 2;

/// Immutable receiver activity snapshot for one CPU-saver decision.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxCpuSaverInput {
    /// One when the legacy receiver CPU saver is enabled.
    pub enabled: u32,
    /// One when the retained carrier decision is asserted.
    pub carrier_detect: u32,
    /// One when the retained signaling state is idle.
    pub signal_mode_null: u32,
    /// One when logical transmitter input PTT is asserted.
    pub tx_ptt_in: u32,
    /// One when logical transmitter output PTT is asserted.
    pub tx_ptt_out: u32,
}

/// Caller-owned receiver halt state and stage-transition action.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RxCpuSaverState {
    /// One while the compatibility receive DSP is halted.
    pub halted: u32,
    /// Zero for no change, one for enter, or two for leave.
    pub action: u32,
}

pub(crate) fn advance(input: &RxCpuSaverInput, state: &mut RxCpuSaverState) -> Result<(), c_int> {
    if input.enabled > 1
        || input.carrier_detect > 1
        || input.signal_mode_null > 1
        || input.tx_ptt_in > 1
        || input.tx_ptt_out > 1
        || state.halted > 1
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // This is the historical C predicate exactly.  The transition label lets
    // the adapter retain ownership of its HPF/deemphasis enable writes.
    let halted = u32::from(
        input.enabled != 0
            && input.carrier_detect == 0
            && input.signal_mode_null != 0
            && input.tx_ptt_in == 0
            && input.tx_ptt_out == 0,
    );
    let action = if halted == state.halted {
        ACTION_NONE
    } else if halted != 0 {
        ACTION_ENTER
    } else {
        ACTION_LEAVE
    };
    *state = RxCpuSaverState { halted, action };
    Ok(())
}

/// C ABI entry for one legacy receiver CPU-saver transition.
///
/// The operation allocates nothing, locks nothing, and performs no I/O.  It
/// stages the result before publishing it, so a rejected snapshot leaves the
/// compatibility caller's state available to its exact C fallback.
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
pub(crate) extern "C" fn radio_rx_cpu_saver_advance(
    input: *const RxCpuSaverInput,
    state: *mut RxCpuSaverState,
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
    use super::{
        ACTION_ENTER, ACTION_LEAVE, ACTION_NONE, RxCpuSaverInput, RxCpuSaverState,
        radio_rx_cpu_saver_advance,
    };
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    #[test]
    fn each_invalid_boolean_preserves_the_halt_state() {
        for field in 0..5 {
            let mut input = RxCpuSaverInput::default();
            let fields = [
                &mut input.enabled,
                &mut input.carrier_detect,
                &mut input.signal_mode_null,
                &mut input.tx_ptt_in,
                &mut input.tx_ptt_out,
            ];
            *fields[field] = 2;
            let initial = RxCpuSaverState {
                halted: 1,
                action: ACTION_ENTER,
            };
            let mut state = initial;
            assert_eq!(
                super::advance(&input, &mut state),
                Err(RADIO_INVALID_ARGUMENT)
            );
            assert_eq!(state, initial);
        }
    }

    #[test]
    fn every_boolean_snapshot_preserves_the_legacy_transition() {
        for enabled in 0..=1 {
            for carrier_detect in 0..=1 {
                for signal_mode_null in 0..=1 {
                    for tx_ptt_in in 0..=1 {
                        for tx_ptt_out in 0..=1 {
                            for prior_halted in 0..=1 {
                                let input = RxCpuSaverInput {
                                    enabled,
                                    carrier_detect,
                                    signal_mode_null,
                                    tx_ptt_in,
                                    tx_ptt_out,
                                };
                                let mut state = RxCpuSaverState {
                                    halted: prior_halted,
                                    action: u32::MAX,
                                };
                                let expected_halted = u32::from(
                                    enabled != 0
                                        && carrier_detect == 0
                                        && signal_mode_null != 0
                                        && tx_ptt_in == 0
                                        && tx_ptt_out == 0,
                                );
                                let expected_action = if expected_halted == prior_halted {
                                    ACTION_NONE
                                } else if expected_halted != 0 {
                                    ACTION_ENTER
                                } else {
                                    ACTION_LEAVE
                                };

                                assert_eq!(
                                    radio_rx_cpu_saver_advance(&input, &mut state),
                                    RADIO_OK
                                );
                                assert_eq!(state.halted, expected_halted);
                                assert_eq!(state.action, expected_action);
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_or_null_arguments_do_not_commit_state() {
        let invalid = RxCpuSaverInput {
            enabled: 2,
            carrier_detect: 0,
            signal_mode_null: 1,
            tx_ptt_in: 0,
            tx_ptt_out: 0,
        };
        let initial = RxCpuSaverState {
            halted: 1,
            action: ACTION_ENTER,
        };
        let mut state = initial;

        assert_eq!(
            radio_rx_cpu_saver_advance(&invalid, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);
        assert_eq!(
            radio_rx_cpu_saver_advance(std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_rx_cpu_saver_advance(&invalid, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);

        state.halted = 2;
        assert_eq!(
            radio_rx_cpu_saver_advance(
                &RxCpuSaverInput {
                    enabled: 1,
                    carrier_detect: 0,
                    signal_mode_null: 1,
                    tx_ptt_in: 0,
                    tx_ptt_out: 0,
                },
                &mut state,
            ),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state.halted, 2);
    }
}
