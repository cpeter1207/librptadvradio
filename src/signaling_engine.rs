//! One owner for native receive detection, radio qualification, and TX signaling.
//!
//! This is the signaling half of the migrating native tick, not a second audio
//! renderer. Delivered voice PCM remains owned by the FFmpeg processing path.
//! The adapter supplies already-published hardware/control snapshots; this
//! object performs no device I/O, Asterisk work, allocation, or locking in a tick.

use crate::{
    RxCpuSaverInput, RxCpuSaverState,
    receive_path::{self, ReceivePath},
    receive_qualification::{self, SignalMode},
    rx_cpu_saver, signal_mode, timer,
    transmit_signaling::{self, TransmitSignaling},
};

/// Default periodic status cadence, independent of audio callback partitioning.
pub const DEFAULT_PUBLICATION_INTERVAL_MS: u32 = 50;

/// Setup policy copied into a stream-owned signaling engine.
#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    /// Detector and compatibility-calibration coefficient/profile selection.
    pub receive: receive_path::Config<'a>,
    /// Source selection, duplex policy, and receiver admission delays.
    pub qualification: receive_qualification::Config,
    /// Transmitter timing, mode mapping, and subaudible turn-off policy.
    pub transmit: transmit_signaling::Config,
    /// Freeze idle compatibility receive work while retaining carrier detection.
    pub receive_cpu_saver_enabled: bool,
    /// Nonzero periodic-status interval; edges do not wait for this deadline.
    pub publication_interval_ms: u32,
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use crate::allocation_test;
    use crate::receive_path::{FilterConfig, FrontendConfig};
    use crate::receive_qualification::{CarrierSource, SubaudibleSource};

    #[test]
    fn renderer_coalescing_keeps_frequency_ctcss_and_dcs_boundaries() {
        let mut engine = SignalingEngine::new(config()).unwrap();
        let mut output = engine.tick(&[0.0, 0.0], keyed()).unwrap();
        let start = engine.render_spans.len();
        output.transmit.render_spans[0].ctcss_frequency_tenths_hz += 1;
        engine.collect_frame(&output, 1);
        assert_eq!(engine.render_spans.len(), start + 1);
        output.transmit.render_spans[0].ctcss_render.tail_tone_hz = 55.0;
        engine.collect_frame(&output, 2);
        assert_eq!(engine.render_spans.len(), start + 2);
        output.transmit.render_spans[0].dcs_turnoff_active = true;
        engine.collect_frame(&output, 3);
        assert_eq!(engine.render_spans.len(), start + 3);
    }

    #[test]
    fn bad_pcm_and_restored_internal_saver_are_rejected() {
        assert_eq!(qualification_mode(signal_mode::MODE_DCS), SignalMode::Dcs);
        assert_eq!(qualification_mode(-1), SignalMode::Other);
        assert_eq!(
            component_invariant(transmit_signaling::Error::InvalidInput),
            Error::ComponentInvariant
        );
        let mut engine = SignalingEngine::new(config()).unwrap();
        assert_eq!(
            engine.tick(&[f32::NAN, 0.0], keyed()),
            Err(Error::InvalidInput)
        );
        engine.receive_cpu_saver.halted = 2;
        assert_eq!(
            engine.tick(&[0.0, 0.0], keyed()),
            Err(Error::ComponentInvariant)
        );
    }

    fn config() -> Config<'static> {
        const IDENTITY: [i16; 1] = [32_767];
        const NOISE: [i16; 1] = [1];
        let filter = FilterConfig {
            coefficients: &IDENTITY,
            input_gain: 256,
            output_gain: 256,
            calc_adjust: 32_767,
        };
        Config {
            receive: receive_path::Config {
                native_sample_rate_hz: 48_000,
                maximum_native_frames: 960,
                frontend: FrontendConfig {
                    baseband_coefficients: &IDENTITY,
                    baseband_calc_adjust: 32_767,
                    baseband_output_gain: 256,
                    noise_coefficients: &NOISE,
                    noise_divisor: 1,
                    decimate: 6,
                    calibration_window: 960,
                    open_level: 1,
                    hysteresis: 0,
                    noise_squelch: false,
                },
                lsd_filter: filter,
                hpf_filter: filter,
                center_slicer: None,
                deemphasis: None,
                delay: None,
                vox: None,
                measurement: None,
                ctcss: None,
                dcs: None,
            },
            qualification: receive_qualification::Config {
                carrier_source: CarrierSource::Hardware,
                subaudible_source: SubaudibleSource::Ignore,
                radio_duplex: true,
                ..receive_qualification::Config::default()
            },
            transmit: transmit_signaling::Config::disabled(),
            receive_cpu_saver_enabled: false,
            publication_interval_ms: DEFAULT_PUBLICATION_INTERVAL_MS,
        }
    }

    fn keyed() -> Inputs {
        Inputs {
            hardware_carrier: true,
            external_ptt_request: true,
            physical_ptt_applied: true,
            tx_render_admitted: true,
            ..Inputs::default()
        }
    }

    #[test]
    fn construction_rejects_invalid_policy_before_any_tick() {
        let mut policy = config();
        policy.publication_interval_ms = 0;
        assert!(matches!(
            SignalingEngine::new(policy),
            Err(Error::InvalidConfiguration)
        ));
        policy = config();
        policy.receive.native_sample_rate_hz = 96_000;
        assert!(matches!(
            SignalingEngine::new(policy),
            Err(Error::InvalidConfiguration)
        ));
        policy = config();
        policy.transmit.signal_mode.ctcss_tx_enabled = 2;
        assert!(matches!(
            SignalingEngine::new(policy),
            Err(Error::InvalidConfiguration)
        ));
    }

    #[test]
    fn rejected_pcm_does_not_advance_any_composed_state() {
        let mut engine = SignalingEngine::new(config()).unwrap();
        let mut reference = SignalingEngine::new(config()).unwrap();
        let oversized = [0.0; 1_922];
        for bad in [&[][..], &[0.0][..], &[f32::NAN, 0.0][..], &oversized[..]] {
            assert_eq!(engine.tick(bad, keyed()), Err(Error::InvalidInput));
        }
        let pcm = [0.0; 1_920];
        assert_eq!(engine.tick(&pcm, keyed()), reference.tick(&pcm, keyed()));
    }

    #[test]
    fn qualified_edges_do_not_wait_for_periodic_status() {
        let mut engine = SignalingEngine::new(config()).unwrap();
        let first = engine.tick(&[0.0; 12], keyed()).unwrap();
        assert!(first.qualification.rx_keyed);
        assert!(first.edges.carrier && first.edges.receiver_keyed && first.edges.ptt);
        assert!(first.transmit.logical_ptt_raised && first.transmit.ctcss_status_event);
        assert!(!first.edges.ctcss && !first.edges.dcs);
        assert!(!first.publish_periodic_status);
        let steady = engine.tick(&[0.0; 12], keyed()).unwrap();
        assert_eq!(steady.edges, Edges::default());
        assert!(!steady.transmit.logical_ptt_raised && !steady.transmit.ctcss_status_event);
        let lost = engine
            .tick(
                &[0.0; 2],
                Inputs {
                    hardware_carrier: false,
                    ..keyed()
                },
            )
            .unwrap();
        assert!(lost.edges.carrier && lost.edges.receiver_keyed);
        assert!(!lost.qualification.rx_keyed);
        assert!(!lost.publish_periodic_status);
    }

    #[test]
    fn periodic_deadlines_round_up_without_accumulating_rounding_error() {
        let mut engine = SignalingEngine::new(config()).unwrap();
        let mut publications = Vec::new();
        for tick in 1..=10 {
            if engine
                .tick(&[0.0; 1_920], Inputs::default())
                .unwrap()
                .publish_periodic_status
            {
                publications.push(tick * 960);
            }
        }
        assert_eq!(publications, [2_880, 4_800, 7_680, 9_600]);
        let mut engine = SignalingEngine::new(config()).unwrap();
        // 2,399 + 1 frames reaches the first 50 ms deadline exactly.
        for frames in [960, 960, 479] {
            assert!(
                !engine
                    .tick(&[0.0; 1_920][..frames * 2], Inputs::default())
                    .unwrap()
                    .publish_periodic_status
            );
        }
        assert!(
            engine
                .tick(&[0.0; 2], Inputs::default())
                .unwrap()
                .publish_periodic_status
        );
    }

    #[test]
    fn receive_progress_and_publication_continue_when_dac_admission_stops() {
        let mut engine = SignalingEngine::new(config()).unwrap();
        let inputs = Inputs {
            tx_render_admitted: false,
            ..keyed()
        };
        let pcm = [0.0; 1_920];
        for index in 0..3 {
            let output = engine.tick(&pcm, inputs).unwrap();
            assert_eq!(output.receive.native_frame_count, 960);
            assert_eq!(output.receive.baseband_frame_count, 160);
            assert!(output.qualification.rx_keyed);
            assert!(!output.transmit.logical_ptt);
            assert_eq!(
                output.transmit.transmitter_state,
                transmit_signaling::STATE_IDLE
            );
            assert_eq!(output.publish_periodic_status, index == 2);
        }
        let admitted = engine.tick(&pcm, keyed()).unwrap();
        assert!(admitted.transmit.logical_ptt && admitted.edges.ptt);
    }

    #[test]
    fn hardware_qualification_and_tx_timing_preserve_irregular_partitions() {
        let mut policy = config();
        policy.qualification.rx_on_delay_frames = 2;
        policy.transmit.tx_settle_time_ms = 40;
        let mut whole = SignalingEngine::new(policy).unwrap();
        let mut split = SignalingEngine::new(policy).unwrap();
        let pcm = [0.125; 1_920];
        for _ in 0..5 {
            let expected = whole.tick(&pcm, keyed()).unwrap();
            let mut received = Vec::new();
            let mut calibrated = Vec::new();
            let mut actual = None;
            for frames in [5, 1, 7, 5, 942] {
                actual = Some(split.tick(&pcm[..frames * 2], keyed()).unwrap());
                received.extend_from_slice(split.detector_baseband());
                calibrated.extend_from_slice(split.calibration_output());
            }
            let actual = actual.unwrap();
            assert_eq!(actual.qualification, expected.qualification);
            assert_eq!(
                actual.transmit.settle_remaining_ms,
                expected.transmit.settle_remaining_ms
            );
            assert_eq!(actual.transmit.logical_ptt, expected.transmit.logical_ptt);
            assert_eq!(received, whole.detector_baseband());
            assert_eq!(calibrated, whole.calibration_output());
        }
    }

    #[test]
    fn owned_tick_allocates_nothing_across_key_unkey_and_cpu_saver_transitions() {
        let mut policy = config();
        policy.receive_cpu_saver_enabled = true;
        policy.transmit.tx_cpu_saver_enabled = true;
        let mut engine = SignalingEngine::new(policy).unwrap();
        let pcm = [0.0; 1_920];
        let (result, allocations) = allocation_test::count(|| {
            let mut saw_halt = false;
            for index in 0..100 {
                let inputs = if index < 25 {
                    keyed()
                } else {
                    Inputs {
                        tx_render_admitted: true,
                        ..Inputs::default()
                    }
                };
                for frames in [5, 1, 7, 5, 942] {
                    let output = engine.tick(&pcm[..frames * 2], inputs)?;
                    saw_halt |= output.receive_halted;
                }
            }
            Ok::<bool, Error>(saw_halt)
        });
        assert_eq!(allocations, 0);
        assert_eq!(result, Ok(true));
    }

    #[test]
    fn physical_ptt_release_rearms_blanking_before_receive_processing() {
        let mut policy = config();
        policy.transmit.tx_complete.txrx_blanking_time_ms = 7;
        let mut engine = SignalingEngine::new(policy).unwrap();
        let initial = engine.tick(&[0.125; 12], keyed()).unwrap();
        assert!(initial.qualification.rx_keyed);
        let released = Inputs {
            physical_ptt_applied: false,
            tx_render_admitted: false,
            ..keyed()
        };
        let protected = engine.tick(&[0.125; 600], released).unwrap();
        assert_eq!(protected.receive.blanked_native_frames, 300);
        assert!(!protected.qualification.rx_keyed);
        assert!(engine.detector_baseband().iter().all(|value| *value == 0.0));
        assert_eq!(engine.carrier_gates().len(), 300);
        let tail = engine.tick(&[0.125; 72], released).unwrap();
        assert_eq!(tail.receive.blanked_native_frames, 36);
        // A steady physical-PTT snapshot must not continuously rearm the guard.
        let open = engine.tick(&[0.125; 12], released).unwrap();
        assert_eq!(open.receive.blanked_native_frames, 0);
        assert!(open.qualification.rx_keyed);
    }

    #[test]
    fn active_ctcss_and_renderer_intent_match_irregular_callback_partitions() {
        let mut policy = config();
        policy.receive.frontend.noise_coefficients = &[0];
        policy.receive.frontend.noise_squelch = true;
        policy.receive.center_slicer = Some(receive_path::CenterSlicerConfig {
            limit: 625,
            setpoint: 4_900,
            decay_factor: 5,
            trace: false,
        });
        policy.receive.ctcss = Some(receive_path::CtcssConfig {
            tone_mask: 1 << 11,
            relax: false,
        });
        policy.qualification.subaudible_source = SubaudibleSource::Dsp;
        policy.transmit.signal_mode.ctcss_tx_enabled = 1;
        policy
            .transmit
            .signal_mode
            .default_tx_ctcss_frequency_tenths_hz = 1_000;
        policy
            .transmit
            .signal_mode
            .mapped_tx_ctcss_frequency_tenths_hz[11] = 1_000;
        let mut whole = SignalingEngine::new(policy).unwrap();
        let mut split = SignalingEngine::new(policy).unwrap();
        let mut saw_decode = false;
        let mut saw_release = false;
        for block in 0..100 {
            let mut pcm = [0.0_f32; 1_920];
            if block < 70 {
                for (frame, pair) in pcm.chunks_exact_mut(2).enumerate() {
                    let angle = (block * 960 + frame) as f64 * std::f64::consts::TAU / 480.0;
                    pair[0] = angle.sin() as f32 * 0.3;
                }
            }
            let expected = whole.tick(&pcm, keyed()).unwrap();
            let mut cursor = 0;
            let mut actual = None;
            let mut intents = Vec::new();
            for frames in [5, 1, 7, 5, 942] {
                actual = Some(
                    split
                        .tick(&pcm[cursor * 2..(cursor + frames) * 2], keyed())
                        .unwrap(),
                );
                for span in split.render_spans() {
                    for _ in 0..span.frame_count {
                        intents.push((span.logical_ptt, span.ctcss_frequency_tenths_hz));
                    }
                }
                cursor += frames;
            }
            let actual = actual.unwrap();
            assert_eq!(actual.receive.ctcss_decoded, expected.receive.ctcss_decoded);
            assert_eq!(actual.qualification, expected.qualification);
            assert_eq!(actual.transmit.signal_mode, expected.transmit.signal_mode);
            let expected_intents: Vec<_> = whole
                .render_spans()
                .iter()
                .flat_map(|span| {
                    (0..span.frame_count)
                        .map(move |_| (span.logical_ptt, span.ctcss_frequency_tenths_hz))
                })
                .collect();
            assert_eq!(intents, expected_intents);
            saw_decode |= expected.receive.ctcss_decoded == 11;
            saw_release |= block >= 70 && saw_decode && expected.receive.ctcss_decoded == -1;
        }
        assert!(saw_decode, "the real native CTCSS waveform must qualify");
        assert!(saw_release, "silence must release the decoded tone");
    }

    #[test]
    fn status_reports_edges_even_when_the_final_state_returns_to_its_start() {
        let mut previous = Status::default();
        let mut edges = Edges::default();
        Status {
            carrier: true,
            subaudible: true,
            receiver_keyed: true,
            ctcss: 11,
            dcs: true,
            ptt: true,
        }
        .accumulate(&mut previous, &mut edges);
        Status::default().accumulate(&mut previous, &mut edges);
        assert_eq!(previous, Status::default());
        assert_eq!(
            edges,
            Edges {
                carrier: true,
                subaudible: true,
                receiver_keyed: true,
                ctcss: true,
                dcs: true,
                ptt: true,
            }
        );
    }

    #[test]
    fn logical_completion_blanks_only_following_samples_including_the_final_guard_sample() {
        let mut policy = config();
        policy.transmit.tx_complete.txrx_blanking_time_ms = 7;
        let mut engine = SignalingEngine::new(policy).unwrap();
        engine.tick(&[0.125; 2], keyed()).unwrap();
        let unkey = Inputs {
            external_ptt_request: false,
            ..keyed()
        };
        let mut completed = false;
        for _ in 0..20_000 {
            let output = engine.tick(&[0.125; 2], unkey).unwrap();
            if output.transmit.receiver_blanking_arm.is_some() {
                assert_eq!(output.receive.blanked_native_frames, 0);
                assert!(output.qualification.rx_keyed);
                completed = true;
                break;
            }
        }
        assert!(completed, "the admitted transmission must finish");
        for _ in 0..336 {
            let output = engine.tick(&[0.125; 2], unkey).unwrap();
            assert_eq!(output.receive.blanked_native_frames, 1);
            assert!(!output.qualification.rx_keyed);
        }
        let clear = engine.tick(&[0.125; 2], unkey).unwrap();
        assert_eq!(clear.receive.blanked_native_frames, 0);
        assert!(clear.qualification.rx_keyed);
    }

    #[test]
    fn transmit_completion_actions_survive_the_rest_of_a_large_tick() {
        let mut policy = config();
        policy.transmit.tx_complete.txrx_blanking_time_ms = 7;
        let mut engine = SignalingEngine::new(policy).unwrap();
        engine.tick(&[0.0; 1_920], keyed()).unwrap();
        let unkey = Inputs {
            external_ptt_request: false,
            ..keyed()
        };
        let mut completed = false;
        for _ in 0..30 {
            let output = engine.tick(&[0.0; 634], unkey).unwrap();
            if output.transmit.logical_ptt_released {
                assert!(output.edges.ptt);
                assert_eq!(
                    output.transmit.receiver_blanking_arm,
                    Some(transmit_signaling::BlankingArm { duration_ms: 7 })
                );
                assert!(output.transmit.ctcss_status_event);
                completed = true;
                break;
            }
        }
        assert!(completed);
        let steady = engine.tick(&[0.0; 1_920], unkey).unwrap();
        assert!(!steady.transmit.logical_ptt_released && !steady.transmit.ctcss_status_event);
        assert_eq!(steady.transmit.receiver_blanking_arm, None);
    }
}

/// Hardware and control values held stable for one adapter-supplied tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    /// Normal-polarity CM119/HID carrier snapshot.
    pub hardware_carrier: bool,
    /// Normal-polarity parallel-port carrier snapshot.
    pub parallel_carrier: bool,
    /// Normal-polarity CM119/HID CTCSS snapshot.
    pub hardware_subaudible: bool,
    /// Normal-polarity parallel-port CTCSS snapshot.
    pub parallel_subaudible: bool,
    /// Current external request to transmit program audio.
    pub external_ptt_request: bool,
    /// Physical PTT completion published by the hardware worker.
    pub physical_ptt_applied: bool,
    /// A matching DAC span has been admitted for transmitter rendering.
    pub tx_render_admitted: bool,
    /// Transient control inhibiting CTCSS transmission.
    pub ctcss_inhibit: bool,
}

/// Status transitions to publish immediately after the detecting tick.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Edges {
    /// The resolved carrier indication changed.
    pub carrier: bool,
    /// The resolved subaudible indication changed.
    pub subaudible: bool,
    /// Qualified receiver keying changed.
    pub receiver_keyed: bool,
    /// The decoded CTCSS tone changed, including loss of tone.
    pub ctcss: bool,
    /// DCS decoder validity changed.
    pub dcs: bool,
    /// Logical PTT intent changed.
    pub ptt: bool,
}

/// Complete signaling result for one bounded native callback.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Output {
    /// Native detector/measurement results from this span.
    pub receive: receive_path::ProcessResult,
    /// Fully resolved receiver admission; adapters do not repeat this policy.
    pub qualification: receive_qualification::Output,
    /// Final logical PTT/state and deadlines, with one-shot actions accumulated
    /// across this tick. Use `render_spans()` for the complete PCM interval.
    pub transmit: transmit_signaling::Output,
    /// Receiver CPU saver state after its pre-detector decision.
    pub receive_halted: bool,
    /// Signals that changed during this tick, even if they returned to their
    /// starting value. The associated snapshots contain their final states.
    pub edges: Edges,
    /// A periodic status deadline was reached by the end of this tick.
    pub publish_periodic_status: bool,
}

/// Input/setup failures and an unexpected violation of a composed invariant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// An immutable profile or publication interval is invalid.
    InvalidConfiguration,
    /// Empty, oversized, non-stereo, or non-finite native PCM input.
    InvalidInput,
    /// A validated component rejected its owned state.
    ComponentInvariant,
}

/// Preserve the composition boundary's common failure mapping for each primitive.
fn component_invariant<T>(_: T) -> Error {
    Error::ComponentInvariant
}

/// Preserve all historical signaling-mode labels at the qualification boundary.
fn qualification_mode(mode: i32) -> SignalMode {
    match mode {
        signal_mode::MODE_NONE => SignalMode::None,
        signal_mode::MODE_CTCSS => SignalMode::Ctcss,
        signal_mode::MODE_DCS => SignalMode::Dcs,
        _ => SignalMode::Other,
    }
}

/// Last status values used only to derive end-of-tick edge notifications.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Status {
    carrier: bool,
    subaudible: bool,
    receiver_keyed: bool,
    ctcss: i16,
    dcs: bool,
    ptt: bool,
}

impl Default for Status {
    fn default() -> Self {
        Self {
            carrier: false,
            subaudible: false,
            receiver_keyed: false,
            ctcss: -1,
            dcs: false,
            ptt: false,
        }
    }
}

impl Status {
    /// Compare successive snapshots without retaining a callback event queue.
    fn edges_from(self, previous: Self) -> Edges {
        Edges {
            carrier: self.carrier != previous.carrier,
            subaudible: self.subaudible != previous.subaudible,
            receiver_keyed: self.receiver_keyed != previous.receiver_keyed,
            ctcss: self.ctcss != previous.ctcss,
            dcs: self.dcs != previous.dcs,
            ptt: self.ptt != previous.ptt,
        }
    }

    /// Retain all changed signals even if they return to their initial value.
    fn accumulate(self, previous: &mut Self, edges: &mut Edges) {
        let changed = self.edges_from(*previous);
        edges.carrier |= changed.carrier;
        edges.subaudible |= changed.subaudible;
        edges.receiver_keyed |= changed.receiver_keyed;
        edges.ctcss |= changed.ctcss;
        edges.dcs |= changed.dcs;
        edges.ptt |= changed.ptt;
        *previous = self;
    }

    /// Extract the final state of one internal sample step.
    fn from_output(output: &Output) -> Self {
        Self {
            carrier: output.qualification.carrier_active,
            subaudible: output.qualification.subaudible_active,
            receiver_keyed: output.qualification.rx_keyed,
            ctcss: output.receive.ctcss_decoded,
            dcs: output.receive.dcs_valid,
            ptt: output.transmit.logical_ptt,
        }
    }
}

/// Preallocated state for one immutable 48 kHz radio generation.
pub struct SignalingEngine {
    receive: ReceivePath,
    qualification_config: receive_qualification::Config,
    qualification: receive_qualification::State,
    transmit: TransmitSignaling,
    receive_cpu_saver: RxCpuSaverState,
    receive_cpu_saver_enabled: bool,
    ctcss_receive_enabled: bool,
    dcs_receive_enabled: bool,
    receive_blanking_time_ms: i32,
    physical_ptt_applied: bool,
    maximum_native_frames: usize,
    publication_interval_frames: u64,
    publication_remaining_frames: u64,
    previous_status: Status,
    detector_baseband: Vec<f32>,
    calibration: Vec<f32>,
    carrier_gates: Vec<u8>,
    render_spans: Vec<transmit_signaling::RenderSpan>,
}

impl SignalingEngine {
    /// Copy the validated profile and allocate all workspaces off the callback.
    pub fn new(config: Config<'_>) -> Result<Self, Error> {
        if config.publication_interval_ms == 0 {
            return Err(Error::InvalidConfiguration);
        }
        let transmit =
            TransmitSignaling::new(config.transmit).map_err(|_| Error::InvalidConfiguration)?;
        let receive = ReceivePath::new(config.receive).map_err(|_| Error::InvalidConfiguration)?;
        let baseband_capacity = receive.baseband_capacity();
        let publication_interval_frames =
            u64::from(config.publication_interval_ms) * u64::from(timer::FRAMES_PER_MILLISECOND);
        Ok(Self {
            receive,
            qualification_config: config.qualification,
            qualification: receive_qualification::State::default(),
            transmit,
            receive_cpu_saver: RxCpuSaverState::default(),
            receive_cpu_saver_enabled: config.receive_cpu_saver_enabled,
            ctcss_receive_enabled: config.receive.ctcss.is_some(),
            dcs_receive_enabled: config.receive.dcs.is_some(),
            receive_blanking_time_ms: config.transmit.tx_complete.txrx_blanking_time_ms,
            physical_ptt_applied: false,
            maximum_native_frames: config.receive.maximum_native_frames as usize,
            publication_interval_frames,
            publication_remaining_frames: publication_interval_frames,
            previous_status: Status::default(),
            detector_baseband: Vec::with_capacity(baseband_capacity),
            calibration: Vec::with_capacity(baseband_capacity),
            carrier_gates: Vec::with_capacity(config.receive.maximum_native_frames as usize),
            render_spans: Vec::with_capacity(config.receive.maximum_native_frames as usize),
        })
    }

    /// Advance receive, signaling, and source qualification for exactly this span.
    ///
    /// Invalid PCM is rejected before any state changes. Hardware snapshots are
    /// stable during a call. Internal sample-time steps keep RX decisions and TX
    /// actions causal without a second event timeline or an arbitrary 20 ms gate.
    /// All calls are Rust-internal; no per-sample ABI, hardware, or adapter call
    /// occurs. TX freezes when not admitted, but RX/status time still advance.
    pub fn tick(&mut self, pcm: &[f32], inputs: Inputs) -> Result<Output, Error> {
        let frame_count = pcm.len() / 2;
        if pcm.is_empty()
            || pcm.len() % 2 != 0
            || frame_count > self.maximum_native_frames
            || pcm.iter().any(|sample| !sample.is_finite())
        {
            return Err(Error::InvalidInput);
        }
        let frame_count = frame_count as u32;
        self.detector_baseband.clear();
        self.calibration.clear();
        self.carrier_gates.clear();
        self.render_spans.clear();
        let mut output = self.advance_frame(&pcm[..2], inputs)?;
        let mut edges = Edges::default();
        Status::from_output(&output).accumulate(&mut self.previous_status, &mut edges);
        self.collect_frame(&output, 0);
        let mut blanked = output.receive.blanked_native_frames;
        let mut rssi_updated = output.receive.rssi_updated;
        let mut ptt_raised = output.transmit.logical_ptt_raised;
        let mut ptt_released = output.transmit.logical_ptt_released;
        let mut ctcss_status_event = output.transmit.ctcss_status_event;
        let mut blanking_arm = output.transmit.receiver_blanking_arm;
        for (index, frame) in pcm[2..].chunks_exact(2).enumerate() {
            output = self.advance_frame(frame, inputs)?;
            Status::from_output(&output).accumulate(&mut self.previous_status, &mut edges);
            self.collect_frame(&output, index as u32 + 1);
            blanked += output.receive.blanked_native_frames;
            rssi_updated |= output.receive.rssi_updated;
            ptt_raised |= output.transmit.logical_ptt_raised;
            ptt_released |= output.transmit.logical_ptt_released;
            ctcss_status_event |= output.transmit.ctcss_status_event;
            blanking_arm = output.transmit.receiver_blanking_arm.or(blanking_arm);
        }
        output.receive.native_frame_count = frame_count;
        output.receive.baseband_frame_count = self.detector_baseband.len() as u32;
        output.receive.blanked_native_frames = blanked;
        output.receive.rssi_updated = rssi_updated;
        output.transmit.logical_ptt_raised = ptt_raised;
        output.transmit.logical_ptt_released = ptt_released;
        output.transmit.ctcss_status_event = ctcss_status_event;
        output.transmit.receiver_blanking_arm = blanking_arm;
        output.edges = edges;
        output.publish_periodic_status = self.advance_publication(frame_count);
        Ok(output)
    }

    /// Advance one physical sample; the outer tick aggregates without allocation.
    fn advance_frame(&mut self, pcm: &[f32], inputs: Inputs) -> Result<Output, Error> {
        // Staged DAC audio can outlive logical PTT. Rearm the RX guard when the
        // hardware actually releases, not only when the state machine finishes.
        if self.physical_ptt_applied && !inputs.physical_ptt_applied {
            self.receive
                .arm_receive_blanking(self.receive_blanking_time_ms);
        }
        self.physical_ptt_applied = inputs.physical_ptt_applied;
        let previous_tx = self.transmit.snapshot();
        rx_cpu_saver::advance(
            &RxCpuSaverInput {
                enabled: u32::from(self.receive_cpu_saver_enabled),
                carrier_detect: u32::from(self.receive.last_result().carrier_detect),
                signal_mode_null: u32::from(
                    previous_tx.signal_mode.smode == signal_mode::MODE_NONE,
                ),
                tx_ptt_in: u32::from(inputs.external_ptt_request),
                tx_ptt_out: u32::from(previous_tx.logical_ptt),
            },
            &mut self.receive_cpu_saver,
        )
        .map_err(component_invariant)?;
        let receive_halted = self.receive_cpu_saver.halted != 0;
        self.receive.set_voice_processing_active(!receive_halted);
        let receive = self.receive.process(pcm).map_err(component_invariant)?;
        let transmit = self
            .transmit
            .tick(transmit_signaling::Input {
                native_frame_count: 1,
                tx_render_admitted: inputs.tx_render_admitted,
                external_ptt_request: inputs.external_ptt_request,
                physical_ptt_applied: inputs.physical_ptt_applied,
                decoded_ctcss: i32::from(receive.ctcss_decoded),
                dcs_valid: receive.dcs_valid,
                ctcss_inhibit: inputs.ctcss_inhibit,
            })
            .map_err(component_invariant)?;
        if let Some(blanking) = transmit.receiver_blanking_arm {
            self.receive.arm_receive_blanking(blanking.duration_ms);
        }
        let signal_mode = qualification_mode(transmit.signal_mode.smode);
        let qualification = receive_qualification::advance(
            &self.qualification_config,
            &receive_qualification::Inputs {
                hardware_carrier: inputs.hardware_carrier,
                parallel_carrier: inputs.parallel_carrier,
                dsp_carrier: receive.carrier_detect,
                hardware_subaudible: inputs.hardware_subaudible,
                parallel_subaudible: inputs.parallel_subaudible,
                dcs_receive_enabled: self.dcs_receive_enabled,
                dcs_valid: receive.dcs_valid,
                ctcss_receive_enabled: self.ctcss_receive_enabled,
                ctcss_decoder_available: self.ctcss_receive_enabled,
                ctcss_decoded: receive.ctcss_decoded >= 0,
                signal_mode,
                tx_ptt_out: if transmit.render_span_count == 0 {
                    transmit.logical_ptt
                } else {
                    transmit.render_spans[0].logical_ptt
                },
                txrx_blanking_active: receive.blanked_native_frames != 0,
            },
            1,
            &mut self.qualification,
        );
        Ok(Output {
            receive,
            qualification,
            transmit,
            receive_halted,
            edges: Edges::default(),
            publish_periodic_status: false,
        })
    }

    /// Append this sample's bounded diagnostics and coalesce unchanged TX intent.
    fn collect_frame(&mut self, output: &Output, frame_offset: u32) {
        self.detector_baseband
            .extend_from_slice(self.receive.baseband_output());
        self.calibration
            .extend_from_slice(self.receive.voice_output());
        self.carrier_gates
            .extend_from_slice(self.receive.carrier_gates());
        if output.transmit.render_span_count == 0 {
            return;
        }
        let intent = output.transmit.render_spans[0];
        if let Some(previous) = self.render_spans.last_mut() {
            let mut continuing = previous.ctcss_render;
            // A phase action belongs at the first sample only. Extending its
            // span retains that single action rather than repeating it.
            continuing.phase_shift_degrees = 0.0;
            if previous.logical_ptt == intent.logical_ptt
                && previous.ctcss_frequency_tenths_hz == intent.ctcss_frequency_tenths_hz
                && continuing == intent.ctcss_render
                && previous.dcs_turnoff_active == intent.dcs_turnoff_active
            {
                previous.frame_count += 1;
                return;
            }
        }
        self.render_spans.push(transmit_signaling::RenderSpan {
            frame_offset,
            ..intent
        });
    }

    /// Advance the periodic deadline without accumulating callback rounding drift.
    fn advance_publication(&mut self, native_frames: u32) -> bool {
        let frames = u64::from(native_frames);
        if frames < self.publication_remaining_frames {
            self.publication_remaining_frames -= frames;
            return false;
        }
        let overshoot = frames - self.publication_remaining_frames;
        self.publication_remaining_frames =
            self.publication_interval_frames - overshoot % self.publication_interval_frames;
        true
    }

    /// Return detector baseband for diagnostics, never as delivered voice audio.
    #[must_use]
    pub fn detector_baseband(&self) -> &[f32] {
        &self.detector_baseband
    }

    /// Return the native per-frame carrier gate consumed by the voice renderer.
    #[must_use]
    pub fn carrier_gates(&self) -> &[u8] {
        &self.carrier_gates
    }

    /// Return the retained calibration-only receiver output.
    ///
    /// The old voice-calibration facilities still measure this tap. The native
    /// renderer must continue using its own FFmpeg-processed receive audio.
    #[must_use]
    pub fn calibration_output(&self) -> &[f32] {
        &self.calibration
    }

    /// Return contiguous sample-offset transmitter intents covering admitted PCM.
    #[must_use]
    pub fn render_spans(&self) -> &[transmit_signaling::RenderSpan] {
        &self.render_spans
    }
}
