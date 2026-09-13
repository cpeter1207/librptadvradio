//! Owned transmit-signaling state for one native radio stream.
//!
//! This component composes the established signaling primitives into the
//! ordered transmitter half of the native tick.  It selects CTCSS, advances
//! DCS and CTCSS tails, owns logical PTT and finishing timers, and returns
//! renderer intent.  PCM rendering, device I/O, Asterisk, and callback
//! admission remain outside this portable component.

use std::ffi::c_int;
use std::mem::size_of;

use crate::{
    CtcssRenderState, CtcssRenderStateConfig, CtcssRenderStateInput, DcsTurnoffConfig,
    DcsTurnoffInput, DcsTurnoffState, SignalModeConfig, SignalModeInput, SignalModeState,
    TxCompleteConfig, TxCompleteState, TxCpuSaverInput, TxCpuSaverState, TxFinishInput,
    TxFinishState, ctcss_render_state, dcs_turnoff, signal_mode, timer, tx_complete, tx_cpu_saver,
    tx_finish,
};

/// Legacy idle transmitter state.
pub const STATE_IDLE: i32 = 0;
/// Legacy normal-transmit state.
pub const STATE_ACTIVE: i32 = 1;
/// Legacy CTCSS or DCS turn-off state.
pub const STATE_TOC: i32 = 2;
/// Legacy final transmit-buffer drain state.
pub const STATE_FINISHING: i32 = 4;
/// Legacy completed transmitter state before cleanup.
pub const STATE_COMPLETE: i32 = 5;

/// Duration in milliseconds of one historical transmitter buffer.
const LEGACY_BUFFER_MS: i32 = 20;
/// Tail-tone final-drain buffer count.
const TAIL_TONE_BUFFER_CLEAR_FRAMES: i32 = 8;
/// Native frames in one historical 20 ms signaling interval.
const LEGACY_SIGNAL_INTERVAL_FRAMES: u32 = LEGACY_BUFFER_MS as u32 * timer::FRAMES_PER_MILLISECOND;
/// Initial CTCSS frequency retained by the legacy radio constructor.
const INITIAL_CTCSS_FREQUENCY_TENTHS_HZ: i32 = 1_000;
/// Maximum distinct renderer phases crossed by one bounded transmitter tick.
///
/// A constant request can cross CTCSS tail, post-tone gap, finishing, and idle
/// at most once each.  Adjacent equal phases are coalesced, so four spans are
/// sufficient without allocating an event queue.
pub const MAX_RENDER_SPANS: usize = 4;

/// Configured CTCSS turn-off behavior.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ToneOffMode {
    /// Release PTT through the normal final drain without a CTCSS tail.
    #[default]
    None,
    /// Apply the configured CTCSS phase shift before release.
    PhaseShift,
    /// Remove CTCSS for the configured tail duration before release.
    ToneRemove,
    /// Substitute the configured low-frequency tail tone before release.
    TailTone,
}

/// Immutable transmitter policy prepared off the real-time path.
///
/// The signal-mode configuration owns CTCSS mapping and default selection;
/// its `ctcss_tx_enabled` field is also the transmitter CTCSS enable.  All
/// values are copied into [`TransmitSignaling`] and never retain caller data.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Config {
    /// Receive-signaling mode and transmit CTCSS mapping policy.
    pub signal_mode: SignalModeConfig,
    /// Duration of a selected DCS transmitter turn-off tail.
    pub dcs_turnoff: DcsTurnoffConfig,
    /// CTCSS renderer turn-off parameters.
    pub ctcss_render: CtcssRenderStateConfig,
    /// Receiver blanking intent armed after a completed transmission.
    pub tx_complete: TxCompleteConfig,
    /// Configured CTCSS turn-off policy.
    pub tone_off_mode: ToneOffMode,
    /// One while a fixed DCS code is transmitted.
    pub dcs_transmit_enabled: bool,
    /// One while DCS uses its configured turn-off tail.
    pub dcs_turnoff_enabled: bool,
    /// Logical PTT settle time, consumed only after physical PTT is applied.
    pub tx_settle_time_ms: i32,
    /// One while idle transmitter rendering may be halted.
    pub tx_cpu_saver_enabled: bool,
}

impl Config {
    /// Return a valid no-signaling policy suitable for a disabled transmitter.
    #[must_use]
    pub fn disabled() -> Self {
        Self {
            signal_mode: SignalModeConfig::disabled(),
            dcs_turnoff: DcsTurnoffConfig::default(),
            ctcss_render: CtcssRenderStateConfig::disabled(),
            tx_complete: TxCompleteConfig {
                struct_size: size_of::<TxCompleteConfig>() as u32,
                txrx_blanking_time_ms: 0,
            },
            tone_off_mode: ToneOffMode::None,
            dcs_transmit_enabled: false,
            dcs_turnoff_enabled: false,
            tx_settle_time_ms: 0,
            tx_cpu_saver_enabled: false,
        }
    }
}

/// Stable native-tick inputs for one transmitter update.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Input {
    /// Native frames in this bounded callback span.
    pub native_frame_count: u32,
    /// One when a matching native DAC span will be rendered.
    pub tx_render_admitted: bool,
    /// Current logical transmitter request imported from the adapter.
    pub external_ptt_request: bool,
    /// Current physical PTT state published by the hardware adapter.
    pub physical_ptt_applied: bool,
    /// Decoded CTCSS table index, or the no-tone sentinel.
    pub decoded_ctcss: i32,
    /// One while the receive DCS decoder qualifies its configured code.
    pub dcs_valid: bool,
    /// One while transient control inhibits transmit CTCSS selection/tails.
    pub ctcss_inhibit: bool,
}

/// Failure from a malformed signaling snapshot or immutable policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The input cannot describe a native transmitter callback.
    InvalidInput,
    /// A composed primitive rejected policy or persistent state.
    InvalidConfigurationOrState,
}

/// Receiver-blanking intent emitted after a completed transmission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BlankingArm {
    /// Duration the receive path must protect after physical PTT release.
    pub duration_ms: i32,
}

/// One contiguous transmitter-rendering intent inside a native callback.
///
/// `ctcss_render.phase_shift_degrees` is an action at the span's first frame,
/// not a per-sample offset.  The renderer therefore applies it once before
/// generating the span's continuous oscillator samples.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RenderSpan {
    /// First native frame of this span within the enclosing tick.
    pub frame_offset: u32,
    /// Number of native frames represented by this intent.
    pub frame_count: u32,
    /// Logical PTT intent during this span.
    pub logical_ptt: bool,
    /// CTCSS oscillator intent for this span.
    pub ctcss_render: CtcssRenderState,
    /// Selected normal CTCSS frequency for this exact span, in tenths of hertz.
    ///
    /// This belongs beside oscillator intent rather than only in the final
    /// signaling snapshot: a decoded mapping can change within a large native
    /// callback, and the renderer must retain the frequency that applied to
    /// each emitted span.
    pub ctcss_frequency_tenths_hz: i32,
    /// One while the DCS turn-off waveform is selected for this span.
    pub dcs_turnoff_active: bool,
}

/// Fixed-capacity renderer intent for one transmitter tick.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct RenderTimeline {
    spans: [RenderSpan; MAX_RENDER_SPANS],
    count: usize,
}

impl RenderTimeline {
    /// Add one contiguous intent, coalescing a matching preceding span.
    fn append(&mut self, frame_count: u32, intent: RenderSpan) -> Result<(), Error> {
        if frame_count == 0 {
            return Ok(());
        }
        if self.count != 0 {
            let previous = &mut self.spans[self.count - 1];
            if previous.logical_ptt == intent.logical_ptt
                && previous.ctcss_render == intent.ctcss_render
                && previous.ctcss_frequency_tenths_hz == intent.ctcss_frequency_tenths_hz
                && previous.dcs_turnoff_active == intent.dcs_turnoff_active
            {
                previous.frame_count = previous
                    .frame_count
                    .checked_add(frame_count)
                    .ok_or(Error::InvalidConfigurationOrState)?;
                return Ok(());
            }
        }
        if self.count == MAX_RENDER_SPANS {
            return Err(Error::InvalidConfigurationOrState);
        }
        let frame_offset = if self.count == 0 {
            0
        } else {
            let previous = self.spans[self.count - 1];
            previous
                .frame_offset
                .checked_add(previous.frame_count)
                .ok_or(Error::InvalidConfigurationOrState)?
        };
        self.spans[self.count] = RenderSpan {
            frame_offset,
            frame_count,
            ..intent
        };
        self.count += 1;
        Ok(())
    }
}

/// Explicit sample-clocked transmitter turn-off phase.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum TurnoffPhase {
    /// No turn-off phase is active.
    #[default]
    None,
    /// A phase-shifted or low-frequency CTCSS tail is audible.
    CtcssTail,
    /// CTCSS is silent for the retained one-interval post-tone dwell.
    CtcssGap,
    /// The selected DCS turn-off waveform is audible.
    DcsTail,
    /// CTCSS is immediately removed while PTT remains asserted.
    NoTone,
}

/// One-shot control-plane notifications emitted by a transmitter tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct TickActions {
    /// One when the adapter must publish the selected CTCSS frequency status.
    ctcss_status_event: bool,
    /// A new receiver-blanking interval to hand to receive-side policy.
    receiver_blanking_arm: Option<BlankingArm>,
}

/// Complete observable transmitter-signaling snapshot after one tick.
///
/// The renderer consumes `ctcss_render`; a hardware adapter consumes
/// `logical_ptt`.  Blanking is intentionally only armed here: receive-side
/// policy owns and consumes the active interval before frontend work.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    /// Current receive-signaling selection and selected transmit CTCSS tone.
    pub signal_mode: SignalModeState,
    /// Current legacy transmitter state.
    pub transmitter_state: i32,
    /// Desired logical PTT output.
    pub logical_ptt: bool,
    /// One only on a logical PTT assertion in this tick.
    pub logical_ptt_raised: bool,
    /// One only on a logical PTT release in this tick.
    pub logical_ptt_released: bool,
    /// Renderer-facing CTCSS oscillator state and tail intent.
    pub ctcss_render: CtcssRenderState,
    /// Number of valid entries in `render_spans`.
    pub render_span_count: usize,
    /// Bounded sample-offset rendering intents for the enclosing tick.
    pub render_spans: [RenderSpan; MAX_RENDER_SPANS],
    /// Remaining DCS turn-off tail duration.
    pub dcs_turnoff_remaining_ms: i32,
    /// Remaining CTCSS no-tone dwell duration.
    pub tx_hang_remaining_ms: i32,
    /// Remaining final-drain duration.
    pub finish_remaining_ms: i32,
    /// Historical final-drain buffer count.
    pub buffer_clear_frames: i32,
    /// Remaining PTT settle duration.
    pub settle_remaining_ms: i32,
    /// One when the adapter must publish selected CTCSS frequency status.
    pub ctcss_status_event: bool,
    /// Receiver blanking interval armed by this tick, if any.
    pub receiver_blanking_arm: Option<BlankingArm>,
    /// One while idle transmitter rendering is intentionally skipped.
    pub cpu_halted: bool,
    /// Sub-millisecond frames retained by receive signaling-mode timing.
    pub rx_timer_remainder_frames: u32,
    /// Sub-millisecond frames retained by admitted transmitter timing.
    pub tx_timer_remainder_frames: u32,
}

/// Callback-owned transmitter signaling state.
#[derive(Clone, Copy, Debug, PartialEq)]
struct State {
    signal_mode: SignalModeState,
    transmitter_state: i32,
    logical_ptt: bool,
    ctcss_render: CtcssRenderState,
    dcs_turnoff_remaining_ms: i32,
    tx_hang_remaining_ms: i32,
    finish_remaining_ms: i32,
    buffer_clear_frames: i32,
    settle_remaining_ms: i32,
    cpu_halted: bool,
    turnoff_phase: TurnoffPhase,
    /// Remaining frames in the active CTCSS/DCS/no-tone turn-off phase.
    turnoff_remaining_frames: u32,
    /// CTCSS configured tail time retained for legacy visible timer state.
    ctcss_legacy_remaining_frames: u32,
    /// Remaining sample-clocked finishing-drain duration.
    finish_remaining_frames: u32,
    rx_timer_remainder_frames: u32,
    tx_timer_remainder_frames: u32,
}

impl Default for State {
    fn default() -> Self {
        Self {
            signal_mode: SignalModeState {
                tx_ctcss_frequency_tenths_hz: INITIAL_CTCSS_FREQUENCY_TENTHS_HZ,
                ..SignalModeState::default()
            },
            transmitter_state: STATE_IDLE,
            logical_ptt: false,
            ctcss_render: CtcssRenderState::default(),
            dcs_turnoff_remaining_ms: 0,
            tx_hang_remaining_ms: 0,
            finish_remaining_ms: 0,
            buffer_clear_frames: 0,
            settle_remaining_ms: 0,
            cpu_halted: false,
            turnoff_phase: TurnoffPhase::None,
            turnoff_remaining_frames: 0,
            ctcss_legacy_remaining_frames: 0,
            finish_remaining_frames: 0,
            rx_timer_remainder_frames: 0,
            tx_timer_remainder_frames: 0,
        }
    }
}

/// Complete portable owner for one transmitter signaling state machine.
///
/// [`TransmitSignaling::tick`] performs no allocation, lock, I/O, or PCM
/// rendering.  It is deliberately a Rust-only composition surface until the
/// complete M1 engine replaces primitive descriptor calls as one unit.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TransmitSignaling {
    config: Config,
    state: State,
}

impl TransmitSignaling {
    /// Create a reset transmitter state from one immutable copied policy.
    ///
    /// Immutable policy is validated before native processing begins.  This
    /// keeps a valid constructed stream from failing inside its callback due
    /// to configuration that is otherwise unchanged for its lifetime.
    pub fn new(config: Config) -> Result<Self, Error> {
        validate_config(&config)?;
        Ok(Self {
            config,
            state: State::default(),
        })
    }

    /// Return the current state without advancing signaling time.
    #[must_use]
    pub fn snapshot(&self) -> Output {
        output_from_state(
            &self.state,
            false,
            false,
            TickActions::default(),
            RenderTimeline::default(),
        )
    }

    /// Advance signaling for exactly one admitted or held native callback.
    ///
    /// Receive signaling-mode time advances for every native span.  All
    /// transmitter timers and CTCSS rendering advance only when
    /// `tx_render_admitted` is true, matching the C playout-hold boundary.
    /// On failure, the persistent state remains unchanged.
    pub fn tick(&mut self, input: Input) -> Result<Output, Error> {
        if input.native_frame_count == 0
            || input.decoded_ctcss < signal_mode::CTCSS_NONE
            || input.decoded_ctcss >= signal_mode::TONE_COUNT as i32
        {
            return Err(Error::InvalidInput);
        }

        let mut next = self.state;
        let rx_elapsed_ms = timer::elapsed_ms(
            &mut next.rx_timer_remainder_frames,
            input.native_frame_count,
        );
        component(signal_mode::advance(
            &self.config.signal_mode,
            &SignalModeInput {
                elapsed_ms: rx_elapsed_ms,
                decoded_ctcss: input.decoded_ctcss,
                dcs_valid: u32::from(input.dcs_valid),
                tx_ptt_in: u32::from(input.external_ptt_request),
            },
            &mut next.signal_mode,
        ))?;
        // A selected tail owns renderer state until it completes; a later
        // receive-side selection cannot overwrite an audible tail.
        if next.turnoff_phase == TurnoffPhase::None {
            sync_ctcss_option(&mut next);
        }

        if !input.tx_render_admitted {
            self.state = next;
            return Ok(output_from_state(
                &self.state,
                false,
                false,
                TickActions::default(),
                RenderTimeline::default(),
            ));
        }

        let tx_elapsed_ms = timer::elapsed_ms(
            &mut next.tx_timer_remainder_frames,
            input.native_frame_count,
        );
        let previous_ptt = next.logical_ptt;
        let mut actions = TickActions::default();
        prepare_transmitter(&self.config, &input, &mut next, &mut actions)?;
        let mut timeline = RenderTimeline::default();
        advance_render_timeline(&self.config, &input, &mut next, &mut actions, &mut timeline)?;

        if next.settle_remaining_ms != 0 && input.physical_ptt_applied {
            let _ = timer::consume(&mut next.settle_remaining_ms, tx_elapsed_ms);
        }

        let mut saver = TxCpuSaverState {
            halted: u32::from(next.cpu_halted),
        };
        component(tx_cpu_saver::advance(
            &TxCpuSaverInput {
                enabled: u32::from(self.config.tx_cpu_saver_enabled),
                tx_ptt_in: u32::from(input.external_ptt_request),
                tx_ptt_out: u32::from(next.logical_ptt),
                tx_idle: u32::from(next.transmitter_state == STATE_IDLE),
            },
            &mut saver,
        ))?;
        next.cpu_halted = saver.halted != 0;

        // Completion queues the historical disable option.  Match the C
        // ordering by consuming it after CPU-saver selection, while retaining
        // it untouched when an idle saver skips renderer work.
        if !next.cpu_halted && next.ctcss_render.option == ctcss_render_state::OPTION_DISABLE {
            apply_ctcss_option(&self.config, &mut next, ctcss_render_state::OPTION_DISABLE)?;
        }

        let raised = !previous_ptt && next.logical_ptt;
        let released = previous_ptt && !next.logical_ptt;
        self.state = next;
        Ok(output_from_state(
            &self.state,
            raised,
            released,
            actions,
            timeline,
        ))
    }
}

/// Map an internal primitive result to the Rust-only composition error.
fn component(result: Result<(), c_int>) -> Result<(), Error> {
    result.map_err(|_| Error::InvalidConfigurationOrState)
}

/// Validate every immutable policy that a transmitter callback can consume.
///
/// Primitive validators remain their single source of truth.  The conditional
/// DCS validation mirrors the only branch that can consume its duration, and
/// duration conversion catches values that would otherwise overflow only when
/// a later native callback starts the selected tail.
fn validate_config(config: &Config) -> Result<(), Error> {
    component(signal_mode::validate_config(&config.signal_mode))?;
    component(ctcss_render_state::validate_config(&config.ctcss_render))?;
    component(tx_complete::validate_config(&config.tx_complete))?;
    if config.tx_settle_time_ms < 0 {
        return Err(Error::InvalidConfigurationOrState);
    }

    if config.dcs_transmit_enabled && config.dcs_turnoff_enabled {
        component(dcs_turnoff::validate_config(&config.dcs_turnoff))?;
        let _ = duration_frames(config.dcs_turnoff.turnoff_duration_ms)?;
    }
    if config.tone_off_mode != ToneOffMode::None {
        let _ = duration_frames(config.ctcss_render.turnoff_duration_ms)?;
    }
    Ok(())
}

/// Return whether the copied signal-mode policy enables transmit CTCSS.
fn ctcss_transmit_enabled(config: &Config) -> bool {
    config.signal_mode.ctcss_tx_enabled != 0
}

/// Keep the shared historical CTCSS option represented by both primitives coherent.
fn sync_ctcss_option(state: &mut State) {
    state.ctcss_render.option = state.signal_mode.tx_ctcss_option;
}

/// Copy the renderer's consumed option back to the signaling-mode state.
fn sync_signal_mode_option(state: &mut State) {
    state.signal_mode.tx_ctcss_option = state.ctcss_render.option;
}

/// Set the one historical CTCSS option consumed by the renderer.
fn set_ctcss_option(state: &mut State, option: u32) {
    state.signal_mode.tx_ctcss_option = option;
    state.ctcss_render.option = option;
}

/// Select the transmitter CTCSS frequency for a fresh logical PTT assertion.
fn select_start_ctcss(config: &Config, input: &Input, state: &mut State) {
    state.signal_mode.tx_ctcss_frequency_tenths_hz = 0;
    if !ctcss_transmit_enabled(config) || input.ctcss_inhibit {
        return;
    }

    let selected = if state.signal_mode.smode == signal_mode::MODE_CTCSS
        && input.decoded_ctcss != signal_mode::CTCSS_NONE
    {
        config.signal_mode.mapped_tx_ctcss_frequency_tenths_hz[input.decoded_ctcss as usize]
    } else {
        config.signal_mode.default_tx_ctcss_frequency_tenths_hz
    };
    if selected != 0 {
        state.signal_mode.tx_ctcss_frequency_tenths_hz = selected;
        set_ctcss_option(state, ctcss_render_state::OPTION_START);
        state.ctcss_render.enabled = 1;
        state.ctcss_render.turnoff_remaining_ms = 0;
    }
}

/// Select the CTCSS renderer values appropriate to the configured tail mode.
fn ctcss_render_config(config: &Config) -> CtcssRenderStateConfig {
    let mut render = config.ctcss_render;
    match config.tone_off_mode {
        ToneOffMode::TailTone => render.turnoff_phase_shift_degrees = 0.0,
        ToneOffMode::PhaseShift => render.turnoff_tail_tone_hz = 0.0,
        ToneOffMode::None | ToneOffMode::ToneRemove => {}
    }
    render
}

/// Perform the scalar cleanup that follows a completed transmitter drain.
fn complete_transmission(config: &Config, state: &mut State) -> Result<TickActions, Error> {
    let mut complete = TxCompleteState {
        tx_state: state.transmitter_state,
        tx_ptt_out: u32::from(state.logical_ptt),
        tx_ctcss_option: state.ctcss_render.option,
        txrx_blanking_timer_ms: 0,
        txrx_blanking_sample_remainder: 0,
        tx_ctcss_ready: 0,
    };
    component(tx_complete::complete(&config.tx_complete, &mut complete))?;
    state.transmitter_state = complete.tx_state;
    state.logical_ptt = complete.tx_ptt_out != 0;
    set_ctcss_option(state, complete.tx_ctcss_option);
    Ok(TickActions {
        ctcss_status_event: complete.tx_ctcss_ready != 0,
        receiver_blanking_arm: Some(BlankingArm {
            duration_ms: complete.txrx_blanking_timer_ms,
        }),
    })
}

/// Convert a nonnegative millisecond duration to exact native frames.
fn duration_frames(milliseconds: i32) -> Result<u32, Error> {
    if milliseconds < 0 {
        return Err(Error::InvalidConfigurationOrState);
    }
    u32::try_from(
        u64::try_from(milliseconds)
            .map_err(|_| Error::InvalidConfigurationOrState)?
            .checked_mul(u64::from(timer::FRAMES_PER_MILLISECOND))
            .ok_or(Error::InvalidConfigurationOrState)?,
    )
    .map_err(|_| Error::InvalidConfigurationOrState)
}

/// Report an exact native-frame countdown through the retained millisecond API.
fn remaining_milliseconds(frames: u32) -> i32 {
    let milliseconds = u64::from(frames).div_ceil(u64::from(timer::FRAMES_PER_MILLISECOND));
    i32::try_from(milliseconds).expect("native-frame countdown fits in i32 milliseconds")
}

/// Round a CTCSS tail up to the historical 20 ms audible tail interval.
fn ctcss_tail_frames(milliseconds: i32) -> Result<u32, Error> {
    let requested = duration_frames(milliseconds)?;
    let intervals = requested.div_ceil(LEGACY_SIGNAL_INTERVAL_FRAMES).max(1);
    intervals
        .checked_mul(LEGACY_SIGNAL_INTERVAL_FRAMES)
        .ok_or(Error::InvalidConfigurationOrState)
}

/// Convert no-tone dwell directly to its exact configured sample duration.
///
/// Unlike the explicit CTCSS phase/tail sequence, no-tone has no independent
/// legacy waveform interval.  It therefore expires at its configured sample
/// deadline rather than at an implementation callback boundary.
fn no_tone_output_frames(milliseconds: i32) -> Result<u32, Error> {
    duration_frames(milliseconds)
}

/// Consume no more than one sample-clocked phase's remaining duration.
fn consume_phase(remaining: &mut u32, requested: u32) -> u32 {
    let consumed = (*remaining).min(requested);
    *remaining -= consumed;
    consumed
}

/// Consume one renderer option immediately before emitting its PCM intent.
fn apply_ctcss_option(config: &Config, state: &mut State, option: u32) -> Result<(), Error> {
    if option == ctcss_render_state::OPTION_START {
        state.ctcss_render.enabled = 1;
    }
    set_ctcss_option(state, option);
    component(ctcss_render_state::advance(
        &ctcss_render_config(config),
        &CtcssRenderStateInput { elapsed_ms: 0 },
        &mut state.ctcss_render,
    ))?;
    sync_signal_mode_option(state);
    Ok(())
}

/// Consume a pending signal-mode CTCSS request before renderer intent is emitted.
fn apply_pending_ctcss_option(config: &Config, state: &mut State) -> Result<(), Error> {
    if state.ctcss_render.option == ctcss_render_state::OPTION_HOLD {
        return Ok(());
    }
    apply_ctcss_option(config, state, state.ctcss_render.option)
}

/// Start the selected CTCSS phase-shift or 55 Hz tail at the current sample.
fn start_ctcss_tail(config: &Config, state: &mut State) -> Result<(), Error> {
    state.transmitter_state = STATE_TOC;
    state.turnoff_phase = TurnoffPhase::CtcssTail;
    state.turnoff_remaining_frames = ctcss_tail_frames(config.ctcss_render.turnoff_duration_ms)?;
    state.ctcss_legacy_remaining_frames = duration_frames(config.ctcss_render.turnoff_duration_ms)?;
    state.tx_hang_remaining_ms = 0;
    apply_ctcss_option(config, state, ctcss_render_state::OPTION_TURNOFF)
}

/// Start the compatibility no-tone dwell with CTCSS muted immediately.
fn start_no_tone(config: &Config, state: &mut State) -> Result<(), Error> {
    state.transmitter_state = STATE_TOC;
    state.turnoff_phase = TurnoffPhase::NoTone;
    state.turnoff_remaining_frames =
        no_tone_output_frames(config.ctcss_render.turnoff_duration_ms)?;
    state.tx_hang_remaining_ms = remaining_milliseconds(state.turnoff_remaining_frames);
    apply_ctcss_option(config, state, ctcss_render_state::OPTION_DISABLE)
}

/// Start an exact DCS tail while retaining normal CTCSS render intent.
fn start_dcs_tail(config: &Config, state: &mut State) -> Result<(), Error> {
    let mut dcs = DcsTurnoffState {
        tx_state: state.transmitter_state,
        dcs_turnoff_remaining_ms: state.dcs_turnoff_remaining_ms,
        tx_hang_remaining_ms: state.tx_hang_remaining_ms,
        finish_requested: 0,
        finish_elapsed_ms: 0,
    };
    component(dcs_turnoff::advance(
        &config.dcs_turnoff,
        &DcsTurnoffInput {
            elapsed_ms: 0,
            tx_ptt_in: 0,
            begin_turnoff: 1,
        },
        &mut dcs,
    ))?;
    state.transmitter_state = dcs.tx_state;
    state.dcs_turnoff_remaining_ms = dcs.dcs_turnoff_remaining_ms;
    state.tx_hang_remaining_ms = dcs.tx_hang_remaining_ms;
    state.turnoff_phase = TurnoffPhase::DcsTail;
    state.turnoff_remaining_frames = duration_frames(config.dcs_turnoff.turnoff_duration_ms)?;
    Ok(())
}

/// Rekey an active DCS tail through its retained primitive transition.
fn rekey_dcs_tail(config: &Config, state: &mut State) -> Result<(), Error> {
    let mut dcs = DcsTurnoffState {
        tx_state: state.transmitter_state,
        dcs_turnoff_remaining_ms: state.dcs_turnoff_remaining_ms,
        tx_hang_remaining_ms: state.tx_hang_remaining_ms,
        finish_requested: 0,
        finish_elapsed_ms: 0,
    };
    component(dcs_turnoff::advance(
        &config.dcs_turnoff,
        &DcsTurnoffInput {
            elapsed_ms: 0,
            tx_ptt_in: 1,
            begin_turnoff: 0,
        },
        &mut dcs,
    ))?;
    state.transmitter_state = dcs.tx_state;
    state.dcs_turnoff_remaining_ms = dcs.dcs_turnoff_remaining_ms;
    state.tx_hang_remaining_ms = dcs.tx_hang_remaining_ms;
    state.turnoff_phase = TurnoffPhase::None;
    state.turnoff_remaining_frames = 0;
    state.ctcss_legacy_remaining_frames = 0;
    Ok(())
}

/// Enter a sample-clocked normal or 55 Hz finishing drain.
fn start_finishing(state: &mut State, tail_tone: bool) -> Result<(), Error> {
    let mut finish = TxFinishState {
        buffer_clear_frames: state.buffer_clear_frames,
        finish_remaining_ms: state.finish_remaining_ms,
        tx_state: state.transmitter_state,
    };
    component(tx_finish::advance(
        &TxFinishInput { elapsed_ms: 0 },
        &mut finish,
    ))?;
    state.buffer_clear_frames = if tail_tone {
        TAIL_TONE_BUFFER_CLEAR_FRAMES
    } else {
        finish.buffer_clear_frames
    };
    state.finish_remaining_ms = (state.buffer_clear_frames + 1) * LEGACY_BUFFER_MS;
    state.finish_remaining_frames = duration_frames(state.finish_remaining_ms)?;
    state.transmitter_state = STATE_FINISHING;
    state.turnoff_phase = TurnoffPhase::None;
    state.turnoff_remaining_frames = 0;
    state.ctcss_legacy_remaining_frames = 0;
    Ok(())
}

/// Return the current renderer intent for one contiguous sample span.
fn current_render_span(state: &State) -> RenderSpan {
    RenderSpan {
        frame_offset: 0,
        frame_count: 0,
        logical_ptt: state.logical_ptt,
        ctcss_render: state.ctcss_render,
        ctcss_frequency_tenths_hz: state.signal_mode.tx_ctcss_frequency_tenths_hz,
        dcs_turnoff_active: state.turnoff_phase == TurnoffPhase::DcsTail,
    }
}

/// Append the current renderer intent and consume a one-shot phase action.
fn append_current_span(
    timeline: &mut RenderTimeline,
    state: &mut State,
    frame_count: u32,
) -> Result<(), Error> {
    timeline.append(frame_count, current_render_span(state))?;
    if frame_count != 0 {
        state.ctcss_render.phase_shift_degrees = 0.0;
    }
    Ok(())
}

/// Start or rekey transmitter state before consuming the current sample span.
fn prepare_transmitter(
    config: &Config,
    input: &Input,
    state: &mut State,
    actions: &mut TickActions,
) -> Result<(), Error> {
    if input.external_ptt_request && state.transmitter_state == STATE_IDLE {
        select_start_ctcss(config, input, state);
        apply_pending_ctcss_option(config, state)?;
        if config.dcs_transmit_enabled {
            state.dcs_turnoff_remaining_ms = 0;
        }
        state.turnoff_phase = TurnoffPhase::None;
        state.turnoff_remaining_frames = 0;
        state.ctcss_legacy_remaining_frames = 0;
        state.finish_remaining_frames = 0;
        state.tx_hang_remaining_ms = 0;
        actions.ctcss_status_event = true;
        state.transmitter_state = STATE_ACTIVE;
        state.logical_ptt = true;
        // The retained C path refreshed this hold only on a later ACTIVE
        // callback.  Initialize it at the PTT edge so one 960-frame callback
        // and any partition of that same PCM time have identical signaling
        // state, without changing PTT or renderer intent.
        state.signal_mode.smode_timer_ms = config.signal_mode.hold_ms;
        state.settle_remaining_ms = config.tx_settle_time_ms;
    } else if input.external_ptt_request && state.transmitter_state == STATE_ACTIVE {
        state.signal_mode.smode_timer_ms = config.signal_mode.hold_ms;
        apply_pending_ctcss_option(config, state)?;
    } else if !input.external_ptt_request && state.transmitter_state == STATE_ACTIVE {
        if config.dcs_transmit_enabled && config.dcs_turnoff_enabled {
            start_dcs_tail(config, state)?;
        } else if state.ctcss_render.enabled != 0 && !input.ctcss_inhibit {
            if config.tone_off_mode == ToneOffMode::None || !ctcss_transmit_enabled(config) {
                apply_ctcss_option(config, state, ctcss_render_state::OPTION_DISABLE)?;
                start_finishing(state, false)?;
            } else if config.tone_off_mode == ToneOffMode::ToneRemove {
                start_no_tone(config, state)?;
            } else {
                start_ctcss_tail(config, state)?;
            }
        } else {
            apply_ctcss_option(config, state, ctcss_render_state::OPTION_DISABLE)?;
            start_finishing(state, false)?;
        }
    } else if state.transmitter_state == STATE_TOC {
        if input.external_ptt_request && state.turnoff_phase == TurnoffPhase::DcsTail {
            rekey_dcs_tail(config, state)?;
            state.signal_mode.smode_timer_ms = config.signal_mode.hold_ms;
        } else if input.external_ptt_request && ctcss_transmit_enabled(config) {
            state.transmitter_state = STATE_ACTIVE;
            state.turnoff_phase = TurnoffPhase::None;
            state.turnoff_remaining_frames = 0;
            state.ctcss_legacy_remaining_frames = 0;
            apply_ctcss_option(config, state, ctcss_render_state::OPTION_START)?;
            state.signal_mode.smode_timer_ms = config.signal_mode.hold_ms;
        }
    }
    Ok(())
}

/// Consume transmitter phases by exact native sample counts into renderer spans.
fn advance_render_timeline(
    config: &Config,
    input: &Input,
    state: &mut State,
    actions: &mut TickActions,
    timeline: &mut RenderTimeline,
) -> Result<(), Error> {
    let mut remaining = input.native_frame_count;
    while remaining != 0 {
        match state.transmitter_state {
            STATE_IDLE | STATE_ACTIVE => {
                append_current_span(timeline, state, remaining)?;
                return Ok(());
            }
            STATE_TOC => match state.turnoff_phase {
                TurnoffPhase::CtcssTail => {
                    let consumed = consume_phase(&mut state.turnoff_remaining_frames, remaining);
                    append_current_span(timeline, state, consumed)?;
                    remaining -= consumed;
                    let _ = consume_phase(&mut state.ctcss_legacy_remaining_frames, consumed);
                    state.ctcss_render.turnoff_remaining_ms =
                        remaining_milliseconds(state.ctcss_legacy_remaining_frames);
                    if state.turnoff_remaining_frames == 0 {
                        apply_ctcss_option(config, state, ctcss_render_state::OPTION_DISABLE)?;
                        state.turnoff_phase = TurnoffPhase::CtcssGap;
                        state.turnoff_remaining_frames = LEGACY_SIGNAL_INTERVAL_FRAMES;
                    }
                }
                TurnoffPhase::CtcssGap => {
                    let consumed = consume_phase(&mut state.turnoff_remaining_frames, remaining);
                    append_current_span(timeline, state, consumed)?;
                    remaining -= consumed;
                    if state.turnoff_remaining_frames == 0 {
                        start_finishing(state, config.tone_off_mode == ToneOffMode::TailTone)?;
                    }
                }
                TurnoffPhase::DcsTail => {
                    let consumed = consume_phase(&mut state.turnoff_remaining_frames, remaining);
                    append_current_span(timeline, state, consumed)?;
                    remaining -= consumed;
                    state.dcs_turnoff_remaining_ms =
                        remaining_milliseconds(state.turnoff_remaining_frames);
                    if state.turnoff_remaining_frames == 0 {
                        state.dcs_turnoff_remaining_ms = 0;
                        start_finishing(state, false)?;
                    }
                }
                TurnoffPhase::NoTone => {
                    if state.turnoff_remaining_frames == 0 {
                        state.tx_hang_remaining_ms = 0;
                        state.transmitter_state = STATE_COMPLETE;
                        continue;
                    }
                    let consumed = consume_phase(&mut state.turnoff_remaining_frames, remaining);
                    append_current_span(timeline, state, consumed)?;
                    remaining -= consumed;
                    state.tx_hang_remaining_ms =
                        remaining_milliseconds(state.turnoff_remaining_frames);
                    if state.turnoff_remaining_frames == 0 {
                        state.transmitter_state = STATE_COMPLETE;
                    }
                }
                TurnoffPhase::None => return Err(Error::InvalidConfigurationOrState),
            },
            STATE_FINISHING => {
                if state.finish_remaining_frames == 0 {
                    state.transmitter_state = STATE_COMPLETE;
                    continue;
                }
                let consumed = consume_phase(&mut state.finish_remaining_frames, remaining);
                append_current_span(timeline, state, consumed)?;
                remaining -= consumed;
                state.finish_remaining_ms = remaining_milliseconds(state.finish_remaining_frames);
                if state.finish_remaining_frames == 0 {
                    state.finish_remaining_ms = 0;
                    state.buffer_clear_frames = 0;
                    state.transmitter_state = STATE_COMPLETE;
                }
            }
            STATE_COMPLETE => {
                *actions = complete_transmission(config, state)?;
                state.turnoff_phase = TurnoffPhase::None;
                state.turnoff_remaining_frames = 0;
                state.ctcss_legacy_remaining_frames = 0;
                state.finish_remaining_frames = 0;
            }
            _ => return Err(Error::InvalidConfigurationOrState),
        }
    }
    if state.transmitter_state == STATE_COMPLETE {
        *actions = complete_transmission(config, state)?;
        state.turnoff_phase = TurnoffPhase::None;
    }
    Ok(())
}

/// Build a complete immutable output snapshot from callback-owned state.
fn output_from_state(
    state: &State,
    logical_ptt_raised: bool,
    logical_ptt_released: bool,
    actions: TickActions,
    timeline: RenderTimeline,
) -> Output {
    Output {
        signal_mode: state.signal_mode,
        transmitter_state: state.transmitter_state,
        logical_ptt: state.logical_ptt,
        logical_ptt_raised,
        logical_ptt_released,
        ctcss_render: state.ctcss_render,
        render_span_count: timeline.count,
        render_spans: timeline.spans,
        dcs_turnoff_remaining_ms: state.dcs_turnoff_remaining_ms,
        tx_hang_remaining_ms: state.tx_hang_remaining_ms,
        finish_remaining_ms: state.finish_remaining_ms,
        buffer_clear_frames: state.buffer_clear_frames,
        settle_remaining_ms: state.settle_remaining_ms,
        ctcss_status_event: actions.ctcss_status_event,
        receiver_blanking_arm: actions.receiver_blanking_arm,
        cpu_halted: state.cpu_halted,
        rx_timer_remainder_frames: state.rx_timer_remainder_frames,
        tx_timer_remainder_frames: state.tx_timer_remainder_frames,
    }
}

#[cfg(test)]
mod tests {
    use std::mem::size_of;

    use super::{
        Config, Error, Input, Output, STATE_ACTIVE, STATE_FINISHING, STATE_IDLE, STATE_TOC,
        ToneOffMode, TransmitSignaling,
    };
    use crate::{
        CtcssRenderState, CtcssRenderStateConfig, DcsTurnoffConfig, SignalModeConfig,
        SignalModeState, TxCompleteConfig,
        ctcss_render_state::{
            OPTION_HOLD, STATE_ACTIVE as CTCSS_ACTIVE, STATE_DISABLED, STATE_TURNOFF,
        },
        signal_mode::{CTCSS_NONE, MODE_CTCSS},
    };

    fn config() -> Config {
        let mut signal_mode = SignalModeConfig::disabled();
        signal_mode.hold_ms = 40;
        signal_mode.ctcss_tx_enabled = 1;
        signal_mode.default_tx_ctcss_frequency_tenths_hz = 1_000;
        signal_mode.mapped_tx_ctcss_frequency_tenths_hz[4] = 1_230;
        Config {
            signal_mode,
            dcs_turnoff: DcsTurnoffConfig {
                turnoff_duration_ms: 180,
            },
            ctcss_render: CtcssRenderStateConfig {
                struct_size: size_of::<CtcssRenderStateConfig>() as u32,
                turnoff_duration_ms: 180,
                turnoff_phase_shift_degrees: 120.0,
                turnoff_tail_tone_hz: 55.0,
            },
            tx_complete: TxCompleteConfig {
                struct_size: size_of::<TxCompleteConfig>() as u32,
                txrx_blanking_time_ms: 37,
            },
            tone_off_mode: ToneOffMode::PhaseShift,
            dcs_transmit_enabled: false,
            dcs_turnoff_enabled: false,
            tx_settle_time_ms: 0,
            tx_cpu_saver_enabled: false,
        }
    }

    fn input(native_frame_count: u32, external_ptt_request: bool) -> Input {
        Input {
            native_frame_count,
            tx_render_admitted: true,
            external_ptt_request,
            physical_ptt_applied: true,
            decoded_ctcss: CTCSS_NONE,
            dcs_valid: false,
            ctcss_inhibit: false,
        }
    }

    /// Per-sample rendering intent used only to compare callback partitions.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct RenderSample {
        logical_ptt: bool,
        oscillator_state: u32,
        enabled: u32,
        ctcss_frequency_tenths_hz: i32,
        tail_tone_hz: f64,
        dcs_turnoff_active: bool,
        phase_shift_degrees: f64,
    }

    /// Expand bounded spans into a canonical waveform-intent sequence for tests.
    fn collect_render_samples(output: Output, samples: &mut Vec<RenderSample>) {
        for span in &output.render_spans[..output.render_span_count] {
            for frame in 0..span.frame_count {
                samples.push(RenderSample {
                    logical_ptt: span.logical_ptt,
                    oscillator_state: span.ctcss_render.oscillator_state,
                    enabled: span.ctcss_render.enabled,
                    ctcss_frequency_tenths_hz: span.ctcss_frequency_tenths_hz,
                    tail_tone_hz: span.ctcss_render.tail_tone_hz,
                    dcs_turnoff_active: span.dcs_turnoff_active,
                    phase_shift_degrees: if frame == 0 {
                        span.ctcss_render.phase_shift_degrees
                    } else {
                        0.0
                    },
                });
            }
        }
    }

    /// Irregular callback partition totaling one historical 20 ms span.
    const IRREGULAR_LEGACY_INTERVAL: [u32; 5] = [5, 1, 7, 5, 942];

    /// One-shot notifications observed while advancing one logical interval.
    #[derive(Debug, Default, PartialEq, Eq)]
    struct GroupEvents {
        ptt_raised: bool,
        ptt_released: bool,
        ctcss_status: bool,
        blanking_arms_ms: Vec<i32>,
    }

    /// Construct one test callback with all non-timing inputs held stable.
    fn tick_input(
        native_frame_count: u32,
        external_ptt_request: bool,
        physical_ptt_applied: bool,
        tx_render_admitted: bool,
    ) -> Input {
        Input {
            native_frame_count,
            tx_render_admitted,
            external_ptt_request,
            physical_ptt_applied,
            decoded_ctcss: CTCSS_NONE,
            dcs_valid: false,
            ctcss_inhibit: false,
        }
    }

    /// Advance one logical interval through caller-selected callback partitions.
    ///
    /// The helper records renderer intent and one-shot control actions so a
    /// 960-frame legacy call can be compared directly with the same native
    /// PCM time delivered as irregular callback spans.
    fn drive_interval(
        signaling: &mut TransmitSignaling,
        partitions: &[u32],
        external_ptt_request: bool,
        physical_ptt_applied: bool,
        tx_render_admitted: bool,
        samples: &mut Vec<RenderSample>,
    ) -> GroupEvents {
        let mut events = GroupEvents::default();

        for &native_frame_count in partitions {
            let output = signaling
                .tick(tick_input(
                    native_frame_count,
                    external_ptt_request,
                    physical_ptt_applied,
                    tx_render_admitted,
                ))
                .unwrap();
            let rendered_frames = output.render_spans[..output.render_span_count]
                .iter()
                .map(|span| span.frame_count)
                .sum::<u32>();
            assert_eq!(
                rendered_frames,
                if tx_render_admitted {
                    native_frame_count
                } else {
                    0
                }
            );
            collect_render_samples(output, samples);
            events.ptt_raised |= output.logical_ptt_raised;
            events.ptt_released |= output.logical_ptt_released;
            events.ctcss_status |= output.ctcss_status_event;
            if let Some(blanking) = output.receiver_blanking_arm {
                events.blanking_arms_ms.push(blanking.duration_ms);
            }
        }
        events
    }

    /// Assert that callback partitioning preserves a complete transmit trace.
    fn assert_partition_invariant(policy: Config, intervals: &[(bool, bool, bool)]) {
        let mut whole = TransmitSignaling::new(policy).unwrap();
        let mut split = TransmitSignaling::new(policy).unwrap();
        let mut whole_samples = Vec::new();
        let mut split_samples = Vec::new();

        for &(external_ptt_request, physical_ptt_applied, tx_render_admitted) in intervals {
            let whole_events = drive_interval(
                &mut whole,
                &[960],
                external_ptt_request,
                physical_ptt_applied,
                tx_render_admitted,
                &mut whole_samples,
            );
            let split_events = drive_interval(
                &mut split,
                &IRREGULAR_LEGACY_INTERVAL,
                external_ptt_request,
                physical_ptt_applied,
                tx_render_admitted,
                &mut split_samples,
            );
            assert_eq!(whole_events, split_events);
            assert_eq!(whole.snapshot(), split.snapshot());
        }
        assert_eq!(whole_samples, split_samples);
    }

    #[test]
    fn start_snapshot_matches_the_legacy_ctcss_mapping_and_ptt_vector() {
        let mut signaling = TransmitSignaling::new(config()).unwrap();
        let mut start = input(960, true);
        start.decoded_ctcss = 4;
        let output = signaling.tick(start).unwrap();

        assert_eq!(
            output.signal_mode,
            SignalModeState {
                smode: MODE_CTCSS,
                smode_was: MODE_CTCSS,
                smode_timer_ms: 40,
                last_rx_ctcss: 4,
                tx_ctcss_frequency_tenths_hz: 1_230,
                tx_ctcss_option: OPTION_HOLD,
                smode_turnoff: 0,
            }
        );
        assert_eq!(output.transmitter_state, STATE_ACTIVE);
        assert!(output.logical_ptt);
        assert!(output.logical_ptt_raised);
        assert!(!output.logical_ptt_released);
        assert_eq!(
            output.ctcss_render,
            CtcssRenderState {
                option: OPTION_HOLD,
                oscillator_state: CTCSS_ACTIVE,
                enabled: 1,
                turnoff_remaining_ms: 0,
                phase_shift_degrees: 0.0,
                tail_tone_hz: 0.0,
            }
        );
        assert_eq!(output.render_span_count, 1);
        assert_eq!(output.render_spans[0].frame_offset, 0);
        assert_eq!(output.render_spans[0].frame_count, 960);
        assert!(output.render_spans[0].logical_ptt);
        assert_eq!(output.render_spans[0].ctcss_frequency_tenths_hz, 1_230);
        assert_eq!(output.dcs_turnoff_remaining_ms, 0);
        assert_eq!(output.tx_hang_remaining_ms, 0);
        assert_eq!(output.finish_remaining_ms, 0);
        assert_eq!(output.buffer_clear_frames, 0);
        assert_eq!(output.settle_remaining_ms, 0);
        assert!(output.ctcss_status_event);
        assert_eq!(output.receiver_blanking_arm, None);
        assert!(!output.cpu_halted);
        assert_eq!(output.rx_timer_remainder_frames, 0);
        assert_eq!(output.tx_timer_remainder_frames, 0);
    }

    #[test]
    fn first_key_initializes_mode_hold_without_changing_ptt_or_render_intent() {
        let mut signaling = TransmitSignaling::new(config()).unwrap();

        let output = signaling.tick(input(960, true)).unwrap();

        assert_eq!(output.transmitter_state, STATE_ACTIVE);
        assert!(output.logical_ptt);
        assert!(output.logical_ptt_raised);
        assert_eq!(output.signal_mode.smode_timer_ms, 40);
        assert_eq!(output.render_span_count, 1);
        assert_eq!(output.render_spans[0].frame_count, 960);
        assert!(output.render_spans[0].logical_ptt);
        assert_eq!(
            output.render_spans[0].ctcss_render.oscillator_state,
            CTCSS_ACTIVE
        );
    }

    #[test]
    fn held_dac_span_advances_signal_mode_but_not_transmit_timing() {
        let mut policy = config();
        policy.tx_settle_time_ms = 40;
        let mut signaling = TransmitSignaling::new(policy).unwrap();
        let mut start = input(960, true);
        start.physical_ptt_applied = false;
        start.decoded_ctcss = 4;
        assert_eq!(signaling.tick(start).unwrap().settle_remaining_ms, 40);

        let mut held = input(960, false);
        held.tx_render_admitted = false;
        let output = signaling.tick(held).unwrap();
        assert_eq!(output.transmitter_state, STATE_ACTIVE);
        assert!(output.logical_ptt);
        assert_eq!(output.settle_remaining_ms, 40);
        assert_eq!(output.tx_timer_remainder_frames, 0);
        assert_eq!(output.signal_mode.smode, MODE_CTCSS);
        assert_eq!(output.signal_mode.smode_timer_ms, 20);
    }

    #[test]
    fn phase_and_tail_tone_vectors_preserve_the_ctcss_renderer_intent() {
        let mut phase = TransmitSignaling::new(config()).unwrap();
        assert!(phase.tick(input(960, true)).unwrap().logical_ptt);
        let phase_output = phase.tick(input(960, false)).unwrap();
        assert_eq!(phase_output.transmitter_state, STATE_TOC);
        assert_eq!(phase_output.ctcss_render.oscillator_state, STATE_TURNOFF);
        assert_eq!(phase_output.ctcss_render.phase_shift_degrees, 0.0);
        assert_eq!(
            phase_output.render_spans[0]
                .ctcss_render
                .phase_shift_degrees,
            120.0
        );
        assert_eq!(phase_output.ctcss_render.tail_tone_hz, 0.0);
        assert_eq!(phase_output.ctcss_render.turnoff_remaining_ms, 160);

        let mut tail_policy = config();
        tail_policy.tone_off_mode = ToneOffMode::TailTone;
        tail_policy.ctcss_render.turnoff_duration_ms = 250;
        let mut tail = TransmitSignaling::new(tail_policy).unwrap();
        assert!(tail.tick(input(960, true)).unwrap().logical_ptt);
        let tail_output = tail.tick(input(960, false)).unwrap();
        assert_eq!(tail_output.transmitter_state, STATE_TOC);
        assert_eq!(tail_output.ctcss_render.oscillator_state, STATE_TURNOFF);
        assert_eq!(tail_output.ctcss_render.phase_shift_degrees, 0.0);
        assert_eq!(
            tail_output.render_spans[0].ctcss_render.phase_shift_degrees,
            0.0
        );
        assert_eq!(tail_output.ctcss_render.tail_tone_hz, 55.0);
        assert_eq!(tail_output.ctcss_render.turnoff_remaining_ms, 230);
    }

    #[test]
    fn fixed_960_vectors_match_legacy_release_and_tail_transitions() {
        let mut normal_policy = config();
        normal_policy.tone_off_mode = ToneOffMode::None;
        let mut normal = TransmitSignaling::new(normal_policy).unwrap();
        assert!(normal.tick(input(960, true)).unwrap().logical_ptt);
        let first_finish = normal.tick(input(960, false)).unwrap();
        assert_eq!(first_finish.transmitter_state, STATE_FINISHING);
        assert!(first_finish.logical_ptt);
        assert_eq!(first_finish.finish_remaining_ms, 60);
        assert_eq!(first_finish.buffer_clear_frames, 3);
        assert_eq!(first_finish.ctcss_render.oscillator_state, STATE_DISABLED);
        assert_eq!(
            normal.tick(input(960, false)).unwrap().finish_remaining_ms,
            40
        );
        assert_eq!(
            normal.tick(input(960, false)).unwrap().finish_remaining_ms,
            20
        );
        let normal_complete = normal.tick(input(960, false)).unwrap();
        assert_eq!(normal_complete.transmitter_state, STATE_IDLE);
        assert!(!normal_complete.logical_ptt);
        assert!(normal_complete.logical_ptt_released);
        assert!(normal_complete.ctcss_status_event);
        assert_eq!(
            normal_complete.receiver_blanking_arm.unwrap().duration_ms,
            37
        );

        let mut no_tone_policy = config();
        no_tone_policy.tone_off_mode = ToneOffMode::ToneRemove;
        let mut no_tone = TransmitSignaling::new(no_tone_policy).unwrap();
        assert!(no_tone.tick(input(960, true)).unwrap().logical_ptt);
        let no_tone_tail = no_tone.tick(input(960, false)).unwrap();
        assert_eq!(no_tone_tail.transmitter_state, STATE_TOC);
        assert!(no_tone_tail.logical_ptt);
        assert_eq!(no_tone_tail.tx_hang_remaining_ms, 160);
        assert_eq!(no_tone_tail.ctcss_render.oscillator_state, STATE_DISABLED);

        let mut phase = TransmitSignaling::new(config()).unwrap();
        assert!(phase.tick(input(960, true)).unwrap().logical_ptt);
        let phase_tail = phase.tick(input(960, false)).unwrap();
        assert_eq!(phase_tail.transmitter_state, STATE_TOC);
        assert_eq!(phase_tail.ctcss_render.oscillator_state, STATE_TURNOFF);
        assert_eq!(phase_tail.ctcss_render.turnoff_remaining_ms, 160);
        assert_eq!(
            phase_tail.render_spans[0].ctcss_render.phase_shift_degrees,
            120.0
        );

        let mut tail_policy = config();
        tail_policy.tone_off_mode = ToneOffMode::TailTone;
        tail_policy.ctcss_render.turnoff_duration_ms = 250;
        let mut tail = TransmitSignaling::new(tail_policy).unwrap();
        assert!(tail.tick(input(960, true)).unwrap().logical_ptt);
        let tail_tone = tail.tick(input(960, false)).unwrap();
        assert_eq!(tail_tone.transmitter_state, STATE_TOC);
        assert_eq!(tail_tone.ctcss_render.oscillator_state, STATE_TURNOFF);
        assert_eq!(tail_tone.ctcss_render.turnoff_remaining_ms, 230);
        assert_eq!(tail_tone.render_spans[0].ctcss_render.tail_tone_hz, 55.0);

        let mut dcs_policy = config();
        dcs_policy.dcs_transmit_enabled = true;
        dcs_policy.dcs_turnoff_enabled = true;
        let mut dcs = TransmitSignaling::new(dcs_policy).unwrap();
        assert!(dcs.tick(input(960, true)).unwrap().logical_ptt);
        let dcs_tail = dcs.tick(input(960, false)).unwrap();
        assert_eq!(dcs_tail.transmitter_state, STATE_TOC);
        assert_eq!(dcs_tail.dcs_turnoff_remaining_ms, 160);
        assert!(dcs_tail.render_spans[0].dcs_turnoff_active);
        let dcs_rekey = dcs.tick(input(960, true)).unwrap();
        assert_eq!(dcs_rekey.transmitter_state, STATE_ACTIVE);
        assert_eq!(dcs_rekey.dcs_turnoff_remaining_ms, 0);
        assert!(dcs_rekey.logical_ptt);
        assert_eq!(dcs_rekey.signal_mode.smode_timer_ms, 40);
    }

    #[test]
    fn tx_hold_freezes_tail_until_an_admitted_dac_span_resumes() {
        let mut signaling = TransmitSignaling::new(config()).unwrap();
        assert!(signaling.tick(input(960, true)).unwrap().logical_ptt);
        let tail = signaling.tick(input(960, false)).unwrap();
        assert_eq!(tail.transmitter_state, STATE_TOC);
        assert_eq!(tail.ctcss_render.turnoff_remaining_ms, 160);

        let mut held_samples = Vec::new();
        let held_events = drive_interval(
            &mut signaling,
            &IRREGULAR_LEGACY_INTERVAL,
            false,
            true,
            false,
            &mut held_samples,
        );
        assert!(held_samples.is_empty());
        assert_eq!(held_events, GroupEvents::default());
        let held = signaling.snapshot();
        assert_eq!(held.transmitter_state, STATE_TOC);
        assert!(held.logical_ptt);
        assert_eq!(held.ctcss_render.turnoff_remaining_ms, 160);
        assert_eq!(held.tx_timer_remainder_frames, 0);

        let resumed = signaling.tick(input(960, false)).unwrap();
        assert_eq!(resumed.transmitter_state, STATE_TOC);
        assert_eq!(resumed.ctcss_render.turnoff_remaining_ms, 140);
    }

    #[test]
    fn irregular_partitions_preserve_all_transmit_timing_and_actions() {
        let mut normal_policy = config();
        normal_policy.tone_off_mode = ToneOffMode::None;
        let mut normal_intervals = vec![(true, true, true)];
        normal_intervals.extend(std::iter::repeat_n((false, true, true), 6));
        assert_partition_invariant(normal_policy, &normal_intervals);

        let mut no_tone_policy = config();
        no_tone_policy.tone_off_mode = ToneOffMode::ToneRemove;
        no_tone_policy.ctcss_render.turnoff_duration_ms = 150;
        let mut no_tone_intervals = vec![(true, true, true)];
        no_tone_intervals.extend(std::iter::repeat_n((false, true, true), 10));
        assert_partition_invariant(no_tone_policy, &no_tone_intervals);

        let mut phase_intervals = vec![(true, true, true)];
        phase_intervals.extend(std::iter::repeat_n((false, true, true), 16));
        assert_partition_invariant(config(), &phase_intervals);

        let mut tail_policy = config();
        tail_policy.tone_off_mode = ToneOffMode::TailTone;
        tail_policy.ctcss_render.turnoff_duration_ms = 250;
        let mut tail_intervals = vec![(true, true, true)];
        tail_intervals.extend(std::iter::repeat_n((false, true, true), 25));
        assert_partition_invariant(tail_policy, &tail_intervals);

        let mut dcs_policy = config();
        dcs_policy.dcs_transmit_enabled = true;
        dcs_policy.dcs_turnoff_enabled = true;
        let mut dcs_intervals = vec![(true, true, true), (false, true, true), (true, true, true)];
        dcs_intervals.extend(std::iter::repeat_n((false, true, true), 16));
        assert_partition_invariant(dcs_policy, &dcs_intervals);

        let mut settle_policy = Config::disabled();
        settle_policy.tx_settle_time_ms = 40;
        assert_partition_invariant(
            settle_policy,
            &[
                (true, false, true),
                (true, false, true),
                (true, true, true),
                (true, true, true),
            ],
        );

        assert_partition_invariant(
            config(),
            &[
                (true, true, true),
                (false, true, true),
                (false, true, false),
                (false, true, false),
                (false, true, true),
            ],
        );
    }

    #[test]
    fn no_tone_tail_rekeys_and_completes_with_receiver_blanking() {
        let mut policy = config();
        policy.tone_off_mode = ToneOffMode::ToneRemove;
        policy.ctcss_render.turnoff_duration_ms = 40;
        let mut signaling = TransmitSignaling::new(policy).unwrap();
        assert!(signaling.tick(input(960, true)).unwrap().logical_ptt);
        let tail = signaling.tick(input(960, false)).unwrap();
        assert_eq!(tail.transmitter_state, STATE_TOC);
        assert!(tail.logical_ptt);
        assert_eq!(tail.tx_hang_remaining_ms, 20);
        assert_eq!(tail.ctcss_render.oscillator_state, STATE_DISABLED);

        let rekey = signaling.tick(input(960, true)).unwrap();
        assert_eq!(rekey.transmitter_state, STATE_ACTIVE);
        assert_eq!(rekey.ctcss_render.oscillator_state, CTCSS_ACTIVE);
        assert_eq!(rekey.ctcss_render.enabled, 1);
        assert_eq!(rekey.signal_mode.smode_timer_ms, 40);

        assert_eq!(
            signaling
                .tick(input(960, false))
                .unwrap()
                .tx_hang_remaining_ms,
            20
        );
        let complete = signaling.tick(input(960, false)).unwrap();
        assert_eq!(complete.transmitter_state, STATE_IDLE);
        assert!(!complete.logical_ptt);
        assert!(complete.logical_ptt_released);
        assert!(complete.ctcss_status_event);
        assert_eq!(complete.receiver_blanking_arm.unwrap().duration_ms, 37);
    }

    #[test]
    fn dcs_tail_rekeys_without_finishing_and_consumes_its_first_span() {
        let mut policy = config();
        policy.dcs_transmit_enabled = true;
        policy.dcs_turnoff_enabled = true;
        let mut signaling = TransmitSignaling::new(policy).unwrap();
        assert!(signaling.tick(input(960, true)).unwrap().logical_ptt);

        let tail = signaling.tick(input(960, false)).unwrap();
        assert_eq!(tail.transmitter_state, STATE_TOC);
        assert_eq!(tail.dcs_turnoff_remaining_ms, 160);
        assert_eq!(tail.tx_hang_remaining_ms, 0);
        assert_eq!(tail.render_span_count, 1);
        assert!(tail.render_spans[0].dcs_turnoff_active);
        assert_eq!(
            tail.render_spans[0].ctcss_render.oscillator_state,
            CTCSS_ACTIVE
        );

        let rekey = signaling.tick(input(960, true)).unwrap();
        assert_eq!(rekey.transmitter_state, STATE_ACTIVE);
        assert_eq!(rekey.dcs_turnoff_remaining_ms, 0);
        assert!(rekey.logical_ptt);
        assert_eq!(rekey.signal_mode.smode_timer_ms, 40);
    }

    #[test]
    fn physical_ptt_is_the_only_settle_timer_clock() {
        let mut policy = config();
        policy.signal_mode.ctcss_tx_enabled = 0;
        policy.tx_settle_time_ms = 40;
        let mut signaling = TransmitSignaling::new(policy).unwrap();
        let mut start = input(960, true);
        start.physical_ptt_applied = false;
        assert_eq!(signaling.tick(start).unwrap().settle_remaining_ms, 40);

        let mut still_waiting = input(960, true);
        still_waiting.physical_ptt_applied = false;
        assert_eq!(
            signaling.tick(still_waiting).unwrap().settle_remaining_ms,
            40
        );
        assert_eq!(
            signaling
                .tick(input(960, true))
                .unwrap()
                .settle_remaining_ms,
            20
        );
    }

    #[test]
    fn cpu_saver_halts_only_after_completion_and_defers_renderer_consumption() {
        let mut policy = Config::disabled();
        policy.tx_cpu_saver_enabled = true;
        policy.tx_complete.txrx_blanking_time_ms = 11;
        let mut signaling = TransmitSignaling::new(policy).unwrap();
        assert!(signaling.tick(input(960, false)).unwrap().cpu_halted);
        assert!(!signaling.tick(input(960, true)).unwrap().cpu_halted);
        assert_eq!(
            signaling.tick(input(960, false)).unwrap().transmitter_state,
            STATE_FINISHING
        );

        let _ = signaling.tick(input(960, false)).unwrap();
        let _ = signaling.tick(input(960, false)).unwrap();
        let output = signaling.tick(input(960, false)).unwrap();
        assert_eq!(output.transmitter_state, STATE_IDLE);
        assert!(output.cpu_halted);
        assert_eq!(output.ctcss_render.option, 3);
        assert_eq!(output.receiver_blanking_arm.unwrap().duration_ms, 11);
    }

    #[test]
    fn irregular_callback_partitions_match_whole_callback_tail_timing() {
        let mut policy = config();
        policy.tone_off_mode = ToneOffMode::TailTone;
        policy.ctcss_render.turnoff_duration_ms = 150;
        let mut whole = TransmitSignaling::new(policy).unwrap();
        let mut split = TransmitSignaling::new(policy).unwrap();
        assert!(whole.tick(input(960, true)).unwrap().logical_ptt);
        assert!(split.tick(input(960, true)).unwrap().logical_ptt);

        let mut whole_samples = Vec::new();
        let mut split_samples = Vec::new();
        for _ in 0..18 {
            collect_render_samples(whole.tick(input(960, false)).unwrap(), &mut whole_samples);
            for frames in [5, 1, 7, 5, 942] {
                collect_render_samples(
                    split.tick(input(frames, false)).unwrap(),
                    &mut split_samples,
                );
            }
        }

        assert_eq!(whole_samples, split_samples);
        let whole_output = whole.snapshot();
        let split_output = split.snapshot();
        assert_eq!(whole_output.transmitter_state, STATE_IDLE);
        assert_eq!(
            whole_output.transmitter_state,
            split_output.transmitter_state
        );
        assert_eq!(whole_output.logical_ptt, split_output.logical_ptt);
        assert_eq!(whole_output.ctcss_render, split_output.ctcss_render);
        assert_eq!(
            whole_output.dcs_turnoff_remaining_ms,
            split_output.dcs_turnoff_remaining_ms
        );
        assert_eq!(
            whole_output.tx_hang_remaining_ms,
            split_output.tx_hang_remaining_ms
        );
        assert_eq!(
            whole_output.finish_remaining_ms,
            split_output.finish_remaining_ms
        );
    }

    #[test]
    fn tail_tone_uses_explicit_tail_gap_and_drain_deadlines() {
        let mut policy = config();
        policy.tone_off_mode = ToneOffMode::TailTone;
        policy.ctcss_render.turnoff_duration_ms = 150;
        let mut signaling = TransmitSignaling::new(policy).unwrap();
        assert!(signaling.tick(input(960, true)).unwrap().logical_ptt);

        let tail_and_gap = signaling.tick(input(8_640, false)).unwrap();
        assert_eq!(tail_and_gap.transmitter_state, STATE_FINISHING);
        assert!(tail_and_gap.logical_ptt);
        assert_eq!(tail_and_gap.finish_remaining_ms, 180);
        assert_eq!(tail_and_gap.render_span_count, 2);
        assert_eq!(tail_and_gap.render_spans[0].frame_count, 7_680);
        assert_eq!(
            tail_and_gap.render_spans[0].ctcss_render.oscillator_state,
            STATE_TURNOFF
        );
        assert_eq!(tail_and_gap.render_spans[0].ctcss_render.tail_tone_hz, 55.0);
        assert_eq!(tail_and_gap.render_spans[1].frame_count, 960);
        assert_eq!(
            tail_and_gap.render_spans[1].ctcss_render.oscillator_state,
            STATE_DISABLED
        );

        let finished = signaling.tick(input(8_640, false)).unwrap();
        assert_eq!(finished.transmitter_state, STATE_IDLE);
        assert!(!finished.logical_ptt);
        assert!(finished.logical_ptt_released);
        assert_eq!(finished.receiver_blanking_arm.unwrap().duration_ms, 37);
    }

    #[test]
    fn dcs_tail_uses_its_exact_duration_and_carries_residual_into_finish() {
        let mut policy = config();
        policy.dcs_transmit_enabled = true;
        policy.dcs_turnoff_enabled = true;
        policy.dcs_turnoff.turnoff_duration_ms = 150;
        let mut signaling = TransmitSignaling::new(policy).unwrap();
        assert!(signaling.tick(input(960, true)).unwrap().logical_ptt);

        let output = signaling.tick(input(12_000, false)).unwrap();
        assert_eq!(output.transmitter_state, STATE_IDLE);
        assert!(!output.logical_ptt);
        assert!(output.logical_ptt_released);
        assert_eq!(output.render_span_count, 3);
        assert_eq!(output.render_spans[0].frame_count, 7_200);
        assert!(output.render_spans[0].dcs_turnoff_active);
        assert_eq!(
            output.render_spans[0].ctcss_render.oscillator_state,
            CTCSS_ACTIVE
        );
        assert_eq!(output.render_spans[1].frame_count, 3_840);
        assert!(!output.render_spans[1].dcs_turnoff_active);
        assert!(output.render_spans[1].logical_ptt);
        assert_eq!(output.render_spans[2].frame_count, 960);
        assert!(!output.render_spans[2].logical_ptt);
    }

    #[test]
    fn no_tone_dwell_expires_at_its_exact_sample_deadline() {
        let mut policy = config();
        policy.tone_off_mode = ToneOffMode::ToneRemove;
        policy.ctcss_render.turnoff_duration_ms = 150;
        let mut whole = TransmitSignaling::new(policy).unwrap();
        let mut split = TransmitSignaling::new(policy).unwrap();
        assert!(whole.tick(input(960, true)).unwrap().logical_ptt);
        assert!(split.tick(input(960, true)).unwrap().logical_ptt);

        let mut whole_samples = Vec::new();
        let mut split_samples = Vec::new();
        for _ in 0..7 {
            collect_render_samples(whole.tick(input(960, false)).unwrap(), &mut whole_samples);
            for frames in [5, 1, 7, 5, 942] {
                collect_render_samples(
                    split.tick(input(frames, false)).unwrap(),
                    &mut split_samples,
                );
            }
        }
        collect_render_samples(whole.tick(input(480, false)).unwrap(), &mut whole_samples);
        collect_render_samples(split.tick(input(480, false)).unwrap(), &mut split_samples);

        assert_eq!(whole_samples, split_samples);
        assert_eq!(whole_samples.len(), 7_200);
        assert!(whole_samples.iter().all(|sample| sample.logical_ptt));
        assert!(
            whole_samples
                .iter()
                .all(|sample| sample.oscillator_state == STATE_DISABLED)
        );
        assert_eq!(whole.snapshot().transmitter_state, STATE_IDLE);
        assert_eq!(split.snapshot().transmitter_state, STATE_IDLE);
    }

    #[test]
    fn checked_constructor_rejects_immutable_dcs_tail_policy() {
        let mut policy = config();
        policy.dcs_transmit_enabled = true;
        policy.dcs_turnoff_enabled = true;
        policy.dcs_turnoff.turnoff_duration_ms = 0;
        assert_eq!(
            TransmitSignaling::new(policy),
            Err(Error::InvalidConfigurationOrState)
        );
    }

    #[test]
    fn checked_constructor_rejects_each_immutable_callback_failure() {
        let mut invalid_ctcss = config();
        invalid_ctcss.ctcss_render.turnoff_tail_tone_hz = f64::NAN;
        assert_eq!(
            TransmitSignaling::new(invalid_ctcss),
            Err(Error::InvalidConfigurationOrState)
        );

        let mut invalid_complete = config();
        invalid_complete.tx_complete.struct_size = 0;
        assert_eq!(
            TransmitSignaling::new(invalid_complete),
            Err(Error::InvalidConfigurationOrState)
        );

        let mut invalid_settle = config();
        invalid_settle.tx_settle_time_ms = -1;
        assert_eq!(
            TransmitSignaling::new(invalid_settle),
            Err(Error::InvalidConfigurationOrState)
        );

        let mut long_settle = config();
        long_settle.tx_settle_time_ms = i32::MAX;
        assert!(TransmitSignaling::new(long_settle).is_ok());
    }

    #[test]
    fn invalid_input_is_transactional() {
        let mut signaling = TransmitSignaling::new(config()).unwrap();
        let before = signaling.snapshot();
        let invalid = Input {
            native_frame_count: 0,
            ..input(960, false)
        };
        assert_eq!(signaling.tick(invalid), Err(Error::InvalidInput));
        assert_eq!(signaling.snapshot(), before);
    }
}
