//! Owned native receive path for the whole-session radio engine.
//!
//! This module composes the existing legacy-compatible fixed-point primitives
//! without crossing the C descriptor for each stage.  Construction copies the
//! selected coefficient tables and preallocates every history and PCM span;
//! [`crate::receive_path::ReceivePath::process_with_carrier`] only advances caller-owned
//! stream state.

use super::{
    RADIO_INVALID_ARGUMENT, audio_meter, center_slicer, ctcss_receive, dcs_receive,
    deemphasis_integrator, delay_line, envelope_meter, fir, receive_frontend,
    rx_blanking::{self, RxBlankingInput, RxBlankingState},
    timer,
};

/// Fixed legacy network-rate detector sample rate.
pub const BASE_SAMPLE_RATE_HZ: u32 = 8_000;

/// Native frames in the retained 20 ms legacy delay-gate interval.
///
/// The owned path accepts arbitrary callback partitions, but the historical
/// C delay stage sampled its squelch gate once per 20 ms native block.  Keeping
/// that physical interval preserves its stream behavior independently of a
/// caller's callback boundaries.
const LEGACY_DELAY_GATE_FRAMES: u32 = timer::NATIVE_SAMPLE_RATE_HZ / 50;

/// Fixed receiver configuration for one selected legacy FIR stage.
#[derive(Clone, Copy, Debug)]
pub struct FilterConfig<'a> {
    /// Selected signed-16 coefficient table.
    pub coefficients: &'a [i16],
    /// Historical Q8 input gain.
    pub input_gain: i32,
    /// Historical Q8 output gain.
    pub output_gain: i32,
    /// Historical FIR normalization divisor.
    pub calc_adjust: i32,
}

/// Fixed controls for the native discriminator frontend.
#[derive(Clone, Copy, Debug)]
pub struct FrontendConfig<'a> {
    /// Selected native-to-baseband low-pass coefficient table.
    pub baseband_coefficients: &'a [i16],
    /// Historical native-to-baseband normalization divisor.
    pub baseband_calc_adjust: i32,
    /// Historical Q8 native-to-baseband output gain.
    pub baseband_output_gain: i32,
    /// Selected discriminator-noise coefficient table.
    pub noise_coefficients: &'a [i16],
    /// Historical discriminator-noise normalization divisor.
    pub noise_divisor: i32,
    /// Native frames per emitted baseband sample.
    pub decimate: u32,
    /// Native frames accumulated for one RSSI calibration result.
    pub calibration_window: u32,
    /// Legacy MICOR squelch opening level.
    pub open_level: u32,
    /// Legacy MICOR squelch hysteresis.
    pub hysteresis: u32,
    /// Advance discriminator-noise RSSI and MICOR squelch.
    ///
    /// False retains the historical VOX frontend branch: it still decimates
    /// audio but leaves noise-detector state untouched.
    pub noise_squelch: bool,
}

/// Optional CTCSS center-slicer controls.
#[derive(Clone, Copy, Debug)]
pub struct CenterSlicerConfig {
    /// Legacy signed-PCM limiter magnitude.
    pub limit: i32,
    /// Legacy peak-tracker set point.
    pub setpoint: i16,
    /// Legacy extrema discharge amount per baseband sample.
    pub decay_factor: i32,
    /// Retain the historical alternating extrema diagnostic trace.
    pub trace: bool,
}

/// Fixed controls for the legacy one-pole receiver deemphasis stage.
#[derive(Clone, Copy, Debug)]
pub struct DeemphasisConfig {
    /// Historical feed-forward coefficient.
    pub output_coefficient: i16,
    /// Historical recursive feedback coefficient.
    pub feedback_coefficient: i16,
    /// Historical Q8 output gain.
    pub output_gain: i32,
}

/// Preallocated receiver squelch-delay controls.
#[derive(Clone, Copy, Debug)]
pub struct DelayConfig {
    /// Circular-storage length in baseband samples.
    pub storage_capacity: u32,
    /// Delay lead in baseband samples.
    pub lead: u32,
}

/// Fixed controls for one legacy envelope detector.
#[derive(Clone, Copy, Debug)]
pub struct EnvelopeConfig {
    /// Samples between the historical envelope decay steps.
    pub decay_factor: i32,
    /// Legacy signed-PCM comparator threshold.
    pub threshold: i16,
}

/// VOX detector and carrier-hang configuration.
#[derive(Clone, Copy, Debug)]
pub struct VoxConfig {
    /// Fixed envelope detector controls.
    pub envelope: EnvelopeConfig,
    /// VOX hold duration in milliseconds after the final qualifying sample.
    ///
    /// The owned path converts this once at construction and advances it by
    /// native PCM frames, so callback partitioning cannot extend the hold.
    pub hang_time_ms: i32,
}

/// Selected receive CTCSS detector configuration.
#[derive(Clone, Copy, Debug)]
pub struct CtcssConfig {
    /// Bit mask of configured legacy CTCSS table indices.
    pub tone_mask: u64,
    /// Select relaxed CTCSS talk-off behavior.
    pub relax: bool,
}

/// Selected receive DCS detector configuration.
#[derive(Clone, Copy, Debug)]
pub struct DcsConfig {
    /// Legacy numeric DCS code.
    pub code: i32,
    /// Invert the received DCS symbol polarity.
    pub inverted: bool,
}

/// Setup-only configuration for one owned native receive path.
#[derive(Clone, Copy, Debug)]
pub struct Config<'a> {
    /// Fixed native stream rate for the lifetime of this path.
    pub native_sample_rate_hz: u32,
    /// Largest native frame span accepted by [`ReceivePath::process_with_carrier`].
    pub maximum_native_frames: u32,
    /// Native discriminator frontend controls.
    pub frontend: FrontendConfig<'a>,
    /// CTCSS low-pass FIR controls.
    pub lsd_filter: FilterConfig<'a>,
    /// Receiver voice high-pass FIR controls.
    pub hpf_filter: Option<FilterConfig<'a>>,
    /// CTCSS centering and limiting, enabled with CTCSS detection.
    pub center_slicer: Option<CenterSlicerConfig>,
    /// Optional flat-discriminator receiver deemphasis.
    pub deemphasis: Option<DeemphasisConfig>,
    /// Optional carrier-tail audio delay.
    pub delay: Option<DelayConfig>,
    /// Optional VOX carrier detector.
    pub vox: Option<VoxConfig>,
    /// Optional tuning envelope detector.
    pub measurement: Option<EnvelopeConfig>,
    /// Optional CTCSS detector bank.
    pub ctcss: Option<CtcssConfig>,
    /// Optional DCS detector.
    pub dcs: Option<DcsConfig>,
}

/// Observable result of one native receive span.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ProcessResult {
    /// Native frames consumed from the supplied stereo span.
    pub native_frame_count: u32,
    /// Decimated 8 kHz samples emitted by the native frontend.
    pub baseband_frame_count: u32,
    /// Leading native frames muted by post-transmit blanking.
    pub blanked_native_frames: u32,
    /// Current receiver carrier decision after the configured detector.
    pub carrier_detect: bool,
    /// Current VOX envelope comparator decision.
    pub vox_detect: bool,
    /// Current CTCSS table index, or `-1` when unqualified.
    pub ctcss_decoded: i16,
    /// Whether the configured DCS decoder is qualified.
    pub dcs_valid: bool,
    /// Post-decoder-gain CTCSS half peak-to-peak level as normalized PCM.
    pub ctcss_decoder_peak: f32,
    /// Most recent completed legacy RSSI result.
    pub rssi_peak: i16,
    /// Whether this span completed one fixed RSSI window.
    pub rssi_updated: bool,
}

#[derive(Clone, Copy)]
struct FilterControls {
    input_gain: i32,
    output_gain: i32,
    calc_adjust: i32,
}

#[derive(Clone, Copy)]
struct FrontendControls {
    baseband_calc_adjust: i32,
    baseband_output_gain: i32,
    noise_divisor: i32,
    decimate: u32,
    calibration_window: u32,
    open_level: u32,
    hysteresis: u32,
    noise_squelch: bool,
}

/// Preallocated, callback-owned native receiver pipeline.
///
/// The type is deliberately independent of Asterisk and hardware adapters.
/// Its `f32` input is interleaved native stereo PCM. The detector and tuning
/// stages retain their established fixed-point arithmetic internally, while
/// the component performs no heap allocation during processing and retains
/// every stage state for one stream.
pub struct ReceivePath {
    maximum_native_frames: usize,
    baseband_capacity: usize,
    frontend_controls: FrontendControls,
    lsd_controls: FilterControls,
    hpf_controls: Option<FilterControls>,
    center_config: Option<CenterSlicerConfig>,
    deemphasis_config: Option<DeemphasisConfig>,
    delay_config: Option<DelayConfig>,
    vox_config: Option<VoxConfig>,
    measurement_config: Option<EnvelopeConfig>,
    baseband_coefficients: Vec<i16>,
    noise_coefficients: Vec<i16>,
    lsd_coefficients: Vec<i16>,
    hpf_coefficients: Vec<i16>,
    frontend_history: Vec<i16>,
    lsd_history: Vec<i16>,
    hpf_history: Vec<i16>,
    frontend_state: receive_frontend::State,
    center_state: center_slicer::State,
    deemphasis_state: deemphasis_integrator::State,
    delay_state: delay_line::State,
    vox_envelope_state: envelope_meter::State,
    measurement_state: envelope_meter::State,
    ctcss_state: ctcss_receive::ReceiveState,
    dcs_state: dcs_receive::ReceiveState,
    native_work: Vec<f32>,
    baseband: Vec<f32>,
    carrier_gate: Vec<u8>,
    lsd: Vec<f32>,
    centered: Vec<f32>,
    limited: Vec<f32>,
    hpf: Vec<f32>,
    voice: Vec<f32>,
    delayed: Vec<f32>,
    vox_envelope: Vec<f32>,
    measurement: Vec<f32>,
    center_trace: Vec<f32>,
    delay_storage: Vec<f32>,
    emission_offsets: Vec<u32>,
    blanking_remaining_ms: i32,
    blanking_remainder: u32,
    vox_hang_frames: u32,
    vox_hold_remaining_frames: u32,
    delay_gate_native_remainder: u32,
    delay_outzero: bool,
    voice_processing_active: bool,
    center_trace_phase: u32,
    last_baseband_count: usize,
    last_native_count: usize,
    last_result: ProcessResult,
}

impl ReceivePath {
    /// Construct an owned receiver and preallocate all callback workspaces.
    ///
    /// Coefficient tables are copied here because control-plane configuration
    /// can be released or replaced after stream creation.  The current legacy
    /// pipeline is fixed at 48 kHz native and 8 kHz detector rate.
    pub fn new(config: Config<'_>) -> Result<Self, i32> {
        if config.native_sample_rate_hz != timer::NATIVE_SAMPLE_RATE_HZ
            || config.frontend.decimate == 0
            || config.native_sample_rate_hz % config.frontend.decimate != 0
            || config.native_sample_rate_hz / config.frontend.decimate != BASE_SAMPLE_RATE_HZ
            || config.maximum_native_frames == 0
            || config.frontend.baseband_coefficients.is_empty()
            || config.frontend.noise_squelch
                && (config.frontend.noise_coefficients.is_empty()
                    || config.frontend.noise_coefficients.len()
                        > config.frontend.baseband_coefficients.len())
            || config.frontend.baseband_calc_adjust == 0
            || config.frontend.noise_squelch && config.frontend.noise_divisor == 0
            || config.frontend.calibration_window == 0
            || !valid_filter(config.lsd_filter)
            || config
                .hpf_filter
                .is_some_and(|filter| !valid_filter(filter))
            || (config.ctcss.is_some() != config.center_slicer.is_some())
            || config.hpf_filter.is_none()
                && (config.deemphasis.is_some() || config.delay.is_some())
        {
            return Err(RADIO_INVALID_ARGUMENT);
        }
        if let Some(delay) = config.delay {
            if delay.storage_capacity == 0 || delay.lead > delay.storage_capacity {
                return Err(RADIO_INVALID_ARGUMENT);
            }
        }

        // These u32 counts and their doubled sizes fit the supported 64-bit targets.
        let maximum_native_frames = config.maximum_native_frames as usize;
        let decimate = config.frontend.decimate as usize;
        let two_decimates = decimate * 2;
        let baseband_capacity = (maximum_native_frames + two_decimates - 2) / decimate;
        let native_samples = maximum_native_frames * 2;
        let delay_storage_capacity = config
            .delay
            .map_or(0_usize, |delay| delay.storage_capacity as usize);
        let vox_hang_frames = match config.vox {
            Some(vox) if vox.hang_time_ms > 0 => u32::try_from(
                i64::from(vox.hang_time_ms) * i64::from(timer::FRAMES_PER_MILLISECOND),
            )
            .map_err(|_| RADIO_INVALID_ARGUMENT)?,
            _ => 0,
        };

        let mut ctcss_state = ctcss_receive::ReceiveState::default();
        if let Some(ctcss) = config.ctcss {
            ctcss_state.configure(ctcss.tone_mask, ctcss.relax);
        }
        let mut dcs_state = dcs_receive::ReceiveState::default();
        if let Some(dcs) = config.dcs {
            dcs_state.configure(dcs.code, u32::from(dcs.inverted));
        }

        Ok(Self {
            maximum_native_frames,
            baseband_capacity,
            frontend_controls: FrontendControls {
                baseband_calc_adjust: config.frontend.baseband_calc_adjust,
                baseband_output_gain: config.frontend.baseband_output_gain,
                noise_divisor: config.frontend.noise_divisor,
                decimate: config.frontend.decimate,
                calibration_window: config.frontend.calibration_window,
                open_level: config.frontend.open_level,
                hysteresis: config.frontend.hysteresis,
                noise_squelch: config.frontend.noise_squelch,
            },
            lsd_controls: filter_controls(config.lsd_filter),
            hpf_controls: config.hpf_filter.map(filter_controls),
            center_config: config.center_slicer,
            deemphasis_config: config.deemphasis,
            delay_config: config.delay,
            vox_config: config.vox,
            measurement_config: config.measurement,
            baseband_coefficients: config.frontend.baseband_coefficients.to_vec(),
            noise_coefficients: config.frontend.noise_coefficients.to_vec(),
            lsd_coefficients: config.lsd_filter.coefficients.to_vec(),
            hpf_coefficients: config
                .hpf_filter
                .map_or_else(Vec::new, |filter| filter.coefficients.to_vec()),
            frontend_history: vec![0; config.frontend.baseband_coefficients.len()],
            lsd_history: vec![0; config.lsd_filter.coefficients.len()],
            hpf_history: vec![
                0;
                config
                    .hpf_filter
                    .map_or(0, |filter| filter.coefficients.len())
            ],
            frontend_state: receive_frontend::State {
                decimator: config.frontend.decimate as i16,
                comparator_output: 1,
                ..receive_frontend::State::default()
            },
            center_state: center_slicer::State::default(),
            deemphasis_state: deemphasis_integrator::State::default(),
            delay_state: delay_line::State::default(),
            vox_envelope_state: envelope_meter::State::default(),
            measurement_state: envelope_meter::State::default(),
            ctcss_state,
            dcs_state,
            native_work: vec![0.0; native_samples],
            baseband: vec![0.0; baseband_capacity],
            carrier_gate: vec![0; maximum_native_frames],
            lsd: vec![0.0; baseband_capacity],
            centered: vec![0.0; baseband_capacity],
            limited: vec![0.0; baseband_capacity],
            hpf: vec![0.0; baseband_capacity],
            voice: vec![0.0; baseband_capacity],
            delayed: vec![0.0; baseband_capacity],
            vox_envelope: vec![0.0; baseband_capacity],
            measurement: vec![0.0; baseband_capacity],
            center_trace: vec![0.0; baseband_capacity],
            delay_storage: vec![0.0; delay_storage_capacity],
            emission_offsets: Vec::with_capacity(baseband_capacity),
            blanking_remaining_ms: 0,
            blanking_remainder: 0,
            vox_hang_frames,
            vox_hold_remaining_frames: 0,
            delay_gate_native_remainder: 0,
            delay_outzero: false,
            voice_processing_active: true,
            center_trace_phase: 0,
            last_baseband_count: 0,
            last_native_count: 0,
            last_result: ProcessResult::default(),
        })
    }

    /// Arm post-transmit left-channel receiver blanking in whole milliseconds.
    ///
    /// Arming resets the fractional timing remainder exactly as the current C
    /// transition does.  A nonpositive duration simply leaves no protection.
    pub fn arm_receive_blanking(&mut self, duration_ms: i32) {
        self.blanking_remaining_ms = duration_ms.max(0);
        self.blanking_remainder = 0;
    }

    /// Enable or freeze the voice HPF/deemphasis stages for RX CPU saving.
    ///
    /// The frontend, DCS, and delay remain active. CTCSS acquisition sleeps
    /// while halted, but an already decoded tone continues to be monitored.
    /// This preserves the compatibility CPU-saver acquisition/release policy.
    pub fn set_voice_processing_active(&mut self, active: bool) {
        self.voice_processing_active = active;
    }

    /// Return the conservative baseband workspace capacity allocated at setup.
    #[must_use]
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn baseband_capacity(&self) -> usize {
        self.baseband_capacity
    }

    /// Return the final receiver audio emitted by the latest call.
    #[must_use]
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn voice_output(&self) -> &[f32] {
        let span = self.last_baseband_count;
        if self.hpf_controls.is_none() {
            &[]
        } else if self.delay_config.is_some() {
            &self.delayed[..span]
        } else {
            &self.voice[..span]
        }
    }

    /// Return decimated native-frontend output from the latest call.
    #[must_use]
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn baseband_output(&self) -> &[f32] {
        &self.baseband[..self.last_baseband_count]
    }

    /// Return per-native-frame DSP carrier gates from the latest call.
    #[must_use]
    pub fn carrier_gates(&self) -> &[u8] {
        &self.carrier_gate[..self.last_native_count]
    }

    /// Return the optional center-slicer diagnostic trace from the latest call.
    #[must_use]
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn center_trace(&self) -> Option<&[f32]> {
        self.center_config
            .filter(|config| config.trace)
            .map(|_| &self.center_trace[..self.last_baseband_count])
    }

    /// Return the most recently published receive state.
    #[must_use]
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn last_result(&self) -> ProcessResult {
        self.last_result
    }

    /// Process one bounded native stereo span with no allocation or locking.
    ///
    /// A nonempty span must contain interleaved left/right F32 PCM frames and
    /// cannot exceed the construction-time maximum.  Every native sample
    /// advances blanking, RSSI, MICOR, and DCS timing; the 8 kHz stages run
    /// only for the exact number of decimated samples emitted this call.
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub fn process(&mut self, native_input: &[f32]) -> Result<ProcessResult, i32> {
        self.process_with_carrier(native_input, None)
    }

    /// Qualify CTCSS using external COS when noise/VOX carrier detection is bypassed.
    /// `None` retains the sample-clocked DSP carrier decision.
    pub(crate) fn process_with_carrier(
        &mut self,
        native_input: &[f32],
        external_carrier: Option<bool>,
    ) -> Result<ProcessResult, i32> {
        if native_input.len() % 2 != 0 {
            return Err(RADIO_INVALID_ARGUMENT);
        }
        let native_frame_count = native_input.len() / 2;
        if native_frame_count > self.maximum_native_frames {
            return Err(RADIO_INVALID_ARGUMENT);
        }
        for sample in native_input {
            if !sample.is_finite() {
                return Err(RADIO_INVALID_ARGUMENT);
            }
        }
        if native_frame_count == 0 {
            self.last_baseband_count = 0;
            self.last_native_count = 0;
            self.last_result = ProcessResult {
                ctcss_decoded: self.ctcss_state.decoded(),
                ctcss_decoder_peak: f32::from(self.center_state.peak) / 32_768.0,
                dcs_valid: self.dcs_state.valid(),
                rssi_peak: self.frontend_state.rssi_peak,
                ..ProcessResult::default()
            };
            return Ok(self.last_result);
        }

        let (input, blanked_native_frames) = self.apply_receive_blanking(native_input)?;
        // The accepted frame count cannot exceed the setup-time u32 maximum.
        let native_frame_count_u32 = native_frame_count as u32;
        let decimator_before = self.frontend_state.decimator;
        let frontend = unsafe {
            receive_frontend::process(receive_frontend::Request {
                input,
                baseband_output: self.baseband.as_mut_ptr(),
                baseband_output_capacity: self.baseband_capacity as u32,
                carrier_gate: self.carrier_gate.as_mut_ptr(),
                carrier_gate_capacity: self.carrier_gate.len() as u32,
                native_frame_count: native_frame_count_u32,
                history: self.frontend_history.as_mut_ptr(),
                history_count: self.frontend_history.len() as u32,
                baseband_coefficients: self.baseband_coefficients.as_ptr(),
                baseband_calc_adjust: self.frontend_controls.baseband_calc_adjust,
                baseband_output_gain: self.frontend_controls.baseband_output_gain,
                noise_coefficients: self.noise_coefficients.as_ptr(),
                noise_coefficient_count: self.noise_coefficients.len() as u32,
                noise_divisor: self.frontend_controls.noise_divisor,
                noise_squelch: self.frontend_controls.noise_squelch,
                decimate: self.frontend_controls.decimate,
                calibration_window: self.frontend_controls.calibration_window,
                open_level: self.frontend_controls.open_level,
                hysteresis: self.frontend_controls.hysteresis,
                state: &mut self.frontend_state,
            })
        };
        let frontend = frontend?;
        let baseband_count = frontend.baseband_output_count as usize;
        self.last_native_count = native_frame_count;
        self.last_baseband_count = baseband_count;
        self.record_emission_offsets(decimator_before, native_frame_count_u32);

        let mut vox_detect = false;
        if baseband_count != 0 {
            self.process_baseband(baseband_count)?;
        }
        if let Some(delay) = self.delay_config {
            self.process_delay(delay, baseband_count, native_frame_count_u32)?;
        }
        if baseband_count != 0 {
            if let Some(vox) = self.vox_config {
                let measured = unsafe {
                    envelope_meter::measure(
                        self.baseband.as_ptr(),
                        self.vox_envelope.as_mut_ptr(),
                        baseband_count as u32,
                        vox.envelope.decay_factor,
                        vox.envelope.threshold,
                        &mut self.vox_envelope_state,
                    )
                };
                vox_detect = measured?;
            }
            if let Some(measurement) = self.measurement_config {
                let measured = unsafe {
                    envelope_meter::measure(
                        self.baseband.as_ptr(),
                        self.measurement.as_mut_ptr(),
                        baseband_count as u32,
                        measurement.decay_factor,
                        measurement.threshold,
                        &mut self.measurement_state,
                    )
                };
                let _ = measured?;
            }
        }

        let carrier_detect = if let Some(vox) = self.vox_config {
            self.advance_vox_hold(vox, baseband_count, native_frame_count_u32)?
        } else {
            self.frontend_state.comparator_output == 0
        };

        if self.dcs_state.enabled() {
            let _ = unsafe { self.dcs_state.process(input, native_frame_count_u32) };
        }
        let ctcss_decoded = if baseband_count != 0
            && self.ctcss_state.enabled()
            && (self.voice_processing_active || self.ctcss_state.decoded() != -1)
        {
            unsafe {
                self.ctcss_state.process(
                    self.limited.as_ptr(),
                    baseband_count as u32,
                    external_carrier.unwrap_or(carrier_detect),
                )
            }
        } else {
            self.ctcss_state.decoded()
        };

        self.last_result = ProcessResult {
            native_frame_count: native_frame_count_u32,
            baseband_frame_count: frontend.baseband_output_count,
            blanked_native_frames,
            carrier_detect,
            vox_detect,
            ctcss_decoded,
            dcs_valid: self.dcs_state.valid(),
            ctcss_decoder_peak: f32::from(self.center_state.peak) / 32_768.0,
            rssi_peak: self.frontend_state.rssi_peak,
            rssi_updated: frontend.rssi_updated,
        };
        Ok(self.last_result)
    }

    fn apply_receive_blanking(&mut self, native_input: &[f32]) -> Result<(*const f32, u32), i32> {
        if self.blanking_remaining_ms <= 0 {
            return Ok((native_input.as_ptr(), 0));
        }
        let native_frame_count = (native_input.len() / 2) as u32;
        let remainder_before = self.blanking_remainder;
        let elapsed_ms = timer::elapsed_ms(&mut self.blanking_remainder, native_frame_count);
        let mut state = RxBlankingState {
            remaining_ms: self.blanking_remaining_ms,
            blanked_frame_count: 0,
        };
        rx_blanking::advance(
            &RxBlankingInput {
                elapsed_ms,
                native_frame_count,
                sample_remainder_before: remainder_before,
            },
            &mut state,
        )?;
        self.blanking_remaining_ms = state.remaining_ms;
        self.native_work[..native_input.len()].copy_from_slice(native_input);
        for frame in 0..state.blanked_frame_count as usize {
            self.native_work[frame * 2] = 0.0;
        }
        Ok((self.native_work.as_ptr(), state.blanked_frame_count))
    }

    /// Advance the VOX hold from each emitted detector sample's native offset.
    ///
    /// The old C callback path used the final envelope result and one whole
    /// callback duration. That made the carrier deadline depend on how a
    /// caller split identical PCM. This owned path instead consumes elapsed
    /// native frames before every qualified envelope sample, resets at the
    /// actual sample offset, then consumes the trailing frames. It therefore
    /// holds for the configured duration after the final qualifying sample.
    fn advance_vox_hold(
        &mut self,
        vox: VoxConfig,
        sample_count: usize,
        native_frame_count: u32,
    ) -> Result<bool, i32> {
        // record_emission_offsets uses the frontend's identical decimator and
        // records exactly one increasing in-span offset per emitted sample.
        let mut prior_offset = 0_u32;
        for index in 0..sample_count {
            let offset = self.emission_offsets[index];
            let elapsed = offset - prior_offset;
            self.vox_hold_remaining_frames = self.vox_hold_remaining_frames.saturating_sub(elapsed);

            let peak = audio_meter::f32_to_pcm_code(self.vox_envelope[index])?;
            if peak >= i32::from(vox.envelope.threshold) {
                self.vox_hold_remaining_frames = self.vox_hang_frames;
            }
            prior_offset = offset;
        }
        let trailing = native_frame_count - prior_offset;
        self.vox_hold_remaining_frames = self.vox_hold_remaining_frames.saturating_sub(trailing);
        Ok(self.vox_hold_remaining_frames != 0)
    }

    fn process_baseband(&mut self, sample_count: usize) -> Result<(), i32> {
        if self.ctcss_state.enabled() {
            let filtered = unsafe {
                fir::process(fir::Request {
                    input: self.baseband.as_ptr(),
                    output: self.lsd.as_mut_ptr(),
                    sample_count: sample_count as u32,
                    history: self.lsd_history.as_mut_ptr(),
                    history_count: self.lsd_history.len() as u32,
                    coefficients: self.lsd_coefficients.as_ptr(),
                    input_gain: self.lsd_controls.input_gain,
                    output_gain: self.lsd_controls.output_gain,
                    calc_adjust: self.lsd_controls.calc_adjust,
                })
            };
            filtered?;
            let center = self.center_config.ok_or(RADIO_INVALID_ARGUMENT)?;
            self.process_center_slicer(center, sample_count)?;
        }

        if let Some(hpf_controls) = self.hpf_controls.filter(|_| self.voice_processing_active) {
            let filtered = unsafe {
                fir::process(fir::Request {
                    input: self.baseband.as_ptr(),
                    output: self.hpf.as_mut_ptr(),
                    sample_count: sample_count as u32,
                    history: self.hpf_history.as_mut_ptr(),
                    history_count: self.hpf_history.len() as u32,
                    coefficients: self.hpf_coefficients.as_ptr(),
                    input_gain: hpf_controls.input_gain,
                    output_gain: hpf_controls.output_gain,
                    calc_adjust: hpf_controls.calc_adjust,
                })
            };
            filtered?;
            if let Some(deemphasis) = self.deemphasis_config {
                let integrated = unsafe {
                    deemphasis_integrator::process(deemphasis_integrator::Request {
                        input: self.hpf.as_ptr(),
                        output: self.voice.as_mut_ptr(),
                        sample_count: sample_count as u32,
                        output_coefficient: deemphasis.output_coefficient,
                        feedback_coefficient: deemphasis.feedback_coefficient,
                        output_gain: deemphasis.output_gain,
                        state: &mut self.deemphasis_state,
                    })
                };
                integrated?;
            } else {
                self.voice[..sample_count].copy_from_slice(&self.hpf[..sample_count]);
            }
        }

        Ok(())
    }

    fn record_emission_offsets(&mut self, initial_decimator: i16, native_frame_count: u32) {
        let mut decimator = i32::from(initial_decimator);

        self.emission_offsets.clear();
        for offset in 1..=native_frame_count {
            decimator = decimator.wrapping_sub(1);
            if decimator <= 0 {
                decimator = self.frontend_controls.decimate as i32;
                self.emission_offsets.push(offset);
            }
        }
    }

    fn process_delay(
        &mut self,
        delay: DelayConfig,
        sample_count: usize,
        native_frame_count: u32,
    ) -> Result<(), i32> {
        if self.vox_config.is_some() {
            self.process_delay_segment(delay, 0, sample_count)?;
            return Ok(());
        }

        let mut sample_start = 0_usize;
        let mut boundary =
            LEGACY_DELAY_GATE_FRAMES.saturating_sub(self.delay_gate_native_remainder);
        // The retained remainder is always below the fixed gate interval.
        while boundary <= native_frame_count {
            let sample_end = self
                .emission_offsets
                .partition_point(|offset| *offset <= boundary);
            self.process_delay_segment(delay, sample_start, sample_end)?;
            sample_start = sample_end;
            self.delay_outzero = self.carrier_gate[boundary as usize - 1] == 0;
            boundary = boundary.saturating_add(LEGACY_DELAY_GATE_FRAMES);
        }
        self.process_delay_segment(delay, sample_start, sample_count)?;
        self.delay_gate_native_remainder = self
            .delay_gate_native_remainder
            .wrapping_add(native_frame_count)
            % LEGACY_DELAY_GATE_FRAMES;
        Ok(())
    }

    fn process_delay_segment(
        &mut self,
        delay: DelayConfig,
        start: usize,
        end: usize,
    ) -> Result<(), i32> {
        // Only process_delay supplies these ordered, bounded sample offsets.
        let sample_count = end - start;
        if sample_count == 0 {
            return Ok(());
        }
        let delay_input = if self.deemphasis_config.is_some() {
            self.voice.as_ptr()
        } else {
            self.hpf.as_ptr()
        };
        unsafe {
            delay_line::process(delay_line::Request {
                input: delay_input.add(start),
                output: self.delayed.as_mut_ptr().add(start),
                sample_count: sample_count as u32,
                storage: self.delay_storage.as_mut_ptr(),
                storage_capacity: delay.storage_capacity,
                lead: delay.lead,
                state: &mut self.delay_state,
                enabled: true,
                outzero: self.delay_outzero,
            })
        }
    }

    fn process_center_slicer(
        &mut self,
        center: CenterSlicerConfig,
        sample_count: usize,
    ) -> Result<(), i32> {
        // The primitive's legacy extrema state has two untouched counters.
        // Retain a separate preallocated phase in this path rather than
        // repurposing either compatibility state field.
        let trace = if center.trace {
            Some(center_slicer::Trace {
                output: self.center_trace.as_mut_ptr(),
                phase: &mut self.center_trace_phase,
            })
        } else {
            None
        };
        unsafe {
            center_slicer::process_with_trace(
                center_slicer::Request {
                    input: self.lsd.as_ptr(),
                    centered_output: self.centered.as_mut_ptr(),
                    limited_output: self.limited.as_mut_ptr(),
                    sample_count: sample_count as u32,
                    limit: center.limit,
                    setpoint: center.setpoint,
                    decay_factor: center.decay_factor,
                    state: &mut self.center_state,
                },
                trace,
            )
        }
    }
}

fn valid_filter(config: FilterConfig<'_>) -> bool {
    !config.coefficients.is_empty() && config.calc_adjust != 0
}

fn filter_controls(config: FilterConfig<'_>) -> FilterControls {
    FilterControls {
        input_gain: config.input_gain,
        output_gain: config.output_gain,
        calc_adjust: config.calc_adjust,
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{
        BASE_SAMPLE_RATE_HZ, CenterSlicerConfig, Config, CtcssConfig, DeemphasisConfig,
        DelayConfig, EnvelopeConfig, FilterConfig, FrontendConfig, ReceivePath, VoxConfig,
    };

    // These controls are the deployed C stage scales: Q8 unity gain is 256,
    // native-to-network decimation is six, and flat-discriminator deemphasis
    // uses the 6,878/25,889 legacy integrator pair.  Small test FIR tables
    // make the composed path's exact routing and partition state observable;
    // the C-oracle fixture exercises the full selected production tables.
    const FRONTEND: [i16; 4] = [16_384, 8_192, -4_096, 2_048];
    const NOISE: [i16; 3] = [4_096, -2_048, 1_024];
    const LSD: [i16; 3] = [16_384, 8_192, -4_096];
    const HPF: [i16; 4] = [32_767, -1_024, 512, -256];

    // The selected production filter tables used by the ignored C oracle
    // generator.  Keeping one full oracle vector here detects accidental
    // differences in composed primitive routing without importing C fixtures
    // into the published Rust crate.
    const DEPLOYED_NOISE: [i16; 66] = [
        139, -182, -269, -66, 56, 59, 250, 395, -80, -775, -557, 437, 779, 210, -17, 123, -692,
        -1_664, -256, 2_495, 2_237, -1_018, -2_133, -478, -1_134, -2_711, 2_642, 10_453, 4_010,
        -14_385, -16_488, 6_954, 23_030, 6_954, -16_488, -14_385, 4_010, 10_453, 2_642, -2_711,
        -1_134, -478, -2_133, -1_018, 2_237, 2_495, -256, -1_664, -692, 123, -17, 210, 779, 437,
        -557, -775, -80, 395, 250, 59, 56, -66, -269, -182, 139, 257,
    ];
    const DEPLOYED_NOISE_ALT: [i16; 66] = [
        581, -251, -1_027, -766, 63, 346, 148, 459, 1_165, 847, -824, -1_994, -1_147, 462, 704, 32,
        651, 2_277, 1_790, -1_635, -4_071, -2_240, 1_060, 1_127, -502, 1_963, 7_399, 5_862, -6_693,
        -17_483, -10_387, 10_549, 22_110, 10_549, -10_387, -17_483, -6_693, 5_862, 7_399, 1_963,
        -502, 1_127, 1_060, -2_240, -4_071, -1_635, 1_790, 2_277, 651, 32, 704, 462, -1_147,
        -1_994, -824, 847, 1_165, 459, 148, 346, 63, -766, -1_027, -251, 581, 537,
    ];
    const DEPLOYED_RXLPF: [i16; 66] = [
        259, 58, -185, -437, -654, -793, -815, -696, -434, -48, 414, 886, 1_284, 1_523, 1_529,
        1_254, 691, -117, -1_078, -2_049, -2_854, -3_303, -3_220, -2_472, -995, 1_187, 3_952,
        7_086, 10_300, 13_270, 15_672, 17_236, 17_778, 17_236, 15_672, 13_270, 10_300, 7_086,
        3_952, 1_187, -995, -2_472, -3_220, -3_303, -2_854, -2_049, -1_078, -117, 691, 1_254,
        1_529, 1_523, 1_284, 886, 414, -48, -434, -696, -815, -793, -654, -437, -185, 58, 259, 393,
    ];
    const DEPLOYED_RXHPF: [i16; 66] = [
        -141, -114, -77, -30, 23, 83, 147, 210, 271, 324, 367, 396, 407, 396, 362, 302, 216, 102,
        -36, -199, -383, -585, -798, -1_017, -1_237, -1_452, -1_653, -1_836, -1_995, -2_124,
        -2_219, -2_278, 30_463, -2_278, -2_219, -2_124, -1_995, -1_836, -1_653, -1_452, -1_237,
        -1_017, -798, -585, -383, -199, -36, 102, 216, 302, 362, 396, 407, 396, 367, 324, 271, 210,
        147, 83, 23, -30, -77, -114, -141, -158,
    ];
    const VOX_IDENTITY: [i16; 1] = [32_767];

    fn configuration(maximum_native_frames: u32) -> Config<'static> {
        Config {
            native_sample_rate_hz: 48_000,
            maximum_native_frames,
            frontend: FrontendConfig {
                baseband_coefficients: &FRONTEND,
                baseband_calc_adjust: 16_384,
                baseband_output_gain: 256,
                noise_coefficients: &NOISE,
                noise_divisor: 4_096,
                decimate: 6,
                calibration_window: 960,
                open_level: 1,
                hysteresis: 0,
                noise_squelch: true,
            },
            lsd_filter: FilterConfig {
                coefficients: &LSD,
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 16_384,
            },
            hpf_filter: Some(FilterConfig {
                coefficients: &HPF,
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 32_767,
            }),
            center_slicer: None,
            deemphasis: Some(DeemphasisConfig {
                output_coefficient: 6_878,
                feedback_coefficient: 25_889,
                output_gain: 256,
            }),
            delay: Some(DelayConfig {
                storage_capacity: 64,
                lead: 5,
            }),
            vox: None,
            measurement: Some(EnvelopeConfig {
                decay_factor: 10,
                threshold: 0,
            }),
            ctcss: None,
            dcs: None,
        }
    }

    fn stereo(frames: usize) -> Vec<f32> {
        let mut output = Vec::with_capacity(frames * 2);
        for index in 0..frames {
            let code = ((index as i32 * 5_341 + 12_345) % 60_000 - 30_000) as i16;
            output.push(f32::from(code) / 32_768.0);
            output.push(-0.25);
        }
        output
    }

    #[test]
    fn constructor_rejects_each_invalid_frontend_and_detector_configuration() {
        let cases: &[fn(&mut Config<'_>)] = &[
            |c| c.native_sample_rate_hz = 8_000,
            |c| c.frontend.decimate = 0,
            |c| c.frontend.decimate = 7,
            |c| c.frontend.decimate = 8,
            |c| c.maximum_native_frames = 0,
            |c| c.frontend.baseband_coefficients = &[],
            |c| c.frontend.noise_coefficients = &[],
            |c| c.frontend.noise_coefficients = &[1; 5],
            |c| c.frontend.baseband_calc_adjust = 0,
            |c| c.frontend.noise_divisor = 0,
            |c| c.frontend.calibration_window = 0,
            |c| c.lsd_filter.coefficients = &[],
            |c| c.lsd_filter.calc_adjust = 0,
            |c| c.hpf_filter.as_mut().unwrap().calc_adjust = 0,
            |c| c.hpf_filter = None,
            |c| {
                c.hpf_filter = None;
                c.deemphasis = None;
            },
            |c| {
                c.ctcss = Some(CtcssConfig {
                    tone_mask: 1,
                    relax: false,
                })
            },
            |c| c.delay.as_mut().unwrap().storage_capacity = 0,
            |c| c.delay.as_mut().unwrap().lead = 65,
            |c| {
                c.vox = Some(VoxConfig {
                    envelope: EnvelopeConfig {
                        decay_factor: 1,
                        threshold: 0,
                    },
                    hang_time_ms: i32::MAX,
                })
            },
        ];
        for configure in cases {
            let mut config = configuration(32);
            configure(&mut config);
            assert!(ReceivePath::new(config).is_err());
        }
        let mut native_only = configuration(32);
        native_only.hpf_filter = None;
        native_only.deemphasis = None;
        native_only.delay = None;
        assert!(ReceivePath::new(native_only).is_ok());
    }

    #[test]
    fn input_rejection_empty_spans_and_dcs_setup_preserve_public_results() {
        let mut config = configuration(32);
        config.dcs = Some(super::DcsConfig {
            code: 23,
            inverted: true,
        });
        config.vox = Some(VoxConfig {
            envelope: EnvelopeConfig {
                decay_factor: 1,
                threshold: 0,
            },
            hang_time_ms: 0,
        });
        let mut path = ReceivePath::new(config).expect("valid DCS path");
        let before = path.last_result();
        for input in [&[0.0][..], &[0.0; 66][..], &[f32::NAN, 0.0][..]] {
            assert_eq!(path.process(input), Err(crate::RADIO_INVALID_ARGUMENT));
            assert_eq!(path.last_result(), before);
        }
        let active = path.process(&stereo(32)).expect("enabled DCS processing");
        assert_eq!(active.native_frame_count, 32);
        let empty = path.process(&[]).expect("empty processing");
        assert_eq!(empty.native_frame_count, 0);
        assert!(path.voice_output().is_empty());
        assert!(path.carrier_gates().is_empty());
    }

    #[test]
    fn cpu_saver_suspends_ctcss_acquisition_but_monitors_a_qualified_tone() {
        let mut config = configuration(6);
        config.frontend.noise_squelch = false;
        config.ctcss = Some(CtcssConfig {
            tone_mask: 1,
            relax: false,
        });
        config.center_slicer = Some(CenterSlicerConfig {
            limit: 16_384,
            setpoint: 8_192,
            decay_factor: 1,
            trace: false,
        });
        let mut path = ReceivePath::new(config).expect("valid CTCSS path");
        path.frontend_state.comparator_output = 0;
        path.ctcss_state.test_prepare_detector(0);
        path.center_state.peak = 2_400;
        assert_eq!(
            path.process(&[]).unwrap().ctcss_decoder_peak,
            2_400.0 / 32_768.0
        );
        path.set_voice_processing_active(false);
        let silence = [0.0; 12];
        let sleeping = path.process(&silence).expect("sleeping receiver");
        assert!(sleeping.carrier_detect);
        assert_eq!(sleeping.ctcss_decoded, -1);
        assert_eq!(path.ctcss_state.test_detector_decode(0), 0);

        path.set_voice_processing_active(true);
        assert_eq!(path.process(&silence).unwrap().ctcss_decoded, 0);
        path.set_voice_processing_active(false);
        path.ctcss_state.test_set_detector_release(0);
        assert_eq!(path.process(&silence).unwrap().ctcss_decoded, -1);
        assert_eq!(path.ctcss_state.test_blanking_samples(), 1_600);
    }

    fn deployed_oracle_configuration(variant: u8) -> Config<'static> {
        assert!(variant < 16);
        let speaker_audio = variant & 1 != 0;
        let vox_carrier = variant & 2 != 0;
        let noise_coefficients = if variant & 4 != 0 {
            &DEPLOYED_NOISE_ALT
        } else {
            &DEPLOYED_NOISE
        };
        let delay_ms = if variant & 8 != 0 {
            40
        } else if speaker_audio {
            // The C compatibility path retains a 30 ms delay whenever the
            // speaker-audio profile omits flat-discriminator deemphasis.
            30
        } else {
            0
        };
        Config {
            native_sample_rate_hz: 48_000,
            maximum_native_frames: 960,
            frontend: FrontendConfig {
                baseband_coefficients: &DEPLOYED_RXLPF,
                baseband_calc_adjust: 131_072,
                baseband_output_gain: 256,
                noise_coefficients,
                noise_divisor: 65_536,
                decimate: 6,
                calibration_window: 960,
                open_level: (30 * 32_767) / 100,
                hysteresis: 2_500,
                noise_squelch: !vox_carrier,
            },
            lsd_filter: FilterConfig {
                coefficients: &LSD,
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 16_384,
            },
            hpf_filter: Some(FilterConfig {
                coefficients: &DEPLOYED_RXHPF,
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 32_768,
            }),
            center_slicer: None,
            deemphasis: if speaker_audio {
                None
            } else {
                Some(DeemphasisConfig {
                    output_coefficient: 6_878,
                    feedback_coefficient: 25_889,
                    output_gain: 256,
                })
            },
            delay: if delay_ms == 0 {
                None
            } else {
                Some(DelayConfig {
                    storage_capacity: 4_096,
                    lead: delay_ms * 8,
                })
            },
            vox: if vox_carrier {
                Some(VoxConfig {
                    envelope: EnvelopeConfig {
                        decay_factor: 3,
                        threshold: 1_000,
                    },
                    hang_time_ms: 120,
                })
            } else {
                None
            },
            measurement: None,
            ctcss: None,
            dcs: None,
        }
    }

    fn deployed_oracle_input() -> Vec<f32> {
        let mut random = 0x4a83_76b9_u32;
        let mut output = Vec::with_capacity(48 * 960 * 2);
        for block in 0..48_i32 {
            for frame in 0..960_i32 {
                random = random.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let noise = ((random >> 16) as i32) - 32_768;
                let ramp = (block * 960 + frame) % 480 - 240;
                let left = if !(8..40).contains(&block) {
                    0
                } else {
                    noise / 8 + ramp * 12
                } as i16;
                let right = (noise / 4) as i16;
                output.push(f32::from(left) / 32_768.0);
                output.push(f32::from(right) / 32_768.0);
            }
        }
        output
    }

    fn codes(samples: &[f32]) -> Vec<i16> {
        samples
            .iter()
            .map(|sample| (sample * 32_768.0) as i16)
            .collect()
    }

    fn fnv1a_i16_codes(samples: &[i16]) -> u64 {
        let mut hash = 0xcbf2_9ce4_8422_2325_u64;
        for sample in samples {
            for byte in sample.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
        hash
    }

    fn vox_timing_configuration() -> Config<'static> {
        Config {
            native_sample_rate_hz: 48_000,
            maximum_native_frames: 960,
            frontend: FrontendConfig {
                baseband_coefficients: &VOX_IDENTITY,
                baseband_calc_adjust: 32_767,
                baseband_output_gain: 256,
                noise_coefficients: &[],
                noise_divisor: 0,
                decimate: 6,
                calibration_window: 960,
                open_level: 0,
                hysteresis: 0,
                noise_squelch: false,
            },
            lsd_filter: FilterConfig {
                coefficients: &VOX_IDENTITY,
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 32_767,
            },
            hpf_filter: Some(FilterConfig {
                coefficients: &VOX_IDENTITY,
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 32_767,
            }),
            center_slicer: None,
            deemphasis: None,
            delay: None,
            vox: Some(VoxConfig {
                envelope: EnvelopeConfig {
                    decay_factor: 1,
                    threshold: 9_000,
                },
                hang_time_ms: 120,
            }),
            measurement: None,
            ctcss: None,
            dcs: None,
        }
    }

    fn vox_pulse_input(frames: usize) -> Vec<f32> {
        let mut input = vec![0.0_f32; frames * 2];
        for frame in 0..6 {
            input[frame * 2] = 30_000.0 / 32_768.0;
        }
        for frame in 6..12 {
            input[frame * 2] = -30_000.0 / 32_768.0;
        }
        input
    }

    fn last_qualifying_vox_hit(path: &ReceivePath, start_frame: usize) -> Option<usize> {
        path.vox_envelope[..path.last_baseband_count]
            .iter()
            .zip(path.emission_offsets.iter())
            .filter_map(|(peak, offset)| {
                ((peak * 32_768.0) as i32 >= 9_000).then_some(start_frame + *offset as usize)
            })
            .last()
    }

    #[derive(Debug, PartialEq, Eq)]
    struct Capture {
        baseband: Vec<i16>,
        voice: Vec<i16>,
        gates: Vec<u8>,
        center_trace: Option<Vec<i16>>,
        status: (bool, bool, i16, bool, i16),
    }

    fn process_partitions(path: &mut ReceivePath, input: &[f32], partitions: &[usize]) -> Capture {
        let mut offset = 0_usize;
        let mut baseband = Vec::new();
        let mut voice = Vec::new();
        let mut gates = Vec::new();
        let mut center_trace = path.center_trace().map(|_| Vec::new());
        let mut last = path.last_result();

        for frames in partitions {
            let end = offset + frames * 2;
            last = path
                .process(&input[offset..end])
                .expect("valid receive span");
            baseband.extend(codes(path.baseband_output()));
            voice.extend(codes(path.voice_output()));
            gates.extend_from_slice(path.carrier_gates());
            if let (Some(trace), Some(output)) = (&mut center_trace, path.center_trace()) {
                trace.extend(codes(output));
            }
            offset = end;
        }
        assert_eq!(offset, input.len());
        Capture {
            baseband,
            voice,
            gates,
            center_trace,
            status: (
                last.carrier_detect,
                last.vox_detect,
                last.ctcss_decoded,
                last.dcs_valid,
                last.rssi_peak,
            ),
        }
    }

    fn process_deployed_oracle(path: &mut ReceivePath, partitions: &[usize]) -> Capture {
        assert_eq!(partitions.iter().sum::<usize>(), 960);
        let input = deployed_oracle_input();
        let mut offset = 0_usize;
        let mut baseband = Vec::new();
        let mut voice = Vec::new();
        let mut gates = Vec::new();
        let mut last = path.last_result();

        for block in 0..48 {
            if block == 20 {
                // This is the C-oracle's deterministic 7 ms post-TX blanking
                // transition. It covers 336 native frames without resetting
                // frontend state or changing callback partition behavior.
                path.arm_receive_blanking(7);
            }
            for frames in partitions {
                let end = offset + frames * 2;
                last = path
                    .process(&input[offset..end])
                    .expect("oracle receive span");
                baseband.extend(codes(path.baseband_output()));
                voice.extend(codes(path.voice_output()));
                gates.extend_from_slice(path.carrier_gates());
                offset = end;
            }
        }
        assert_eq!(offset, input.len());
        Capture {
            baseband,
            voice,
            gates,
            center_trace: None,
            status: (
                last.carrier_detect,
                last.vox_detect,
                last.ctcss_decoded,
                last.dcs_valid,
                last.rssi_peak,
            ),
        }
    }

    #[test]
    fn deployed_native_and_base_rates_remain_fixed() {
        assert_eq!(BASE_SAMPLE_RATE_HZ, 8_000);
        let path = ReceivePath::new(configuration(960)).expect("valid deployed setup");

        // Mirrors `urp_radio_create`: max 960 native frames reserve 161
        // baseband slots so a carried decimator phase never overruns a
        // callback-owned workspace.
        assert_eq!(path.baseband_capacity(), 161);
    }

    #[test]
    fn partitioning_preserves_all_active_detector_and_voice_output() {
        let input = stereo(960);
        let mut whole = ReceivePath::new(configuration(960)).expect("whole path");
        let expected = process_partitions(&mut whole, &input, &[960]);

        for partitions in [&[5, 1, 6][..], &[1, 5, 6][..], &[7, 5][..]] {
            let short = stereo(12);
            let mut short_whole = ReceivePath::new(configuration(960)).expect("short whole");
            let short_expected = process_partitions(&mut short_whole, &short, &[12]);
            let mut split = ReceivePath::new(configuration(960)).expect("split path");
            assert_eq!(
                process_partitions(&mut split, &short, partitions),
                short_expected,
                "partitions {partitions:?}"
            );
        }

        let mut irregular = ReceivePath::new(configuration(960)).expect("irregular path");
        assert_eq!(
            process_partitions(&mut irregular, &input, &[5, 1, 7, 5, 942]),
            expected
        );
    }

    #[test]
    fn incomplete_decimation_interval_emits_no_baseband_samples() {
        let input = stereo(5);
        let mut path = ReceivePath::new(configuration(960)).expect("valid path");
        let result = path.process(&input).expect("five native frames");

        assert_eq!(result.baseband_frame_count, 0);
        assert!(path.baseband_output().is_empty());
        assert!(path.voice_output().is_empty());
        assert_eq!(path.carrier_gates().len(), 5);
    }

    #[test]
    fn maximum_callback_after_a_carried_phase_stays_within_preallocated_capacity() {
        let mut path = ReceivePath::new(configuration(960)).expect("valid path");
        let first = stereo(1);
        let second = stereo(960);

        let _ = path.process(&first).expect("carried phase seed");
        let result = path.process(&second).expect("maximum callback");
        assert!(result.baseband_frame_count as usize <= path.baseband_capacity());
        assert_eq!(path.baseband_capacity(), 161);
    }

    #[test]
    fn blanking_preserves_the_contiguous_legacy_prefix_across_callbacks() {
        let mut path = ReceivePath::new(configuration(960)).expect("valid path");
        path.arm_receive_blanking(1);
        let first = path.process(&stereo(36)).expect("first protected span");
        let second = path.process(&stereo(36)).expect("final protected span");

        assert_eq!(first.blanked_native_frames, 36);
        assert_eq!(second.blanked_native_frames, 12);
    }

    #[test]
    fn vox_bypasses_noise_measurement_but_retains_decimation_and_hang() {
        let mut config = configuration(960);
        config.frontend.noise_squelch = false;
        config.vox = Some(VoxConfig {
            envelope: EnvelopeConfig {
                decay_factor: 3,
                threshold: 1,
            },
            hang_time_ms: 40,
        });
        let mut path = ReceivePath::new(config).expect("VOX path");
        let result = path.process(&stereo(6)).expect("VOX span");

        assert_eq!(result.rssi_peak, 0);
        assert!(result.vox_detect);
        assert!(result.carrier_detect);
    }

    #[test]
    fn vox_hold_expires_at_the_same_native_sample_after_irregular_partitions() {
        const INITIAL_BLOCK: usize = 960;
        const HANG_NATIVE_FRAMES: usize = 120 * 48;
        let input = vox_pulse_input(12_000);
        let mut whole = ReceivePath::new(vox_timing_configuration()).expect("whole VOX path");
        let whole_result = whole
            .process(&input[..INITIAL_BLOCK * 2])
            .expect("whole initial block");
        let mut whole_last_hit = last_qualifying_vox_hit(&whole, 0);

        let mut split = ReceivePath::new(vox_timing_configuration()).expect("split VOX path");
        let mut split_offset = 0_usize;
        let mut split_last_hit = None;
        let mut split_result = split.last_result();
        for frames in [5_usize, 1, 7, 5, 942] {
            let end = split_offset + frames * 2;
            split_result = split
                .process(&input[split_offset..end])
                .expect("split initial block");
            if let Some(hit) = last_qualifying_vox_hit(&split, split_offset / 2) {
                split_last_hit = Some(hit);
            }
            split_offset = end;
        }

        assert_eq!(split_offset, INITIAL_BLOCK * 2);
        assert_eq!(whole_result.carrier_detect, split_result.carrier_detect);
        assert_eq!(whole_last_hit, split_last_hit);
        assert_eq!(
            whole.vox_hold_remaining_frames,
            split.vox_hold_remaining_frames
        );

        let mut whole_drop = None;
        let mut split_drop = None;
        for frame in INITIAL_BLOCK..12_000 {
            let start = frame * 2;
            let end = start + 2;
            let whole_result = whole.process(&input[start..end]).expect("whole tail frame");
            let split_result = split.process(&input[start..end]).expect("split tail frame");
            if let Some(hit) = last_qualifying_vox_hit(&whole, frame) {
                whole_last_hit = Some(hit);
            }
            if let Some(hit) = last_qualifying_vox_hit(&split, frame) {
                split_last_hit = Some(hit);
            }
            if !whole_result.carrier_detect && whole_drop.is_none() {
                whole_drop = Some(frame + 1);
            }
            if !split_result.carrier_detect && split_drop.is_none() {
                split_drop = Some(frame + 1);
            }
            if whole_drop.is_some() && split_drop.is_some() {
                break;
            }
        }

        let whole_last_hit = whole_last_hit.expect("qualifying VOX envelope sample");
        assert_eq!(Some(whole_last_hit), split_last_hit);
        assert_eq!(whole_drop, Some(whole_last_hit + HANG_NATIVE_FRAMES));
        assert_eq!(split_drop, whole_drop);
    }

    #[test]
    fn ctcss_center_trace_is_preallocated_and_partition_invariant() {
        let mut config = configuration(960);
        config.center_slicer = Some(CenterSlicerConfig {
            limit: 625,
            setpoint: 4_900,
            decay_factor: 5,
            trace: true,
        });
        config.ctcss = Some(CtcssConfig {
            tone_mask: 1,
            relax: false,
        });
        let input = stereo(24);
        let mut whole = ReceivePath::new(config).expect("whole trace path");
        let expected = process_partitions(&mut whole, &input, &[24]);
        let mut split = ReceivePath::new(config).expect("split trace path");
        let actual = process_partitions(&mut split, &input, &[5, 1, 7, 5, 6]);
        assert_eq!(actual, expected);
    }

    #[test]
    fn production_receive_profiles_match_c_oracle_and_keep_partition_timing() {
        // Generated by USBRadioPlus `.work/export-receive-oracle.c` from the
        // active C pipeline. Its 16 profiles cover flat/speaker audio,
        // noise/VOX carrier, both noise tables, and no/40 ms delay. Every
        // profile has deployed rxlpf/rxhpf controls, no CTCSS/DCS, and one
        // 7 ms blanking interval. The hashes cover all 7,680 emitted base and
        // final voice codes over 48 retained 20 ms intervals.
        const C_BASE_HASH: u64 = 0x0b96_e0ad_e1b7_9fbd;
        const C_FLAT_NO_DELAY_HASH: u64 = 0x1f6b_a2c2_f5da_d9ac;
        const C_SPEAKER_30MS_DELAY_HASH: u64 = 0x36c6_f2f7_3f03_faad;
        const C_FLAT_40MS_DELAY_HASH: u64 = 0x8187_105d_0d1e_13ac;
        const C_SPEAKER_40MS_DELAY_HASH: u64 = 0x065e_1bd0_dfbd_57ad;

        for variant in 0_u8..16 {
            let expected_voice_hash = match (variant & 1 != 0, variant & 8 != 0) {
                (false, false) => C_FLAT_NO_DELAY_HASH,
                (true, false) => C_SPEAKER_30MS_DELAY_HASH,
                (false, true) => C_FLAT_40MS_DELAY_HASH,
                (true, true) => C_SPEAKER_40MS_DELAY_HASH,
            };

            let mut fixed =
                ReceivePath::new(deployed_oracle_configuration(variant)).expect("fixed path");
            let expected = process_deployed_oracle(&mut fixed, &[960]);
            assert_eq!(expected.baseband.len(), 7_680, "variant {variant}");
            assert_eq!(expected.voice.len(), 7_680, "variant {variant}");
            assert_eq!(fnv1a_i16_codes(&expected.baseband), C_BASE_HASH);
            assert_eq!(fnv1a_i16_codes(&expected.voice), expected_voice_hash);

            let mut split =
                ReceivePath::new(deployed_oracle_configuration(variant)).expect("split path");
            // C's historic VOX timer reports different end-of-silence status
            // for this partition pattern even while its PCM is identical.
            // The owned path intentionally advances timing by elapsed samples,
            // so its complete observable result is partition invariant.
            assert_eq!(
                process_deployed_oracle(&mut split, &[5, 1, 7, 5, 942]),
                expected,
                "variant {variant}"
            );
        }
    }
}
