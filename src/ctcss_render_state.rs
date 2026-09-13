//! Legacy-compatible CTCSS render-state transitions.
//!
//! This narrow operation consumes only a request that the compatibility
//! transmitter state machine has already selected.  It does not select a
//! CTCSS frequency, key PTT, generate PCM, or inspect DCS state.  Keeping the
//! boundary this small lets an older compatibility adapter retain its exact C
//! fallback when a descriptor is unavailable or rejects a snapshot.

use std::ffi::c_int;
use std::mem::size_of;
use std::ptr::NonNull;

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK, timer};

/// Keep the current CTCSS render state unchanged apart from the phase reset.
pub const OPTION_HOLD: u32 = 0;
/// Begin rendering the selected CTCSS tone.
pub const OPTION_START: u32 = 1;
/// Begin the configured CTCSS turn-off sequence.
pub const OPTION_TURNOFF: u32 = 2;
/// Disable CTCSS rendering.
pub const OPTION_DISABLE: u32 = 3;

/// CTCSS oscillator is disabled.
pub const STATE_DISABLED: u32 = 0;
/// CTCSS oscillator emits its selected normal tone.
pub const STATE_ACTIVE: u32 = 1;
/// CTCSS oscillator emits a configured turn-off sequence.
pub const STATE_TURNOFF: u32 = 2;

/// Immutable C-compatible CTCSS turn-off configuration.
///
/// The compatibility adapter snapshots the live legacy values after its
/// transmitter state machine has selected phase-shift or tail-tone behavior.
/// This operation neither normalizes nor retains the supplied values.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CtcssRenderStateConfig {
    /// Size supplied by the caller for append-only ABI validation.
    pub struct_size: u32,
    /// Duration of one CTCSS turn-off sequence in milliseconds.
    pub turnoff_duration_ms: i32,
    /// Phase adjustment supplied to the first turn-off PCM callback.
    pub turnoff_phase_shift_degrees: f64,
    /// Replacement tone supplied during a tail-tone turn-off sequence.
    pub turnoff_tail_tone_hz: f64,
}

impl CtcssRenderStateConfig {
    /// Return a valid disabled configuration for a caller that has no TOC.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            turnoff_duration_ms: 0,
            turnoff_phase_shift_degrees: 0.0,
            turnoff_tail_tone_hz: 0.0,
        }
    }
}

/// Elapsed native PCM duration supplied by the compatibility adapter.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CtcssRenderStateInput {
    /// Whole elapsed milliseconds for this native callback span.
    pub elapsed_ms: i32,
}

/// Caller-owned C-compatible CTCSS render state.
///
/// This contains only the legacy fields touched by the final CTCSS
/// render-control branch.  Tone selection, PTT, DCS state, and waveform phase
/// accumulation deliberately remain outside this operation.  A failing call
/// never modifies this structure.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CtcssRenderState {
    /// One of @ref OPTION_HOLD through @ref OPTION_DISABLE.
    pub option: u32,
    /// One of @ref STATE_DISABLED through @ref STATE_TURNOFF.
    pub oscillator_state: u32,
    /// One while the renderer should emit the selected CTCSS source.
    pub enabled: u32,
    /// Milliseconds remaining in an active turn-off sequence.
    pub turnoff_remaining_ms: i32,
    /// Phase adjustment for this renderer callback in degrees.
    pub phase_shift_degrees: f64,
    /// Replacement tail-tone frequency for this renderer callback in hertz.
    pub tail_tone_hz: f64,
}

impl Default for CtcssRenderState {
    fn default() -> Self {
        Self {
            option: OPTION_HOLD,
            oscillator_state: STATE_DISABLED,
            enabled: 0,
            turnoff_remaining_ms: 0,
            phase_shift_degrees: 0.0,
            tail_tone_hz: 0.0,
        }
    }
}

/// Validate immutable CTCSS renderer policy before a stream starts.
///
/// This remains crate-private because the public C ABI validates the same
/// copied structure at its descriptor boundary.
pub(crate) fn validate_config(config: &CtcssRenderStateConfig) -> Result<(), c_int> {
    if config.struct_size < size_of::<CtcssRenderStateConfig>() as u32
        || config.turnoff_duration_ms < 0
        || !config.turnoff_phase_shift_degrees.is_finite()
        || !config.turnoff_tail_tone_hz.is_finite()
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

fn validate_input(input: &CtcssRenderStateInput) -> Result<(), c_int> {
    if input.elapsed_ms < 0 {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

fn validate_state(state: &CtcssRenderState) -> Result<(), c_int> {
    if state.option > OPTION_DISABLE
        || state.oscillator_state > STATE_TURNOFF
        || state.enabled > 1
        || state.turnoff_remaining_ms < 0
        || !state.phase_shift_degrees.is_finite()
        || !state.tail_tone_hz.is_finite()
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(())
}

/// Advance one legacy-compatible CTCSS render-state transition.
///
/// The historical branch clears `phase_shift_degrees` on every native callback
/// before consuming a one-shot option.  A turn-off option consumes the current
/// callback's elapsed time at creation, whereas a subsequent active turn-off
/// consumes that time from its remaining timer and requests disable only for
/// the next callback.  This preserves the CTCSS oscillator's existing
/// phase/tail timing across arbitrary callback partitions.
pub(crate) fn advance(
    config: &CtcssRenderStateConfig,
    input: &CtcssRenderStateInput,
    state: &mut CtcssRenderState,
) -> Result<(), c_int> {
    validate_config(config)?;
    validate_input(input)?;
    validate_state(state)?;

    let mut next = *state;

    next.phase_shift_degrees = 0.0;
    match next.option {
        OPTION_HOLD => {
            if next.oscillator_state == STATE_TURNOFF {
                let _ = timer::consume(&mut next.turnoff_remaining_ms, input.elapsed_ms);
                if next.turnoff_remaining_ms == 0 {
                    next.option = OPTION_DISABLE;
                }
            }
        }
        OPTION_START => {
            next.option = OPTION_HOLD;
            next.oscillator_state = STATE_ACTIVE;
            next.tail_tone_hz = 0.0;
        }
        OPTION_TURNOFF => {
            next.option = OPTION_HOLD;
            next.oscillator_state = STATE_TURNOFF;
            next.turnoff_remaining_ms = config.turnoff_duration_ms.saturating_sub(input.elapsed_ms);
            next.phase_shift_degrees = config.turnoff_phase_shift_degrees;
            next.tail_tone_hz = config.turnoff_tail_tone_hz;
        }
        OPTION_DISABLE => {
            next.option = OPTION_HOLD;
            next.oscillator_state = STATE_DISABLED;
            next.enabled = 0;
            next.tail_tone_hz = 0.0;
        }
        _ => return Err(RADIO_INVALID_ARGUMENT),
    }

    *state = next;
    Ok(())
}

/// C ABI entry for the final legacy CTCSS render-control transition.
///
/// All state is copied before it is advanced.  Invalid arguments therefore
/// leave caller storage untouched so a compatibility adapter can execute its
/// established C branch transactionally.
pub(crate) extern "C" fn radio_ctcss_render_state_advance(
    config: *const CtcssRenderStateConfig,
    input: *const CtcssRenderStateInput,
    state: *mut CtcssRenderState,
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
        CtcssRenderState, CtcssRenderStateConfig, CtcssRenderStateInput, OPTION_DISABLE,
        OPTION_HOLD, OPTION_START, OPTION_TURNOFF, STATE_ACTIVE, STATE_DISABLED, STATE_TURNOFF,
        radio_ctcss_render_state_advance,
    };
    use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

    fn config() -> CtcssRenderStateConfig {
        CtcssRenderStateConfig {
            turnoff_duration_ms: 180,
            turnoff_phase_shift_degrees: 120.0,
            turnoff_tail_tone_hz: 55.0,
            ..CtcssRenderStateConfig::disabled()
        }
    }

    fn input(elapsed_ms: i32) -> CtcssRenderStateInput {
        CtcssRenderStateInput { elapsed_ms }
    }

    #[test]
    fn start_request_changes_only_oscillator_render_state() {
        let mut state = CtcssRenderState {
            option: OPTION_START,
            oscillator_state: STATE_DISABLED,
            enabled: 1,
            turnoff_remaining_ms: 42,
            phase_shift_degrees: 4.0,
            tail_tone_hz: 55.0,
        };

        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut state),
            RADIO_OK
        );
        assert_eq!(state.option, OPTION_HOLD);
        assert_eq!(state.oscillator_state, STATE_ACTIVE);
        assert_eq!(state.enabled, 1);
        assert_eq!(state.turnoff_remaining_ms, 42);
        assert_eq!(state.phase_shift_degrees, 0.0);
        assert_eq!(state.tail_tone_hz, 0.0);
    }

    #[test]
    fn turnoff_request_applies_phase_and_tail_for_its_first_callback() {
        let mut state = CtcssRenderState {
            option: OPTION_TURNOFF,
            oscillator_state: STATE_ACTIVE,
            enabled: 1,
            ..CtcssRenderState::default()
        };

        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut state),
            RADIO_OK
        );
        assert_eq!(state.option, OPTION_HOLD);
        assert_eq!(state.oscillator_state, STATE_TURNOFF);
        assert_eq!(state.enabled, 1);
        assert_eq!(state.turnoff_remaining_ms, 160);
        assert_eq!(state.phase_shift_degrees, 120.0);
        assert_eq!(state.tail_tone_hz, 55.0);

        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut state),
            RADIO_OK
        );
        assert_eq!(state.turnoff_remaining_ms, 140);
        assert_eq!(state.phase_shift_degrees, 0.0);
        assert_eq!(state.tail_tone_hz, 55.0);
    }

    #[test]
    fn completed_turnoff_defers_disable_until_the_next_callback() {
        let mut state = CtcssRenderState {
            oscillator_state: STATE_TURNOFF,
            enabled: 1,
            turnoff_remaining_ms: 20,
            tail_tone_hz: 55.0,
            ..CtcssRenderState::default()
        };

        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut state),
            RADIO_OK
        );
        assert_eq!(state.option, OPTION_DISABLE);
        assert_eq!(state.oscillator_state, STATE_TURNOFF);
        assert_eq!(state.enabled, 1);
        assert_eq!(state.tail_tone_hz, 55.0);

        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut state),
            RADIO_OK
        );
        assert_eq!(state.option, OPTION_HOLD);
        assert_eq!(state.oscillator_state, STATE_DISABLED);
        assert_eq!(state.enabled, 0);
        assert_eq!(state.tail_tone_hz, 0.0);
    }

    #[test]
    fn split_callbacks_reach_the_same_turnoff_state() {
        let mut whole = CtcssRenderState {
            option: OPTION_TURNOFF,
            oscillator_state: STATE_ACTIVE,
            enabled: 1,
            ..CtcssRenderState::default()
        };
        let mut split = whole;

        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(40), &mut whole),
            RADIO_OK
        );
        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut split),
            RADIO_OK
        );
        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut split),
            RADIO_OK
        );
        assert_eq!(split.option, whole.option);
        assert_eq!(split.oscillator_state, whole.oscillator_state);
        assert_eq!(split.enabled, whole.enabled);
        assert_eq!(split.turnoff_remaining_ms, whole.turnoff_remaining_ms);
        assert_eq!(split.tail_tone_hz, whole.tail_tone_hz);
        assert_eq!(whole.phase_shift_degrees, 120.0);
        assert_eq!(split.phase_shift_degrees, 0.0);
    }

    #[test]
    fn disable_request_clears_only_legacy_output_control_fields() {
        let mut state = CtcssRenderState {
            option: OPTION_DISABLE,
            oscillator_state: STATE_TURNOFF,
            enabled: 1,
            turnoff_remaining_ms: 13,
            phase_shift_degrees: 120.0,
            tail_tone_hz: 55.0,
        };

        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut state),
            RADIO_OK
        );
        assert_eq!(state.option, OPTION_HOLD);
        assert_eq!(state.oscillator_state, STATE_DISABLED);
        assert_eq!(state.enabled, 0);
        assert_eq!(state.turnoff_remaining_ms, 13);
        assert_eq!(state.phase_shift_degrees, 0.0);
        assert_eq!(state.tail_tone_hz, 0.0);
    }

    #[test]
    fn invalid_inputs_leave_state_unchanged() {
        let baseline = CtcssRenderState {
            option: OPTION_TURNOFF,
            oscillator_state: STATE_ACTIVE,
            enabled: 1,
            turnoff_remaining_ms: 160,
            phase_shift_degrees: 4.0,
            tail_tone_hz: 55.0,
        };
        let mut state = baseline;
        let mut invalid_config = config();

        invalid_config.turnoff_phase_shift_degrees = f64::NAN;
        assert_eq!(
            radio_ctcss_render_state_advance(&invalid_config, &input(20), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, baseline);
        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(-1), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, baseline);
        state.option = 4;
        assert_eq!(
            radio_ctcss_render_state_advance(&config(), &input(20), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state.option, 4);
    }

    #[test]
    fn ffi_rejects_null_without_committing_state() {
        let config = config();
        let input = input(20);
        let baseline = CtcssRenderState {
            option: OPTION_START,
            oscillator_state: STATE_DISABLED,
            enabled: 1,
            ..CtcssRenderState::default()
        };
        let mut state = baseline;

        assert_eq!(
            radio_ctcss_render_state_advance(std::ptr::null(), &input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_ctcss_render_state_advance(&config, std::ptr::null(), &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(
            radio_ctcss_render_state_advance(&config, &input, std::ptr::null_mut()),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, baseline);
    }
}
