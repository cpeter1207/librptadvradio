//! Legacy-compatible receive signaling-mode selection.
//!
//! This narrow operation owns only the CTCSS/DCS mode hold and CTCSS
//! transmit-tone selection from the historical radio-signaling loop.  It has
//! no PCM, device, Asterisk, allocation, logging, or synchronization
//! dependency.  The compatibility adapter retains configuration parsing and
//! copies its selected fields into the caller-owned ABI state transactionally.

use std::ffi::c_int;
use std::mem::size_of;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK, ctcss_receive, timer};

/// Legacy no-signaling mode.
pub const MODE_NONE: i32 = 0;
/// Legacy CTCSS signaling mode.
pub const MODE_CTCSS: i32 = 2;
/// Legacy DCS signaling mode.
pub const MODE_DCS: i32 = 3;

/// Legacy sentinel meaning that no CTCSS tone decoded.
pub const CTCSS_NONE: i32 = -1;

/// Number of CTCSS mapping entries in the public ABI.
pub const TONE_COUNT: usize = ctcss_receive::TONE_COUNT;

/// Immutable C-compatible signaling-mode configuration.
///
/// The compatibility control plane constructs this value from its already
/// parsed CTCSS configuration.  Mapping entries are exact precomputed tenths
/// of hertz: zero means receive-only, while a nonzero default retains the
/// historical no-default sentinel when one was configured by the legacy
/// parser.  The operation never retains this pointer.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalModeConfig {
    /// Size supplied by the caller for append-only ABI validation.
    pub struct_size: u32,
    /// Configured signaling-mode hold time in milliseconds.
    pub hold_ms: i32,
    /// One when a decoded CTCSS tone may select transmitter CTCSS.
    pub ctcss_tx_enabled: u32,
    /// Exact legacy default transmit CTCSS selection in tenths of hertz.
    pub default_tx_ctcss_frequency_tenths_hz: i32,
    /// Exact legacy mapped transmit CTCSS selections indexed by decoded tone.
    pub mapped_tx_ctcss_frequency_tenths_hz: [i32; TONE_COUNT],
}

/// One elapsed-time and decoder snapshot for a signaling-mode update.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SignalModeInput {
    /// Whole elapsed milliseconds for this callback span.
    pub elapsed_ms: i32,
    /// Decoded CTCSS table index, or @ref CTCSS_NONE when none qualifies.
    pub decoded_ctcss: i32,
    /// One while the native DCS decoder qualifies its configured code.
    pub dcs_valid: u32,
    /// One while transmitter input is asserted and the mode timer is frozen.
    pub tx_ptt_in: u32,
}

/// Caller-owned C-compatible signaling-mode state.
///
/// This is deliberately a compact copy of only the historical fields touched
/// by the mode resolver.  Transmitter startup and turn-off state continue to
/// own the same values in the compatibility adapter until their own migration
/// slice moves them together.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignalModeState {
    /// Current legacy signaling mode.
    pub smode: i32,
    /// Most recently selected legacy signaling mode.
    pub smode_was: i32,
    /// Remaining mode hold time in milliseconds.
    pub smode_timer_ms: i32,
    /// Previously acted-on received CTCSS table index.
    pub last_rx_ctcss: i32,
    /// Selected transmitter CTCSS frequency in tenths of hertz.
    pub tx_ctcss_frequency_tenths_hz: i32,
    /// Historical CTCSS renderer request; one requests a new selected tone.
    pub tx_ctcss_option: u32,
    /// Sticky historical indication that a signaling-mode hold expired.
    pub smode_turnoff: u32,
}

impl Default for SignalModeState {
    fn default() -> Self {
        Self {
            smode: MODE_NONE,
            smode_was: MODE_NONE,
            smode_timer_ms: 0,
            last_rx_ctcss: CTCSS_NONE,
            tx_ctcss_frequency_tenths_hz: 0,
            tx_ctcss_option: 0,
            smode_turnoff: 0,
        }
    }
}

impl SignalModeConfig {
    /// Return a zero-effect configuration with a valid ABI structure size.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            hold_ms: 0,
            ctcss_tx_enabled: 0,
            default_tx_ctcss_frequency_tenths_hz: 0,
            mapped_tx_ctcss_frequency_tenths_hz: [0; TONE_COUNT],
        }
    }
}

/// Validate immutable signaling-mode policy before a stream starts.
///
/// The composed receive/transmit owner uses this at control-plane setup so a
/// valid stream cannot later reject an otherwise valid native callback solely
/// because of immutable configuration.
pub(crate) fn validate_config(config: &SignalModeConfig) -> Result<(), c_int> {
    if config.struct_size < size_of::<SignalModeConfig>() as u32
        || config.ctcss_tx_enabled > 1
        || config
            .mapped_tx_ctcss_frequency_tenths_hz
            .iter()
            .any(|frequency| *frequency < 0)
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

fn validate_input(input: &SignalModeInput) -> Result<(), c_int> {
    if input.dcs_valid > 1
        || input.tx_ptt_in > 1
        || input.decoded_ctcss < CTCSS_NONE
        || input.decoded_ctcss >= TONE_COUNT as i32
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

/// Advance one legacy-compatible signaling-mode transition transactionally.
///
/// The mode timer consumes only supplied elapsed milliseconds and is frozen
/// while PTT input is present.  CTCSS gets first claim when the current mode
/// permits it; a valid DCS result then gets the same claim only when CTCSS did
/// not select the mode.  This preserves the original ordering exactly.
pub(crate) fn advance(
    config: &SignalModeConfig,
    input: &SignalModeInput,
    state: &mut SignalModeState,
) -> Result<(), c_int> {
    validate_config(config)?;
    validate_input(input)?;

    let mut next = *state;

    if next.smode_timer_ms > 0 && input.tx_ptt_in == 0 {
        let _ = timer::consume(&mut next.smode_timer_ms, input.elapsed_ms);
        if next.smode_timer_ms == 0 {
            next.smode_was = next.smode;
            next.smode = MODE_NONE;
            next.smode_turnoff = 1;
        }
    }

    if input.decoded_ctcss > CTCSS_NONE && (next.smode == MODE_NONE || next.smode == MODE_CTCSS) {
        if next.smode != MODE_CTCSS {
            next.smode = MODE_CTCSS;
            next.smode_was = MODE_CTCSS;
        }
        next.smode_timer_ms = config.hold_ms;
    }

    if next.smode == MODE_CTCSS && config.ctcss_tx_enabled != 0 {
        if input.decoded_ctcss != next.last_rx_ctcss {
            next.last_rx_ctcss = input.decoded_ctcss;
            let selected_frequency = if input.decoded_ctcss > CTCSS_NONE {
                config.mapped_tx_ctcss_frequency_tenths_hz[input.decoded_ctcss as usize]
            } else {
                config.default_tx_ctcss_frequency_tenths_hz
            };
            if selected_frequency != 0 && next.tx_ctcss_frequency_tenths_hz != selected_frequency {
                next.tx_ctcss_frequency_tenths_hz = selected_frequency;
                next.tx_ctcss_option = 1;
            }
        }
    } else {
        next.last_rx_ctcss = CTCSS_NONE;
    }

    if input.dcs_valid != 0 && (next.smode == MODE_NONE || next.smode == MODE_DCS) {
        next.smode = MODE_DCS;
        next.smode_was = MODE_DCS;
        next.smode_timer_ms = config.hold_ms;
    }

    *state = next;
    Ok(())
}

/// C ABI entry for the narrow legacy signaling-mode resolver.
///
/// All input is copied before the caller-owned state is changed.  An invalid
/// config or decoder snapshot therefore leaves state untouched, allowing a
/// compatibility adapter to use its established C fallback safely.
pub(crate) extern "C" fn radio_signal_mode_advance(
    config: *const SignalModeConfig,
    input: *const SignalModeInput,
    state: *mut SignalModeState,
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
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{
        CTCSS_NONE, MODE_CTCSS, MODE_DCS, MODE_NONE, SignalModeConfig, SignalModeInput,
        SignalModeState, TONE_COUNT, radio_signal_mode_advance,
    };
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    fn config() -> SignalModeConfig {
        let mut config = SignalModeConfig::disabled();

        config.hold_ms = 25;
        config.ctcss_tx_enabled = 1;
        config.default_tx_ctcss_frequency_tenths_hz = -10;
        config.mapped_tx_ctcss_frequency_tenths_hz[4] = 1_000;
        config
    }

    fn input(
        decoded_ctcss: i32,
        dcs_valid: u32,
        tx_ptt_in: u32,
        elapsed_ms: i32,
    ) -> SignalModeInput {
        SignalModeInput {
            elapsed_ms,
            decoded_ctcss,
            dcs_valid,
            tx_ptt_in,
        }
    }

    #[test]
    fn ctcss_acquire_refresh_expiry_and_default_selection_match_legacy_order() {
        let config = config();
        let mut state = SignalModeState::default();

        assert_eq!(
            radio_signal_mode_advance(&config, &input(4, 0, 0, 0), &mut state),
            RADIO_OK
        );
        assert_eq!(state.smode, MODE_CTCSS);
        assert_eq!(state.smode_was, MODE_CTCSS);
        assert_eq!(state.smode_timer_ms, 25);
        assert_eq!(state.last_rx_ctcss, 4);
        assert_eq!(state.tx_ctcss_frequency_tenths_hz, 1_000);
        assert_eq!(state.tx_ctcss_option, 1);

        state.tx_ctcss_option = 0;
        assert_eq!(
            radio_signal_mode_advance(&config, &input(4, 0, 0, 10), &mut state),
            RADIO_OK
        );
        assert_eq!(state.smode_timer_ms, 25);
        assert_eq!(state.tx_ctcss_option, 0);

        assert_eq!(
            radio_signal_mode_advance(&config, &input(CTCSS_NONE, 0, 0, 25), &mut state),
            RADIO_OK
        );
        assert_eq!(state.smode, MODE_NONE);
        assert_eq!(state.smode_was, MODE_CTCSS);
        assert_eq!(state.smode_turnoff, 1);
        assert_eq!(state.last_rx_ctcss, CTCSS_NONE);

        state.smode = MODE_CTCSS;
        state.last_rx_ctcss = 4;
        assert_eq!(
            radio_signal_mode_advance(&config, &input(CTCSS_NONE, 0, 0, 0), &mut state),
            RADIO_OK
        );
        assert_eq!(state.tx_ctcss_frequency_tenths_hz, -10);
        assert_eq!(state.tx_ctcss_option, 1);
    }

    #[test]
    fn receive_only_mapping_does_not_request_a_transmit_tone() {
        let mut config = config();
        let mut state = SignalModeState::default();

        config.mapped_tx_ctcss_frequency_tenths_hz[4] = 0;
        state.tx_ctcss_frequency_tenths_hz = 777;
        state.tx_ctcss_option = 2;
        assert_eq!(
            radio_signal_mode_advance(&config, &input(4, 0, 0, 0), &mut state),
            RADIO_OK
        );
        assert_eq!(state.smode, MODE_CTCSS);
        assert_eq!(state.last_rx_ctcss, 4);
        assert_eq!(state.tx_ctcss_frequency_tenths_hz, 777);
        assert_eq!(state.tx_ctcss_option, 2);
    }

    #[test]
    fn dcs_only_claims_an_idle_or_dcs_mode_after_ctcss_considers_the_same_snapshot() {
        let config = config();
        let mut state = SignalModeState::default();

        assert_eq!(
            radio_signal_mode_advance(&config, &input(4, 1, 0, 0), &mut state),
            RADIO_OK
        );
        assert_eq!(state.smode, MODE_CTCSS);

        state = SignalModeState::default();
        assert_eq!(
            radio_signal_mode_advance(&config, &input(CTCSS_NONE, 1, 0, 0), &mut state),
            RADIO_OK
        );
        assert_eq!(state.smode, MODE_DCS);
        assert_eq!(state.smode_was, MODE_DCS);
        assert_eq!(state.smode_timer_ms, 25);
    }

    #[test]
    fn ptt_freezes_the_mode_timer_without_clearing_a_sticky_turnoff() {
        let config = config();
        let mut state = SignalModeState {
            smode: MODE_CTCSS,
            smode_was: MODE_CTCSS,
            smode_timer_ms: 7,
            smode_turnoff: 1,
            ..SignalModeState::default()
        };

        assert_eq!(
            radio_signal_mode_advance(&config, &input(CTCSS_NONE, 0, 1, 100), &mut state),
            RADIO_OK
        );
        assert_eq!(state.smode_timer_ms, 7);
        assert_eq!(state.smode_turnoff, 1);
    }

    #[test]
    fn partitioned_elapsed_time_reaches_the_same_mode_state() {
        let config = config();
        let mut whole = SignalModeState {
            smode: MODE_CTCSS,
            smode_was: MODE_CTCSS,
            smode_timer_ms: 20,
            ..SignalModeState::default()
        };
        let mut partitioned = whole;

        assert_eq!(
            radio_signal_mode_advance(&config, &input(CTCSS_NONE, 0, 0, 20), &mut whole),
            RADIO_OK
        );
        for elapsed_ms in [1, 7, 12] {
            assert_eq!(
                radio_signal_mode_advance(
                    &config,
                    &input(CTCSS_NONE, 0, 0, elapsed_ms),
                    &mut partitioned,
                ),
                RADIO_OK
            );
        }
        assert_eq!(partitioned.smode, whole.smode);
        assert_eq!(partitioned.smode_was, whole.smode_was);
        assert_eq!(partitioned.smode_timer_ms, whole.smode_timer_ms);
        assert_eq!(partitioned.smode_turnoff, whole.smode_turnoff);
    }

    #[test]
    fn invalid_configuration_or_snapshot_keeps_caller_state_unchanged() {
        let mut invalid_config = config();
        let valid_input = input(4, 0, 0, 0);
        let initial = SignalModeState {
            smode: MODE_DCS,
            smode_was: MODE_DCS,
            smode_timer_ms: 19,
            last_rx_ctcss: 3,
            tx_ctcss_frequency_tenths_hz: 670,
            tx_ctcss_option: 2,
            smode_turnoff: 1,
        };
        let mut state = initial;

        invalid_config.ctcss_tx_enabled = 2;
        assert_eq!(
            radio_signal_mode_advance(&invalid_config, &valid_input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state.smode, initial.smode);
        assert_eq!(state.smode_was, initial.smode_was);
        assert_eq!(state.smode_timer_ms, initial.smode_timer_ms);
        assert_eq!(state.last_rx_ctcss, initial.last_rx_ctcss);
        assert_eq!(
            state.tx_ctcss_frequency_tenths_hz,
            initial.tx_ctcss_frequency_tenths_hz
        );
        assert_eq!(state.tx_ctcss_option, initial.tx_ctcss_option);
        assert_eq!(state.smode_turnoff, initial.smode_turnoff);

        invalid_config = config();
        assert_eq!(
            radio_signal_mode_advance(
                &invalid_config,
                &input(TONE_COUNT as i32, 0, 0, 0),
                &mut state,
            ),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state.smode, initial.smode);
        assert_eq!(state.smode_timer_ms, initial.smode_timer_ms);
    }

    #[test]
    fn ffi_rejects_null_and_each_invalid_input_without_committing_state() {
        let mut candidate = config();
        let valid_input = input(4, 0, 0, 0);
        let initial = SignalModeState {
            smode: MODE_DCS,
            smode_was: MODE_DCS,
            smode_timer_ms: 19,
            last_rx_ctcss: 3,
            tx_ctcss_frequency_tenths_hz: 670,
            tx_ctcss_option: 2,
            smode_turnoff: 1,
        };
        let mut state = initial;

        assert_eq!(
            radio_signal_mode_advance(std::ptr::null(), &valid_input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_signal_mode_advance(&candidate, std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_signal_mode_advance(&candidate, &valid_input, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);

        candidate.struct_size = 0;
        assert_eq!(
            radio_signal_mode_advance(&candidate, &valid_input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);

        candidate = config();
        candidate.mapped_tx_ctcss_frequency_tenths_hz[0] = -1;
        assert_eq!(
            radio_signal_mode_advance(&candidate, &valid_input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, initial);

        for invalid_input in [
            input(4, 2, 0, 0),
            input(4, 0, 2, 0),
            input(CTCSS_NONE - 1, 0, 0, 0),
            input(TONE_COUNT as i32, 0, 0, 0),
        ] {
            assert_eq!(
                radio_signal_mode_advance(&config(), &invalid_input, &mut state),
                RADIO_INVALID_ARGUMENT
            );
            assert_eq!(state, initial);
        }
    }

    #[test]
    fn resolver_preserves_nonparticipating_legacy_modes() {
        let config = config();

        for mode in [1, 4, 5, 8] {
            let mut state = SignalModeState {
                smode: mode,
                smode_was: mode,
                smode_timer_ms: 25,
                last_rx_ctcss: 4,
                ..SignalModeState::default()
            };

            assert_eq!(
                radio_signal_mode_advance(&config, &input(4, 1, 0, 0), &mut state),
                RADIO_OK
            );
            assert_eq!(state.smode, mode);
            assert_eq!(state.smode_was, mode);
            assert_eq!(state.smode_timer_ms, 25);
            assert_eq!(state.last_rx_ctcss, CTCSS_NONE);
        }
    }
}
