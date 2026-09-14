//! Sample-clocked receive qualification.
//!
//! This portable component preserves the compatibility adapter's receive
//! policy without carrying adapter or hardware types into the portable core.
//! It resolves canonical GPIO and DSP snapshots immediately, then measures
//! receive and post-transmit qualification from the observed signaling edges
//! in native sample time.  No callback-global 20 ms phase changes an edge's
//! real-time delay.

/// Native frames in one configured 20 ms qualification interval.
const QUALIFICATION_INTERVAL_FRAMES: u64 = 960;

/// Largest supported post-transmit delay: 60 seconds in native frames.
const MAX_TX_OFF_DELAY_FRAMES: u64 = 3_000 * QUALIFICATION_INTERVAL_FRAMES;

/// Selects the raw signal used as the receiver carrier indication.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum CarrierSource {
    /// Do not admit a carrier indication from this source.
    #[default]
    Ignore,
    /// Use the normal-polarity CM119/HID input.
    Hardware,
    /// Use the inverted CM119/HID input.
    HardwareInverted,
    /// Use the native DSP noise-squelch indication.
    DspNoise,
    /// Use the native DSP VOX indication.
    DspVox,
    /// Use the normal-polarity parallel input.
    Parallel,
    /// Use the inverted parallel input.
    ParallelInverted,
}

/// Selects the indication used as the receiver subaudible qualification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SubaudibleSource {
    /// Retain the native CTCSS/DCS decision unless both sources are ignored.
    #[default]
    Ignore,
    /// Use the native CTCSS/DCS decision.
    Dsp,
    /// Use the normal-polarity CM119/HID input.
    Hardware,
    /// Use the inverted CM119/HID input.
    HardwareInverted,
    /// Use the normal-polarity parallel input.
    Parallel,
    /// Use the inverted parallel input.
    ParallelInverted,
}

/// Signal mode relevant to CTCSS and DCS qualification.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SignalMode {
    /// No recognized subaudible mode is active.
    #[default]
    None,
    /// The CTCSS mode owns the current received code.
    Ctcss,
    /// The DCS mode owns the current received code.
    Dcs,
}

/// Immutable receive-qualification policy resolved by the control plane.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Config {
    /// Configured source for COR qualification.
    pub carrier_source: CarrierSource,
    /// Configured source for subaudible qualification.
    pub subaudible_source: SubaudibleSource,
    /// Permit receive when the legacy CTCSS override is asserted.
    pub subaudible_override: bool,
    /// Permit carrier during logical transmit for the advanced transport.
    pub advanced_transport: bool,
    /// Permit carrier during logical transmit for a duplex radio.
    pub radio_duplex: bool,
    /// Whole 20 ms qualification intervals required before receiver keying.
    pub rx_on_delay_frames: u32,
    /// Whole 20 ms intervals required after transmitter release.
    pub tx_off_delay_frames: u32,
}

/// Canonical adapter snapshots supplied once for a bounded native tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// Current normal-polarity CM119/HID carrier input.
    pub hardware_carrier: bool,
    /// Current normal-polarity parallel carrier input.
    pub parallel_carrier: bool,
    /// Current native DSP carrier indication.
    pub dsp_carrier: bool,
    /// Current normal-polarity CM119/HID CTCSS input.
    pub hardware_subaudible: bool,
    /// Current normal-polarity parallel CTCSS input.
    pub parallel_subaudible: bool,
    /// One when a DCS receiver is configured.
    pub dcs_receive_enabled: bool,
    /// Current DCS decoder validity.
    pub dcs_valid: bool,
    /// One when CTCSS receive decoding is configured.
    pub ctcss_receive_enabled: bool,
    /// One when a configured CTCSS decoder object is available.
    pub ctcss_decoder_available: bool,
    /// Current CTCSS decoder validity.
    pub ctcss_decoded: bool,
    /// Current signaling mode selected by the receive engine.
    pub signal_mode: SignalMode,
    /// Current logical transmitter PTT output.
    pub tx_ptt_out: bool,
    /// One while post-transmit receive blanking remains active.
    pub txrx_blanking_active: bool,
}

/// Callback-owned receive qualification state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Last externally published hardware carrier state, preserving legacy HID semantics.
    pub external_carrier_detect: bool,
    /// Current resolved carrier indication before on/off qualification.
    pub carrier_active: bool,
    /// Current resolved CTCSS/DCS indication before on/off qualification.
    pub subaudible_active: bool,
    /// Current qualified receiver state.
    pub rx_keyed: bool,
    /// Consecutive admitted native frames since the observed receive rise.
    pub rx_admission_elapsed_frames: u64,
    /// Native frames elapsed since the observed logical PTT release.
    ///
    /// Startup with PTT already deasserted treats the first supplied frame as
    /// post-release time, matching the configured post-transmit guard.
    pub tx_release_elapsed_frames: u64,
}

/// Observable result from one receive-qualification update.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Output {
    /// Resolved carrier indication immediately observable after this tick.
    pub carrier_active: bool,
    /// Resolved CTCSS/DCS indication immediately observable after this tick.
    pub subaudible_active: bool,
    /// Receiver state after the current span's elapsed qualification time.
    pub rx_keyed: bool,
}

/// Resolve the legacy DSP CTCSS/DCS decision before an external source override.
fn dsp_subaudible_active(inputs: &Inputs) -> bool {
    if inputs.dcs_receive_enabled {
        return inputs.dcs_valid && inputs.signal_mode == SignalMode::Dcs;
    }
    if !inputs.ctcss_receive_enabled {
        return true;
    }
    inputs.ctcss_decoder_available
        && inputs.ctcss_decoded
        && inputs.signal_mode == SignalMode::Ctcss
}

/// Resolve published external COS, leaving DSP noise/VOX to their detectors.
pub(crate) fn external_carrier(
    source: CarrierSource,
    hardware: bool,
    parallel: bool,
) -> Option<bool> {
    match source {
        CarrierSource::Ignore => Some(false),
        CarrierSource::Hardware => Some(hardware),
        CarrierSource::HardwareInverted => Some(!hardware),
        CarrierSource::Parallel => Some(parallel),
        CarrierSource::ParallelInverted => Some(!parallel),
        CarrierSource::DspNoise | CarrierSource::DspVox => None,
    }
}

/// Resolve immediate carrier and subaudible indications for one tick.
fn resolve_indications(config: &Config, inputs: &Inputs, state: &mut State) {
    let raw_carrier = external_carrier(
        config.carrier_source,
        inputs.hardware_carrier,
        inputs.parallel_carrier,
    )
    .unwrap_or(inputs.dsp_carrier);
    if matches!(
        config.carrier_source,
        CarrierSource::Hardware | CarrierSource::HardwareInverted
    ) {
        state.external_carrier_detect = raw_carrier;
    }

    let mut carrier =
        raw_carrier && (!inputs.tx_ptt_out || config.advanced_transport || config.radio_duplex);
    if inputs.txrx_blanking_active {
        carrier = false;
    }

    let mut subaudible = dsp_subaudible_active(inputs);
    match config.subaudible_source {
        SubaudibleSource::Ignore | SubaudibleSource::Dsp => {}
        SubaudibleSource::Hardware => subaudible = inputs.hardware_subaudible,
        SubaudibleSource::HardwareInverted => subaudible = !inputs.hardware_subaudible,
        SubaudibleSource::Parallel => subaudible = inputs.parallel_subaudible,
        SubaudibleSource::ParallelInverted => subaudible = !inputs.parallel_subaudible,
    }
    if config.subaudible_override {
        subaudible = true;
    }
    if config.carrier_source == CarrierSource::Ignore
        && config.subaudible_source == SubaudibleSource::Ignore
    {
        carrier = false;
        subaudible = false;
    }

    state.carrier_active = carrier;
    state.subaudible_active = subaudible;
}

/// Convert a configured 20 ms count to its exact native-frame duration.
fn qualification_duration_frames(interval_count: u32) -> u64 {
    u64::from(interval_count) * QUALIFICATION_INTERVAL_FRAMES
}

/// Advance the logical-PTT release guard for one stable input span.
fn advance_tx_release(
    config: &Config,
    inputs: &Inputs,
    native_frame_count: u32,
    state: &mut State,
) {
    if inputs.tx_ptt_out || config.tx_off_delay_frames == 0 {
        state.tx_release_elapsed_frames = 0;
        return;
    }

    state.tx_release_elapsed_frames = state
        .tx_release_elapsed_frames
        .saturating_add(u64::from(native_frame_count))
        .min(MAX_TX_OFF_DELAY_FRAMES);
}

/// Return whether the configured post-transmit guard has elapsed.
fn tx_release_qualified(config: &Config, state: &State) -> bool {
    let required_frames = qualification_duration_frames(config.tx_off_delay_frames);
    required_frames == 0 || state.tx_release_elapsed_frames >= required_frames
}

/// Advance receiver admission from the current observed signaling state.
fn qualify_receive(config: &Config, native_frame_count: u32, state: &mut State) {
    let admitted = state.carrier_active && state.subaudible_active;
    if !admitted {
        state.rx_keyed = false;
        state.rx_admission_elapsed_frames = 0;
        return;
    }

    if state.rx_keyed {
        return;
    }

    let required_frames = qualification_duration_frames(config.rx_on_delay_frames);
    if required_frames != 0 {
        state.rx_admission_elapsed_frames = state
            .rx_admission_elapsed_frames
            .saturating_add(u64::from(native_frame_count))
            .min(required_frames);
    } else {
        state.rx_admission_elapsed_frames = 0;
    }

    if (required_frames == 0 || state.rx_admission_elapsed_frames >= required_frames)
        && tx_release_qualified(config, state)
    {
        state.rx_keyed = true;
    }
}

/// Resolve immediate indications and advance qualification for a native span.
///
/// The caller supplies one stable hardware/DSP snapshot for the span.  An
/// observed signaling change takes effect at the start of that span; configured
/// delays then advance by exactly its native-frame count.  Splitting the same
/// stream into valid callback partitions therefore cannot change qualification
/// duration.  The operation is allocation-free and has no I/O or locking.
#[must_use]
pub fn advance(
    config: &Config,
    inputs: &Inputs,
    native_frame_count: u32,
    state: &mut State,
) -> Output {
    resolve_indications(config, inputs, state);
    advance_tx_release(config, inputs, native_frame_count, state);
    qualify_receive(config, native_frame_count, state);

    Output {
        carrier_active: state.carrier_active,
        subaudible_active: state.subaudible_active,
        rx_keyed: state.rx_keyed,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{
        CarrierSource, Config, Inputs, MAX_TX_OFF_DELAY_FRAMES, Output,
        QUALIFICATION_INTERVAL_FRAMES, SignalMode, State, SubaudibleSource, advance,
    };

    fn active_inputs() -> Inputs {
        Inputs {
            hardware_carrier: true,
            parallel_carrier: true,
            dsp_carrier: true,
            hardware_subaudible: true,
            parallel_subaudible: true,
            ctcss_receive_enabled: false,
            ..Inputs::default()
        }
    }

    fn enabled_config() -> Config {
        Config {
            carrier_source: CarrierSource::Hardware,
            subaudible_source: SubaudibleSource::Ignore,
            radio_duplex: true,
            ..Config::default()
        }
    }

    #[test]
    fn carrier_sources_and_hardware_snapshot_match_the_compatibility_policy() {
        let mut state = State::default();
        let mut inputs = active_inputs();
        let mut config = enabled_config();

        assert!(advance(&config, &inputs, 0, &mut state).carrier_active);
        assert!(state.external_carrier_detect);

        inputs.hardware_carrier = false;
        config.carrier_source = CarrierSource::HardwareInverted;
        assert!(advance(&config, &inputs, 0, &mut state).carrier_active);
        assert!(state.external_carrier_detect);

        config.carrier_source = CarrierSource::DspNoise;
        assert!(advance(&config, &inputs, 0, &mut state).carrier_active);
        config.carrier_source = CarrierSource::DspVox;
        assert!(advance(&config, &inputs, 0, &mut state).carrier_active);

        inputs.parallel_carrier = false;
        config.carrier_source = CarrierSource::ParallelInverted;
        assert!(advance(&config, &inputs, 0, &mut state).carrier_active);
        config.carrier_source = CarrierSource::Parallel;
        assert!(!advance(&config, &inputs, 0, &mut state).carrier_active);
    }

    #[test]
    fn subaudible_precedence_external_sources_and_override_match_legacy() {
        let mut state = State::default();
        let mut inputs = active_inputs();
        let mut config = enabled_config();

        inputs.ctcss_receive_enabled = true;
        inputs.ctcss_decoder_available = true;
        inputs.ctcss_decoded = true;
        inputs.signal_mode = SignalMode::Ctcss;
        config.subaudible_source = SubaudibleSource::Dsp;
        assert!(advance(&config, &inputs, 0, &mut state).subaudible_active);

        inputs.dcs_receive_enabled = true;
        inputs.dcs_valid = true;
        inputs.signal_mode = SignalMode::Dcs;
        assert!(advance(&config, &inputs, 0, &mut state).subaudible_active);
        inputs.signal_mode = SignalMode::Ctcss;
        assert!(!advance(&config, &inputs, 0, &mut state).subaudible_active);

        config.subaudible_source = SubaudibleSource::HardwareInverted;
        inputs.hardware_subaudible = false;
        assert!(advance(&config, &inputs, 0, &mut state).subaudible_active);
        config.subaudible_source = SubaudibleSource::Parallel;
        inputs.parallel_subaudible = false;
        assert!(!advance(&config, &inputs, 0, &mut state).subaudible_active);
        config.subaudible_override = true;
        assert!(advance(&config, &inputs, 0, &mut state).subaudible_active);
    }

    #[test]
    fn duplex_and_blanking_gate_carrier_before_qualification() {
        let mut state = State::default();
        let mut inputs = active_inputs();
        let mut config = enabled_config();

        config.radio_duplex = false;
        inputs.tx_ptt_out = true;
        assert!(!advance(&config, &inputs, 0, &mut state).carrier_active);

        config.advanced_transport = true;
        assert!(advance(&config, &inputs, 0, &mut state).carrier_active);

        inputs.txrx_blanking_active = true;
        assert!(!advance(&config, &inputs, 0, &mut state).carrier_active);
    }

    #[test]
    fn ignored_carrier_and_subaudible_sources_force_the_receiver_closed() {
        let mut state = State::default();
        let config = Config::default();

        assert_eq!(
            advance(&config, &active_inputs(), 0, &mut state),
            Output {
                carrier_active: false,
                subaudible_active: false,
                rx_keyed: false,
            }
        );
    }

    #[test]
    fn incomplete_decodes_and_external_subaudible_sources_keep_their_precedence() {
        let mut config = enabled_config();
        let mut inputs = active_inputs();
        inputs.ctcss_receive_enabled = true;
        inputs.signal_mode = SignalMode::Ctcss;
        for (available, decoded, mode) in [
            (false, true, SignalMode::Ctcss),
            (true, false, SignalMode::Ctcss),
            (true, true, SignalMode::None),
        ] {
            inputs.ctcss_decoder_available = available;
            inputs.ctcss_decoded = decoded;
            inputs.signal_mode = mode;
            assert!(!advance(&config, &inputs, 0, &mut State::default()).subaudible_active);
        }
        inputs.dcs_receive_enabled = true;
        inputs.dcs_valid = false;
        inputs.signal_mode = SignalMode::Dcs;
        assert!(!advance(&config, &inputs, 0, &mut State::default()).subaudible_active);
        config.carrier_source = CarrierSource::Ignore;
        for source in [
            SubaudibleSource::Hardware,
            SubaudibleSource::ParallelInverted,
        ] {
            config.subaudible_source = source;
            inputs.hardware_subaudible = true;
            inputs.parallel_subaudible = false;
            let output = advance(&config, &inputs, 0, &mut State::default());
            assert!(!output.carrier_active);
            assert!(output.subaudible_active);
            assert!(!output.rx_keyed);
        }
    }

    #[test]
    fn configured_delays_use_exact_elapsed_native_frames() {
        let mut state = State::default();
        let mut config = enabled_config();
        let inputs = active_inputs();

        config.rx_on_delay_frames = 2;
        assert!(!advance(&config, &inputs, 1_919, &mut state).rx_keyed);
        assert_eq!(state.rx_admission_elapsed_frames, 1_919);
        assert!(advance(&config, &inputs, 1, &mut state).rx_keyed);
        assert_eq!(
            state.rx_admission_elapsed_frames,
            2 * QUALIFICATION_INTERVAL_FRAMES
        );
        assert!(advance(&config, &inputs, 1, &mut state).rx_keyed);
        assert_eq!(
            state.rx_admission_elapsed_frames,
            2 * QUALIFICATION_INTERVAL_FRAMES
        );

        let mut full_interval_state = State::default();
        assert!(!advance(&config, &inputs, 960, &mut full_interval_state).rx_keyed);
        assert!(advance(&config, &inputs, 960, &mut full_interval_state).rx_keyed);

        config.rx_on_delay_frames = 0;
        let mut immediate_state = State::default();
        assert!(advance(&config, &inputs, 0, &mut immediate_state).rx_keyed);

        config.tx_off_delay_frames = 2;
        let keyed_inputs = Inputs {
            tx_ptt_out: true,
            ..inputs
        };
        let mut holdoff = State::default();
        assert!(!advance(&config, &keyed_inputs, 137, &mut holdoff).rx_keyed);
        assert_eq!(holdoff.tx_release_elapsed_frames, 0);
        assert!(!advance(&config, &inputs, 1_919, &mut holdoff).rx_keyed);
        assert_eq!(holdoff.tx_release_elapsed_frames, 1_919);
        assert!(advance(&config, &inputs, 1, &mut holdoff).rx_keyed);
        assert_eq!(
            holdoff.tx_release_elapsed_frames,
            2 * QUALIFICATION_INTERVAL_FRAMES
        );

        let mut capped_state = State {
            tx_release_elapsed_frames: MAX_TX_OFF_DELAY_FRAMES,
            ..State::default()
        };
        assert!(advance(&config, &inputs, 960, &mut capped_state).rx_keyed);
        assert_eq!(
            capped_state.tx_release_elapsed_frames,
            MAX_TX_OFF_DELAY_FRAMES
        );
    }

    #[test]
    fn receive_rise_has_the_same_delay_after_each_prior_callback_phase() {
        const PARTITIONS: [u32; 7] = [5, 1, 7, 5, 942, 479, 481];
        const PRIOR_PHASES: [u32; 7] = [0, 1, 5, 479, 959, 960, 1_919];
        let config = Config {
            rx_on_delay_frames: 2,
            ..enabled_config()
        };
        let inactive = Inputs {
            ctcss_receive_enabled: false,
            ..Inputs::default()
        };
        let active = active_inputs();

        for prior_frames in PRIOR_PHASES {
            let mut threshold = State::default();
            let mut whole = State::default();
            let mut partitioned = State::default();
            if prior_frames != 0 {
                let _ = advance(&config, &inactive, prior_frames, &mut threshold);
                let _ = advance(&config, &inactive, prior_frames, &mut whole);
                let _ = advance(&config, &inactive, prior_frames, &mut partitioned);
            }

            assert!(
                !advance(&config, &active, 1_919, &mut threshold).rx_keyed,
                "prior phase {prior_frames}"
            );
            assert!(
                advance(&config, &active, 1, &mut threshold).rx_keyed,
                "prior phase {prior_frames}"
            );

            assert!(advance(&config, &active, 1_920, &mut whole).rx_keyed);
            for frames in PARTITIONS {
                let _ = advance(&config, &active, frames, &mut partitioned);
            }
            assert_eq!(partitioned, whole, "prior phase {prior_frames}");
        }
    }

    #[test]
    fn ptt_release_has_the_same_guard_after_each_prior_callback_phase() {
        const PARTITIONS: [u32; 7] = [5, 1, 7, 5, 942, 479, 481];
        const PRIOR_PHASES: [u32; 7] = [0, 1, 5, 479, 959, 960, 1_919];
        let config = Config {
            tx_off_delay_frames: 2,
            ..enabled_config()
        };
        let released = active_inputs();
        let keyed = Inputs {
            tx_ptt_out: true,
            ..released
        };

        for prior_frames in PRIOR_PHASES {
            let mut threshold = State::default();
            let mut whole = State::default();
            let mut partitioned = State::default();
            let _ = advance(&config, &keyed, prior_frames, &mut threshold);
            let _ = advance(&config, &keyed, prior_frames, &mut whole);
            let _ = advance(&config, &keyed, prior_frames, &mut partitioned);

            assert!(
                !advance(&config, &released, 1_919, &mut threshold).rx_keyed,
                "prior phase {prior_frames}"
            );
            assert!(
                advance(&config, &released, 1, &mut threshold).rx_keyed,
                "prior phase {prior_frames}"
            );

            assert!(advance(&config, &released, 1_920, &mut whole).rx_keyed);
            for frames in PARTITIONS {
                let _ = advance(&config, &released, frames, &mut partitioned);
            }
            assert_eq!(partitioned, whole, "prior phase {prior_frames}");
        }
    }

    #[test]
    fn signal_loss_clears_receive_immediately_and_restarts_the_delay() {
        let mut state = State::default();
        let config = Config {
            rx_on_delay_frames: 1,
            ..enabled_config()
        };
        let inactive = Inputs {
            ctcss_receive_enabled: false,
            ..Inputs::default()
        };

        assert!(advance(&config, &active_inputs(), 959, &mut state).carrier_active);
        assert!(!state.rx_keyed);
        assert_eq!(state.rx_admission_elapsed_frames, 959);
        assert!(!advance(&config, &inactive, 0, &mut state).rx_keyed);
        assert_eq!(state.rx_admission_elapsed_frames, 0);
        assert!(!advance(&config, &active_inputs(), 959, &mut state).rx_keyed);
        assert!(advance(&config, &active_inputs(), 1, &mut state).rx_keyed);
    }

    #[test]
    fn zero_span_publishes_indications_without_advancing_nonzero_delay() {
        let mut state = State::default();
        let config = Config {
            rx_on_delay_frames: 1,
            tx_off_delay_frames: 1,
            ..enabled_config()
        };

        assert!(advance(&config, &active_inputs(), 0, &mut state).carrier_active);
        assert_eq!(state.rx_admission_elapsed_frames, 0);
        assert_eq!(state.tx_release_elapsed_frames, 0);
        assert!(!state.rx_keyed);
    }
}
