//! Legacy-compatible post-filter CTCSS receive qualification.
//!
//! The compatibility adapters retain the original 8 kHz subaudible frontend.
//! This state machine starts at its established center-slicer output and ports
//! only the per-tone correlator, qualification, relaxed talk-off, and release
//! blanking behavior.  Keeping this boundary narrow avoids changing the
//! legacy filter response while making the decoder portable and callback-safe.

use super::ctcss_transmit::FREQUENCIES;

/// Fixed legacy decoder sample rate after the compatibility frontend.
pub(crate) const SAMPLE_RATE_HZ: u32 = 8_000;
/// Number of supported legacy CTCSS detector bins.
pub(crate) const TONE_COUNT: usize = FREQUENCIES.len();
const TONE_MASK: u64 = (1_u64 << TONE_COUNT) - 1;
const SCOUNT_MUL: i32 = 100;
const Q15: i32 = 32_767;
const SET_POINT: i16 = (Q15 as f64 * 0.041) as i16;
const HYSTERESIS: i16 = (Q15 as f64 * 0.0130) as i16;
const BIN_FACTOR: i16 = (Q15 as f64 * 0.135) as i16;
const FUDGE_FACTOR: i16 = 8;
const RELEASE_BLANKING_SAMPLES: i32 = SAMPLE_RATE_HZ as i32 / 5;
const Q8: u32 = 256;
const PHASE_LOCK_TOLERANCE_Q8: u32 = Q8 * 2;
const MAX_PHASE_WINDOW_SAMPLES: usize = 120;
/// Half-cycle bounds for the standard 55 Hz CTCSS tail tone.
///
/// The 70 through 76 sample range admits a 55 Hz tail at 8 kHz while rejecting
/// the adjacent 50 Hz and 60 Hz mains frequencies.
const TAIL_HALF_CYCLE_SAMPLES: core::ops::RangeInclusive<u16> = 70..=76;
const TAIL_REQUIRED_HALF_CYCLES: u8 = 2;

/// Exact CTCSS correlator clock dividers from the legacy 8 kHz decoder.
const COUNTER_FACTORS: [i16; TONE_COUNT] = [
    2_985, 2_782, 2_688, 2_597, 2_509, 2_424, 2_342, 2_260, 2_186, 2_110, 2_053, 2_000, 1_932,
    1_866, 1_803, 1_742, 1_684, 1_626, 1_571, 1_517, 1_465, 1_415, 1_368, 1_321, 1_276, 1_233,
    1_191, 1_151, 1_112, 1_074, 1_037, 983, 949, 917, 886, 856, 827, 799,
];

/// Mutable correlator state for one configured CTCSS table entry.
#[derive(Clone, Copy, Default)]
struct Detector {
    counter: i16,
    counter_factor: i16,
    bin_factor: i16,
    fudge_factor: i16,
    peak: i16,
    z_index: usize,
    z: [i16; 4],
    dvu: i16,
    dvd: i16,
    zd: i16,
    set_point: i16,
    hysteresis: i16,
    decode: i16,
}

impl Detector {
    /// Construct one detector with the deployed compatibility constants.
    const fn configured(counter_factor: i16) -> Self {
        Self {
            counter: 0,
            counter_factor,
            bin_factor: BIN_FACTOR,
            fudge_factor: FUDGE_FACTOR,
            peak: 0,
            z_index: 0,
            z: [0; 4],
            dvu: 0,
            dvd: 0,
            zd: 0,
            set_point: SET_POINT,
            hysteresis: HYSTERESIS,
            decode: 0,
        }
    }

    /// Clear only the state cleared by the legacy loss path.
    fn clear_after_loss(&mut self) {
        self.decode = 0;
        self.z = [0; 4];
    }
}

/// Callback-owned detector for CTCSS release signaling.
///
/// The input has already passed the CTCSS center slicer.  It uses half-cycle
/// intervals to recognize the 55 Hz tail, then compares selected-tone signs
/// one period apart to recognize a 120-degree-or-greater phase discontinuity.
#[derive(Clone, Copy, Eq, PartialEq)]
struct ReleaseDetector {
    sign: i8,
    samples: u16,
    matched_half_cycles: u8,
    tail_detected: bool,
    phase_pending: bool,
    interval_ready: bool,
    nominal_half_cycles: u8,
    phase_locked: bool,
    phase_sign_history: [i8; MAX_PHASE_WINDOW_SAMPLES],
    phase_mismatch_history: [u8; MAX_PHASE_WINDOW_SAMPLES],
    phase_index: usize,
    phase_history_count: usize,
    phase_mismatch_count: usize,
    phase_mismatch_total: usize,
}

impl Default for ReleaseDetector {
    fn default() -> Self {
        Self {
            sign: 0,
            samples: 0,
            matched_half_cycles: 0,
            tail_detected: false,
            phase_pending: false,
            interval_ready: false,
            nominal_half_cycles: 0,
            phase_locked: false,
            phase_sign_history: [0; MAX_PHASE_WINDOW_SAMPLES],
            phase_mismatch_history: [0; MAX_PHASE_WINDOW_SAMPLES],
            phase_index: 0,
            phase_history_count: 0,
            phase_mismatch_count: 0,
            phase_mismatch_total: 0,
        }
    }
}

impl ReleaseDetector {
    /// Clear a partial candidate whenever decoder qualification is lost.
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Consume one center-sliced sample and report a recognized release signal.
    fn process(
        &mut self,
        sample: i16,
        expected_half_cycle_q8: u32,
        phase_period_samples: usize,
    ) -> bool {
        self.tail_detected = false;
        self.samples = self.samples.saturating_add(1);
        if sample == 0 {
            return false;
        }
        let sign = if sample.is_negative() { -1 } else { 1 };
        let previous_sign = self.sign;
        if previous_sign == 0 {
            self.sign = sign;
            self.samples = 0;
            return false;
        }
        self.phase_pending |= self.process_phase(sign, phase_period_samples);
        if previous_sign == sign {
            return false;
        }

        let interval = self.samples;
        self.sign = sign;
        self.samples = 0;
        if !self.interval_ready {
            // The first interval begins at an arbitrary callback boundary and
            // cannot describe a phase discontinuity.
            self.interval_ready = true;
            return false;
        }

        let tail_matched = TAIL_HALF_CYCLE_SAMPLES.contains(&interval);
        self.matched_half_cycles = if tail_matched {
            self.matched_half_cycles.saturating_add(1)
        } else {
            0
        };
        self.update_phase_lock(interval, expected_half_cycle_q8);
        self.tail_detected = self.matched_half_cycles >= TAIL_REQUIRED_HALF_CYCLES;
        // A selected tone changing to 55 Hz can look like a phase shift before
        // two tail half-cycles complete.  Resolve the complete interval first.
        self.tail_detected || (self.phase_pending && !tail_matched)
    }

    /// Lock only after stable selected-tone half cycles to reject unrelated tones.
    fn update_phase_lock(&mut self, interval: u16, expected_half_cycle_q8: u32) {
        if self.phase_locked {
            return;
        }
        let interval_q8 = u32::from(interval) * Q8;
        if interval_q8.abs_diff(expected_half_cycle_q8) <= PHASE_LOCK_TOLERANCE_Q8 {
            self.nominal_half_cycles = self.nominal_half_cycles.saturating_add(1);
        } else {
            self.nominal_half_cycles = 0;
        }
        if self.nominal_half_cycles >= 2 {
            self.phase_locked = true;
            self.phase_index = 0;
            self.phase_history_count = 0;
            self.phase_mismatch_count = 0;
            self.phase_mismatch_total = 0;
        }
    }

    /// Compare the sign with the sign exactly one selected-tone period earlier.
    fn process_phase(&mut self, sign: i8, phase_period_samples: usize) -> bool {
        if !self.phase_locked {
            return false;
        }

        let index = self.phase_index;
        if self.phase_history_count < phase_period_samples {
            self.phase_history_count += 1;
        } else {
            let mismatch = usize::from(sign != self.phase_sign_history[index]);
            if self.phase_mismatch_count < phase_period_samples {
                self.phase_mismatch_count += 1;
            } else {
                self.phase_mismatch_total -= self.phase_mismatch_history[index] as usize;
            }
            self.phase_mismatch_history[index] = mismatch as u8;
            self.phase_mismatch_total += mismatch;
        }
        self.phase_sign_history[index] = sign;
        self.phase_index = (index + 1) % phase_period_samples;
        self.phase_mismatch_count == phase_period_samples
            && self.phase_mismatch_total * 12 >= phase_period_samples * 7
    }
}

/// Callback-owned CTCSS receiver state for one fixed native stream.
pub(crate) struct ReceiveState {
    tone_mask: u64,
    relax: bool,
    decoded: i16,
    blanking_samples: i32,
    detectors: [Detector; TONE_COUNT],
    release_detector: ReleaseDetector,
    tail_tone_active: bool,
}

impl Default for ReceiveState {
    fn default() -> Self {
        Self {
            tone_mask: 0,
            relax: false,
            decoded: -1,
            blanking_samples: 0,
            detectors: [Detector::default(); TONE_COUNT],
            release_detector: ReleaseDetector::default(),
            tail_tone_active: false,
        }
    }
}

impl ReceiveState {
    /// Reset the detector bank for a new selected-tone mask or relax mode.
    pub(crate) fn configure(&mut self, tone_mask: u64, relax: bool) {
        self.tone_mask = tone_mask & TONE_MASK;
        self.relax = relax;
        self.decoded = -1;
        self.blanking_samples = 0;
        self.detectors = COUNTER_FACTORS.map(Detector::configured);
        self.release_detector.reset();
        self.tail_tone_active = false;
    }

    /// Return whether at least one configured table entry can be decoded.
    pub(crate) fn enabled(&self) -> bool {
        self.tone_mask != 0
    }

    /// Return the current compatibility-table decode index.
    pub(crate) fn decoded(&self) -> i16 {
        self.decoded
    }

    /// Return whether the current carrier contains a recognized 55 Hz tail.
    pub(crate) fn tail_tone_active(&self) -> bool {
        self.tail_tone_active
    }

    /// End a recognized tail only when the CTCSS carrier source drops.
    pub(crate) fn clear_tail_on_carrier_loss(&mut self, carrier_detect: bool) {
        if !carrier_detect {
            self.tail_tone_active = false;
        }
    }

    /// Set one detector to a deterministic focused-test configuration.
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn test_prepare_detector(&mut self, tone_index: usize) {
        let detector = &mut self.detectors[tone_index];

        detector.counter = 0;
        detector.counter_factor = SCOUNT_MUL as i16;
        detector.bin_factor = Q15 as i16;
        detector.fudge_factor = 1;
        detector.peak = 0;
        detector.set_point = -1;
        detector.hysteresis = 0;
        detector.decode = 0;
        detector.z = [0; 4];
    }

    /// Change deterministic test detector thresholds without exposing state.
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn test_set_detector_release(&mut self, tone_index: usize) {
        let detector = &mut self.detectors[tone_index];

        detector.counter = 0;
        detector.set_point = 1_000;
        detector.hysteresis = 100;
        detector.decode = 1;
    }

    /// Read a focused-test release blanking count.
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn test_blanking_samples(&self) -> i32 {
        self.blanking_samples
    }

    /// Read one focused-test detector qualification counter.
    #[cfg(test)]
    #[cfg_attr(coverage, coverage(off))]
    pub(crate) fn test_detector_decode(&self, tone_index: usize) -> i16 {
        self.detectors[tone_index].decode
    }

    /// Reconstruct an exact signed-16 legacy center-slicer sample from F32.
    fn sample_to_i16(sample: f32) -> i16 {
        // The compatibility callback creates input as `i16 / 32768.0F`, so
        // finite normal samples round-trip exactly.  Bounds also make an ABI
        // caller supplying an out-of-range F32 value deterministic and safe.
        let scaled = (sample * 32_768.0).clamp(f32::from(i16::MIN), f32::from(i16::MAX));
        scaled as i16
    }

    /// Immediately release the active decode and preserve ordinary re-acquire blanking.
    fn release_decode(&mut self, tail_tone_active: bool) {
        self.blanking_samples = RELEASE_BLANKING_SAMPLES;
        self.decoded = -1;
        self.release_detector.reset();
        self.tail_tone_active = tail_tone_active;
        for detector in &mut self.detectors {
            detector.clear_after_loss();
        }
    }

    /// Detect known CTCSS release signaling after this stream qualified a tone.
    ///
    /// The release detector runs at every center-sliced input sample.  It
    /// closes on two 55 Hz tail half-cycles or a 120-degree-or-greater phase
    /// discontinuity in the selected CTCSS tone.
    unsafe fn release_signal_sample_count(
        &mut self,
        samples: *const f32,
        sample_count: u32,
    ) -> Option<(u32, bool)> {
        if self.decoded < 0 {
            self.release_detector.reset();
            return None;
        }
        let (expected_half_cycle_q8, phase_period_samples) =
            Self::phase_parameters(FREQUENCIES[self.decoded as usize]);
        for index in 0..sample_count as usize {
            // SAFETY: the caller guarantees the complete input span is readable.
            let sample = unsafe { samples.add(index).read() };
            if self.release_detector.process(
                Self::sample_to_i16(sample),
                expected_half_cycle_q8,
                phase_period_samples,
            ) {
                return Some((index as u32 + 1, self.release_detector.tail_detected));
            }
        }
        None
    }

    /// Return fixed-point reference values for one selected CTCSS frequency.
    fn phase_parameters(frequency_hz: f64) -> (u32, usize) {
        let expected_half_cycle_q8 =
            (f64::from(SAMPLE_RATE_HZ) * f64::from(Q8) / (2.0 * frequency_hz)).round() as u32;
        let phase_period_samples = (f64::from(SAMPLE_RATE_HZ) / frequency_hz).round() as usize;
        (expected_half_cycle_q8, phase_period_samples)
    }

    /// Consume one legacy decoder sample for one configured detector.
    fn process_sample(
        detector: &mut Detector,
        accum: i16,
        selected: bool,
        relax: bool,
        carrier_detect: bool,
    ) -> bool {
        let bin_factor = i32::from(detector.bin_factor);
        let z_index = detector.z_index;
        let updated_z = i32::from(detector.z[z_index])
            + ((i32::from(accum) - i32::from(detector.z[z_index])) * bin_factor) / Q15;
        detector.z[z_index] = updated_z as i16;

        let peak = (i32::from(detector.z[0]) - i32::from(detector.z[2])).abs()
            + (i32::from(detector.z[1]) - i32::from(detector.z[3])).abs();
        if i32::from(detector.peak) < peak {
            let updated_peak =
                i32::from(detector.peak) + ((peak - i32::from(detector.peak)) * bin_factor) / Q15;
            detector.peak = updated_peak as i16;
        } else {
            detector.peak = peak as i16;
        }

        // These coefficients and the /1024 truncation are copied from the
        // deployed compatibility detector.  They recognize the envelope
        // transition of a reverse burst before it can qualify as voice PL.
        let temp0 = i32::from(detector.zd) * -13_723;
        detector.zd = detector.peak;
        let temp1 = i32::from(detector.peak) * 13_723;
        let diff_peak = ((temp0 + temp1) / 1_024) as i16;
        if f64::from(diff_peak) < -0.03 * f64::from(Q15) {
            detector.dvd = detector.dvd.wrapping_sub(4);
        } else if detector.dvd < 0 {
            detector.dvd = detector.dvd.wrapping_add(1);
        }
        if detector.dvd < -12 && f64::from(diff_peak) > -0.02 * f64::from(Q15) {
            detector.dvu = detector.dvu.wrapping_add(2);
        } else if detector.dvu != 0 {
            detector.dvu = detector.dvu.wrapping_sub(1);
        }

        let mut threshold = detector.set_point;
        if selected {
            threshold = if relax {
                ((i32::from(threshold) * 55) / 100) as i16
            } else {
                ((i32::from(threshold) * 80) / 100) as i16
            };
        }
        if detector.peak > threshold {
            if detector.decode < detector.fudge_factor * 32 {
                detector.decode = detector.decode.wrapping_add(1);
            }
        } else if selected {
            // Relaxed talk-off retains the legacy one-step release even
            // below hysteresis; normal mode drops four counts there.
            if detector.peak > detector.hysteresis || relax {
                detector.decode = detector.decode.wrapping_sub(1);
            } else {
                detector.decode = detector.decode.wrapping_sub(4);
            }
        } else {
            detector.decode = 0;
        }

        if selected && !relax && f64::from(detector.dvu) > 0.00075 * f64::from(Q15) {
            detector.decode = 0;
            detector.z = [0; 4];
            detector.dvu = 0;
        }
        if detector.decode < 0 || !carrier_detect {
            detector.decode = 0;
        }
        let qualified = detector.decode >= detector.fudge_factor;
        if qualified && !selected {
            detector.zd = 0;
            detector.dvu = 0;
            detector.dvd = 0;
        }
        detector.z_index = (detector.z_index + 1) % detector.z.len();
        qualified
    }

    /// Process a post-filter mono F32 span and return the selected table index.
    ///
    /// # Safety
    /// `samples` is readable for `sample_count` F32 values when that count is
    /// nonzero.  The caller serializes access through its callback-owned radio.
    pub(crate) unsafe fn process(
        &mut self,
        samples: *const f32,
        sample_count: u32,
        carrier_detect: bool,
    ) -> i16 {
        if !self.enabled() {
            self.tail_tone_active = false;
            return -1;
        }

        if !carrier_detect {
            self.clear_tail_on_carrier_loss(false);
            self.release_detector.reset();
            return unsafe { self.process_decoder(samples, sample_count, false) };
        }

        if let Some((release_sample_count, tail_tone_active)) =
            unsafe { self.release_signal_sample_count(samples, sample_count) }
        {
            // Advance the compatibility correlator up to the event, then
            // release at its sample boundary.  Processing the remainder with
            // blanking active gives the same state for whole and split calls.
            unsafe { self.process_decoder(samples, release_sample_count, true) };
            self.release_decode(tail_tone_active);
            let remaining = sample_count - release_sample_count;
            if remaining != 0 {
                unsafe {
                    self.process_decoder(
                        samples.add(release_sample_count as usize),
                        remaining,
                        true,
                    )
                };
            }
            return self.decoded;
        }

        unsafe { self.process_decoder(samples, sample_count, true) }
    }

    /// Advance the legacy correlator without inspecting release signaling.
    ///
    /// # Safety
    /// `samples` is readable for `sample_count` F32 values when that count is
    /// nonzero.  The caller serializes access through its callback-owned radio.
    unsafe fn process_decoder(
        &mut self,
        samples: *const f32,
        sample_count: u32,
        carrier_detect: bool,
    ) -> i16 {
        let decoded_before = self.decoded;
        let mut hit = -1_i16;
        let mut selected_detector_probed = false;
        let mut selected_detector_qualified = false;
        let mut points = 0_i32;
        for tone_index in 0..TONE_COUNT {
            if self.tone_mask & (1_u64 << tone_index) == 0
                || (self.decoded >= 0 && tone_index != self.decoded as usize)
            {
                continue;
            }
            let detector = &mut self.detectors[tone_index];
            let mut points_remaining = sample_count as i32;
            points = points_remaining;
            while i32::from(detector.counter) < points_remaining * SCOUNT_MUL {
                let advance = i32::from(detector.counter) / SCOUNT_MUL + 1;
                detector.counter = (i32::from(detector.counter) - advance * SCOUNT_MUL) as i16;
                points_remaining -= advance;
                let input_index = (points - points_remaining - 1) as usize;
                detector.counter =
                    (i32::from(detector.counter) + i32::from(detector.counter_factor)) as i16;
                let sample = unsafe { samples.add(input_index).read() };
                let selected = decoded_before == tone_index as i16;
                let qualified = Self::process_sample(
                    detector,
                    Self::sample_to_i16(sample),
                    selected,
                    self.relax,
                    carrier_detect,
                );
                if selected {
                    // A detector has its own probe clock.  A short callback
                    // with no probe is not a decode loss; when multiple
                    // probes occur, the final probe describes this span's
                    // terminal state independently of its partitioning.
                    selected_detector_probed = true;
                    selected_detector_qualified = qualified;
                }
                if qualified {
                    hit = tone_index as i16;
                }
            }
            detector.counter = (i32::from(detector.counter) - points_remaining * SCOUNT_MUL) as i16;
        }

        if self.blanking_samples > 0 {
            self.blanking_samples -= points;
        }
        if self.blanking_samples < 0 {
            self.blanking_samples = 0;
        }
        if hit >= 0 && self.decoded < 0 && self.blanking_samples == 0 {
            self.decoded = hit;
        } else if self.decoded >= 0 && selected_detector_probed && !selected_detector_qualified {
            self.release_decode(false);
        }
        self.decoded
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn hysteresis_release_and_reverse_burst_reset_selected_decode() {
        let mut detector = Detector {
            fudge_factor: 1,
            decode: 10,
            peak: 100,
            z: [100, 0, 0, 0],
            set_point: 1000,
            hysteresis: 10,
            ..Detector::default()
        };
        assert!(ReceiveState::process_sample(
            &mut detector,
            0,
            true,
            false,
            true
        ));
        assert_eq!(detector.decode, 9);
        detector.dvu = 30;
        assert!(!ReceiveState::process_sample(
            &mut detector,
            0,
            true,
            false,
            true
        ));
        assert_eq!(detector.decode, 0);
        assert_eq!(detector.z, [0; 4]);
        assert_eq!(detector.dvu, 0);
    }

    #[test]
    fn relaxed_release_and_saturated_decode_preserve_correlator_rules() {
        let mut detector = Detector {
            fudge_factor: 1,
            decode: 32,
            set_point: -100,
            ..Detector::default()
        };
        assert!(ReceiveState::process_sample(
            &mut detector,
            0,
            true,
            true,
            true
        ));
        assert_eq!(detector.decode, 32);
        detector.set_point = 1000;
        detector.hysteresis = 1000;
        detector.decode = 10;
        assert!(ReceiveState::process_sample(
            &mut detector,
            0,
            true,
            true,
            true
        ));
        assert_eq!(detector.decode, 9);
    }

    #[test]
    fn disabled_and_multitone_probe_and_expired_blanking_are_deterministic() {
        let mut state = ReceiveState::default();
        assert_eq!(unsafe { state.process(core::ptr::null(), 0, true) }, -1);
        state.configure(3, false);
        state.test_prepare_detector(0);
        state.detectors[0].set_point = -100;
        state.decoded = 0;
        state.blanking_samples = 1;
        assert_eq!(unsafe { state.process([0.0; 2].as_ptr(), 2, true) }, 0);
        assert_eq!(state.blanking_samples, 0);
    }

    /// Prime one selected production-divider detector without changing its clock.
    fn activate_selected_detector(state: &mut ReceiveState, tone_index: usize) {
        state.configure(1_u64 << tone_index, false);
        state.decoded = tone_index as i16;
        let detector = &mut state.detectors[tone_index];

        detector.counter = 0;
        detector.decode = detector.fudge_factor;
        detector.set_point = -2;
        detector.hysteresis = 0;
        detector.peak = 0;
        detector.z = [0; 4];
        detector.dvu = 0;
        detector.dvd = 0;
        detector.zd = 0;
    }

    /// Render the center-sliced waveform delivered by the compatibility frontend.
    fn center_sliced_tone(
        samples: &mut [f32],
        frequency_hz: f32,
        phase_step: Option<(usize, f32)>,
    ) {
        for (index, sample) in samples.iter_mut().enumerate() {
            let phase = core::f32::consts::TAU * frequency_hz * index as f32
                / SAMPLE_RATE_HZ as f32
                + phase_step
                    .filter(|(start, _)| index >= *start)
                    .map_or(0.0, |(_, step)| step);
            *sample = if phase.sin() >= 0.0 {
                625.0 / 32_768.0
            } else {
                -625.0 / 32_768.0
            };
        }
    }

    /// Report whether a test waveform triggers only the release detector.
    fn has_release_signal(samples: &[f32], frequency_hz: f64) -> bool {
        let mut detector = ReleaseDetector::default();
        let (expected_half_cycle_q8, phase_period_samples) =
            ReceiveState::phase_parameters(frequency_hz);

        samples.iter().copied().any(|sample| {
            detector.process(
                ReceiveState::sample_to_i16(sample),
                expected_half_cycle_q8,
                phase_period_samples,
            )
        })
    }

    /// Make one selected detector probe every supplied sample for focused loss tests.
    fn prepare_fast_selected_detector(state: &mut ReceiveState, tone_index: usize, set_point: i16) {
        activate_selected_detector(state, tone_index);
        let detector = &mut state.detectors[tone_index];

        detector.counter = 0;
        detector.counter_factor = SCOUNT_MUL as i16;
        detector.bin_factor = Q15 as i16;
        detector.fudge_factor = 1;
        detector.decode = 1;
        detector.set_point = set_point;
        detector.hysteresis = 0;
    }

    #[test]
    fn table_matches_the_deployed_legacy_tones_and_divisors() {
        assert_eq!(TONE_COUNT, 38);
        assert_eq!(FREQUENCIES[0], 67.0);
        assert_eq!(FREQUENCIES[11], 100.0);
        assert_eq!(FREQUENCIES[37], 250.3);
        assert_eq!(COUNTER_FACTORS[0], 2_985);
        assert_eq!(COUNTER_FACTORS[11], 2_000);
        assert_eq!(COUNTER_FACTORS[37], 799);
    }

    #[test]
    fn configuration_masks_unsupported_bits_and_resets_state() {
        let mut state = ReceiveState::default();
        state.configure(u64::MAX, true);
        assert!(state.enabled());
        assert_eq!(state.tone_mask, TONE_MASK);
        assert!(state.relax);
        state.decoded = 11;
        state.blanking_samples = 99;
        state.configure(1_u64 << 11, false);
        assert_eq!(state.decoded(), -1);
        assert_eq!(state.blanking_samples, 0);
        assert_eq!(state.tone_mask, 1_u64 << 11);
        assert!(!state.relax);
    }

    #[test]
    fn qualification_release_and_blank_behavior_match_the_fixed_point_state_machine() {
        let mut state = ReceiveState::default();
        state.configure(1_u64 << 11, false);
        // Match the focused C harness: one sample visit, immediate threshold
        // qualification, then loss and a 200 ms release blanking period.
        state.test_prepare_detector(11);
        let silence = [0.0_f32];
        assert_eq!(unsafe { state.process(silence.as_ptr(), 1, true) }, 11);
        state.test_set_detector_release(11);
        assert_eq!(unsafe { state.process(silence.as_ptr(), 1, true) }, -1);
        assert_eq!(state.blanking_samples, RELEASE_BLANKING_SAMPLES);
        state.detectors[11].set_point = -1;
        state.detectors[11].counter = 0;
        assert_eq!(unsafe { state.process(silence.as_ptr(), 1, true) }, -1);
        assert_eq!(state.blanking_samples, RELEASE_BLANKING_SAMPLES - 1);
    }

    #[test]
    fn active_decode_survives_a_callback_without_a_selected_detector_probe() {
        const TONE_INDEX: usize = 11;
        let mut state = ReceiveState::default();
        let silence = [0.0_f32];

        activate_selected_detector(&mut state, TONE_INDEX);
        // The deployed 100 Hz divider has just under one selected probe per
        // 20 baseband samples.  This one-sample callback advances its clock
        // but intentionally contains no probe.
        state.detectors[TONE_INDEX].counter = SCOUNT_MUL as i16;
        assert_eq!(
            unsafe { state.process(silence.as_ptr(), 1, true) },
            TONE_INDEX as i16
        );
        assert_eq!(state.decoded(), TONE_INDEX as i16);
        assert_eq!(state.detectors[TONE_INDEX].counter, 0);
    }

    #[test]
    fn active_tone_matches_full_and_irregular_callback_partitions() {
        const TONE_INDEX: usize = 11;
        const PARTITIONS: [usize; 5] = [5, 1, 7, 5, 942];
        let samples = [0.0_f32; 960];
        let mut whole = ReceiveState::default();
        let mut split = ReceiveState::default();
        let mut offset = 0;

        activate_selected_detector(&mut whole, TONE_INDEX);
        activate_selected_detector(&mut split, TONE_INDEX);
        assert_eq!(
            unsafe { whole.process(samples.as_ptr(), samples.len() as u32, true) },
            TONE_INDEX as i16
        );
        for count in PARTITIONS {
            let span = &samples[offset..offset + count];

            assert_eq!(
                unsafe { split.process(span.as_ptr(), count as u32, true) },
                TONE_INDEX as i16
            );
            offset += count;
        }
        assert_eq!(offset, samples.len());
        assert_eq!(split.decoded(), whole.decoded());
        let expected = &whole.detectors[TONE_INDEX];
        let actual = &split.detectors[TONE_INDEX];

        assert_eq!(actual.counter, expected.counter);
        assert_eq!(actual.peak, expected.peak);
        assert_eq!(actual.z_index, expected.z_index);
        assert_eq!(actual.z, expected.z);
        assert_eq!(actual.dvu, expected.dvu);
        assert_eq!(actual.dvd, expected.dvd);
        assert_eq!(actual.zd, expected.zd);
        assert_eq!(actual.decode, expected.decode);
    }

    #[test]
    fn selected_decode_uses_the_final_probe_when_a_callback_contains_loss() {
        const TONE_INDEX: usize = 11;
        let mut state = ReceiveState::default();
        let samples = [1_000.0_f32 / 32_768.0, 0.0, 0.0, 0.0, 0.0, 0.0];

        prepare_fast_selected_detector(&mut state, TONE_INDEX, 1);
        assert_eq!(
            unsafe { state.process(samples.as_ptr(), samples.len() as u32, true) },
            -1
        );
        assert_eq!(state.decoded(), -1);
        assert_eq!(state.test_blanking_samples(), RELEASE_BLANKING_SAMPLES);
    }

    #[test]
    fn selected_decode_survives_a_transient_probe_loss_that_recovers_in_one_callback() {
        const TONE_INDEX: usize = 11;
        let mut state = ReceiveState::default();
        let samples = [0.0_f32, 1_000.0 / 32_768.0];

        prepare_fast_selected_detector(&mut state, TONE_INDEX, 1_000);
        assert_eq!(
            unsafe { state.process(samples.as_ptr(), samples.len() as u32, true) },
            TONE_INDEX as i16
        );
        assert_eq!(state.decoded(), TONE_INDEX as i16);
        assert_eq!(state.test_blanking_samples(), 0);
    }

    #[test]
    fn fifty_five_hz_tail_closes_an_active_decode() {
        const TONE_INDEX: usize = 11;
        let mut state = ReceiveState::default();
        let mut tail = [0.0_f32; 512];

        activate_selected_detector(&mut state, TONE_INDEX);
        state.detectors[TONE_INDEX].counter = i16::MAX;
        center_sliced_tone(&mut tail, 55.0, None);

        assert_eq!(
            unsafe { state.process(tail.as_ptr(), tail.len() as u32, true) },
            -1
        );
        assert_eq!(state.decoded(), -1);
    }

    #[test]
    fn tail_state_persists_until_carrier_loss_but_phase_release_does_not_set_it() {
        const TONE_INDEX: usize = 11;
        let mut selected = [0.0_f32; 640];
        let mut tail = [0.0_f32; 512];
        let mut tail_state = ReceiveState::default();

        activate_selected_detector(&mut tail_state, TONE_INDEX);
        tail_state.detectors[TONE_INDEX].counter = i16::MAX;
        center_sliced_tone(&mut selected, 100.0, None);
        assert_eq!(
            unsafe { tail_state.process(selected.as_ptr(), selected.len() as u32, true) },
            TONE_INDEX as i16
        );
        assert!(tail_state.release_detector.phase_locked);
        center_sliced_tone(&mut tail, 55.0, None);
        assert_eq!(
            unsafe { tail_state.process(tail.as_ptr(), tail.len() as u32, true) },
            -1
        );
        assert!(tail_state.tail_tone_active());
        assert_eq!(
            unsafe { tail_state.process(core::ptr::null(), 0, true) },
            -1
        );
        assert!(tail_state.tail_tone_active());
        assert_eq!(
            unsafe { tail_state.process(core::ptr::null(), 0, false) },
            -1
        );
        assert!(!tail_state.tail_tone_active());

        let mut phase = [0.0_f32; 480];
        let mut phase_state = ReceiveState::default();
        activate_selected_detector(&mut phase_state, TONE_INDEX);
        phase_state.detectors[TONE_INDEX].counter = i16::MAX;
        center_sliced_tone(
            &mut phase,
            100.0,
            Some((320, 2.0 * core::f32::consts::PI / 3.0)),
        );
        assert_eq!(
            unsafe { phase_state.process(phase.as_ptr(), phase.len() as u32, true) },
            -1
        );
        assert!(!phase_state.tail_tone_active());
    }

    #[test]
    fn fifty_five_hz_tail_wins_over_phase_release_for_every_supported_tone() {
        for (tone_index, frequency_hz) in FREQUENCIES.into_iter().enumerate() {
            let period = (f64::from(SAMPLE_RATE_HZ) / frequency_hz).ceil() as usize;
            let mut selected = vec![0.0_f32; period * 4];
            let mut tail = [0.0_f32; 512];
            let mut state = ReceiveState::default();

            activate_selected_detector(&mut state, tone_index);
            state.detectors[tone_index].counter = i16::MAX;
            center_sliced_tone(&mut selected, frequency_hz as f32, None);
            assert_eq!(
                unsafe { state.process(selected.as_ptr(), selected.len() as u32, true) },
                tone_index as i16
            );
            assert!(state.release_detector.phase_locked);

            center_sliced_tone(&mut tail, 55.0, None);
            assert_eq!(
                unsafe { state.process(tail.as_ptr(), tail.len() as u32, true) },
                -1,
                "{frequency_hz} Hz decoded tone"
            );
            assert!(state.tail_tone_active(), "{frequency_hz} Hz decoded tone");
        }
    }

    #[test]
    fn legacy_reverse_burst_envelope_closes_an_active_decode() {
        const TONE_INDEX: usize = 11;
        let mut state = ReceiveState::default();

        activate_selected_detector(&mut state, TONE_INDEX);
        state.detectors[TONE_INDEX].dvu = 30;
        assert_eq!(unsafe { state.process([0.0].as_ptr(), 1, true) }, -1);
        assert_eq!(state.decoded(), -1);
    }

    #[test]
    fn one_hundred_twenty_degree_phase_shift_closes_an_active_decode() {
        const TONE_INDEX: usize = 11;
        let mut state = ReceiveState::default();
        let mut tone = [0.0_f32; 480];

        activate_selected_detector(&mut state, TONE_INDEX);
        state.detectors[TONE_INDEX].counter = i16::MAX;
        center_sliced_tone(
            &mut tone,
            100.0,
            Some((320, 2.0 * core::f32::consts::PI / 3.0)),
        );

        assert_eq!(
            unsafe { state.process(tone.as_ptr(), tone.len() as u32, true) },
            -1
        );
        assert_eq!(state.decoded(), -1);
    }

    #[test]
    fn release_detector_accepts_one_hundred_twenty_but_not_ninety_degrees() {
        let mut one_twenty = [0.0_f32; 480];
        let mut ninety = [0.0_f32; 480];

        center_sliced_tone(
            &mut one_twenty,
            100.0,
            Some((320, 2.0 * core::f32::consts::PI / 3.0)),
        );
        center_sliced_tone(&mut ninety, 100.0, Some((320, core::f32::consts::PI / 2.0)));

        assert!(has_release_signal(&one_twenty, 100.0));
        assert!(!has_release_signal(&ninety, 100.0));
    }

    #[test]
    fn phase_release_threshold_covers_each_tone_and_phase_offset() {
        for frequency_hz in FREQUENCIES {
            let (expected_half_cycle_q8, _) = ReceiveState::phase_parameters(frequency_hz);
            let period = (expected_half_cycle_q8 as usize).div_ceil(Q8 as usize) * 2;
            // Allow the reference to observe multiple stable crossings before
            // applying the reverse-burst step.
            let warmup = period * 4;
            for phase_offset in 0..period {
                for (phase_step, expected_release) in [
                    (2.0 * core::f32::consts::PI / 3.0, true),
                    (5.0 * core::f32::consts::PI / 6.0, true),
                    (core::f32::consts::PI, true),
                    (7.0 * core::f32::consts::PI / 6.0, true),
                    (-2.0 * core::f32::consts::PI / 3.0, true),
                    (core::f32::consts::PI / 2.0, false),
                    (-core::f32::consts::PI / 2.0, false),
                ] {
                    let mut samples = vec![0.0_f32; warmup + period * 3];
                    center_sliced_tone(
                        &mut samples,
                        frequency_hz as f32,
                        Some((warmup + phase_offset, phase_step)),
                    );
                    assert_eq!(
                        has_release_signal(&samples, frequency_hz),
                        expected_release,
                        "{frequency_hz} Hz, phase offset {phase_offset}, step {phase_step}"
                    );
                }
            }
        }
    }

    #[test]
    fn phase_release_matches_whole_and_split_callbacks() {
        const TONE_INDEX: usize = 11;
        // Single-sample callbacks also place the release exactly at a callback end.
        for partitions in [&[133, 19, 488][..], &[1; 640][..]] {
            let mut whole = ReceiveState::default();
            let mut split = ReceiveState::default();
            let mut samples = [0.0_f32; 640];

            activate_selected_detector(&mut whole, TONE_INDEX);
            activate_selected_detector(&mut split, TONE_INDEX);
            whole.detectors[TONE_INDEX].counter = i16::MAX;
            split.detectors[TONE_INDEX].counter = i16::MAX;
            center_sliced_tone(
                &mut samples,
                100.0,
                Some((400, 2.0 * core::f32::consts::PI / 3.0)),
            );
            assert_eq!(
                unsafe { whole.process(samples.as_ptr(), samples.len() as u32, true) },
                -1
            );

            let mut offset = 0;
            for &count in partitions {
                unsafe { split.process(samples[offset..].as_ptr(), count as u32, true) };
                offset += count;
            }

            assert_eq!(offset, samples.len());
            assert_eq!(split.decoded(), whole.decoded());
            assert_eq!(split.blanking_samples, whole.blanking_samples);
            assert!(split.release_detector == whole.release_detector);
            for (actual, expected) in split.detectors.iter().zip(&whole.detectors) {
                assert_eq!(actual.counter, expected.counter);
                assert_eq!(actual.peak, expected.peak);
                assert_eq!(actual.z_index, expected.z_index);
                assert_eq!(actual.z, expected.z);
                assert_eq!(actual.dvu, expected.dvu);
                assert_eq!(actual.dvd, expected.dvd);
                assert_eq!(actual.zd, expected.zd);
                assert_eq!(actual.decode, expected.decode);
            }
        }
    }

    #[test]
    fn ninety_degree_phase_shift_preserves_an_active_decode() {
        const TONE_INDEX: usize = 11;
        let mut state = ReceiveState::default();
        let mut tone = [0.0_f32; 480];

        activate_selected_detector(&mut state, TONE_INDEX);
        state.detectors[TONE_INDEX].counter = i16::MAX;
        center_sliced_tone(&mut tone, 100.0, Some((320, core::f32::consts::PI / 2.0)));

        assert_eq!(
            unsafe { state.process(tone.as_ptr(), tone.len() as u32, true) },
            TONE_INDEX as i16
        );
        assert_eq!(state.decoded(), TONE_INDEX as i16);
    }

    #[test]
    fn mains_frequencies_do_not_look_like_release_signaling() {
        let (expected_half_cycle_q8, phase_period_samples) = ReceiveState::phase_parameters(100.0);
        for frequency_hz in [50.0_f32, 60.0] {
            let mut detector = ReleaseDetector::default();
            let mut tone = [0.0_f32; 512];

            center_sliced_tone(&mut tone, frequency_hz, None);
            assert!(tone.into_iter().all(|sample| !detector.process(
                ReceiveState::sample_to_i16(sample),
                expected_half_cycle_q8,
                phase_period_samples,
            )));
        }
    }

    #[test]
    fn tail_detector_requires_two_55_hz_half_cycles() {
        let mut detector = ReleaseDetector::default();
        let (expected_half_cycle_q8, phase_period_samples) = ReceiveState::phase_parameters(100.0);

        assert!(!detector.process(625, expected_half_cycle_q8, phase_period_samples));
        for _ in 0..72 {
            assert!(!detector.process(625, expected_half_cycle_q8, phase_period_samples));
        }
        assert!(!detector.process(-625, expected_half_cycle_q8, phase_period_samples));
        for _ in 0..72 {
            assert!(!detector.process(-625, expected_half_cycle_q8, phase_period_samples));
        }
        assert!(!detector.process(625, expected_half_cycle_q8, phase_period_samples));
        for _ in 0..72 {
            assert!(!detector.process(625, expected_half_cycle_q8, phase_period_samples));
        }
        assert!(detector.process(-625, expected_half_cycle_q8, phase_period_samples));
    }

    #[test]
    fn tail_release_matches_whole_and_split_callbacks() {
        const TONE_INDEX: usize = 11;
        const PARTITIONS: [usize; 3] = [91, 137, 28];
        let mut whole = ReceiveState::default();
        let mut split = ReceiveState::default();
        let mut tail = [0.0_f32; 256];

        activate_selected_detector(&mut whole, TONE_INDEX);
        activate_selected_detector(&mut split, TONE_INDEX);
        whole.detectors[TONE_INDEX].counter = i16::MAX;
        split.detectors[TONE_INDEX].counter = i16::MAX;
        center_sliced_tone(&mut tail, 55.0, None);
        assert_eq!(
            unsafe { whole.process(tail.as_ptr(), tail.len() as u32, true) },
            -1
        );

        let mut offset = 0;
        for count in PARTITIONS {
            unsafe { split.process(tail[offset..].as_ptr(), count as u32, true) };
            offset += count;
        }
        assert_eq!(offset, tail.len());
        assert_eq!(split.decoded(), whole.decoded());
        assert_eq!(split.blanking_samples, whole.blanking_samples);
        for (actual, expected) in split.detectors.iter().zip(&whole.detectors) {
            assert_eq!(actual.counter, expected.counter);
            assert_eq!(actual.peak, expected.peak);
            assert_eq!(actual.z_index, expected.z_index);
            assert_eq!(actual.z, expected.z);
            assert_eq!(actual.dvu, expected.dvu);
            assert_eq!(actual.dvd, expected.dvd);
            assert_eq!(actual.zd, expected.zd);
            assert_eq!(actual.decode, expected.decode);
        }
    }

    #[test]
    fn lowest_ctcss_tone_does_not_match_the_tail_detector() {
        const TONE_INDEX: usize = 0;
        let mut state = ReceiveState::default();
        let mut tone = [0.0_f32; 512];

        activate_selected_detector(&mut state, TONE_INDEX);
        center_sliced_tone(&mut tone, 67.0, None);

        assert_eq!(
            unsafe { state.process(tone.as_ptr(), tone.len() as u32, true) },
            TONE_INDEX as i16
        );
        assert_eq!(state.decoded(), TONE_INDEX as i16);
    }

    #[test]
    fn deployed_100_hz_divider_qualifies_a_center_sliced_tone() {
        let mut state = ReceiveState::default();
        let mut samples = [0.0_f32; SAMPLE_RATE_HZ as usize];

        state.configure(1_u64 << 11, false);
        for (index, sample) in samples.iter_mut().enumerate() {
            let phase = 2.0 * core::f32::consts::PI * 100.0 * index as f32 / SAMPLE_RATE_HZ as f32;

            // The legacy center slicer limits its post-frontend output to
            // this 625-code span.  Feed the canonical-F32 boundary exactly
            // as the USBRadioPlus bridge does.
            // The slicer preserves timing while limiting amplitude, so its
            // normal output is a 625-code square wave rather than a sine.
            *sample = if phase.sin() >= 0.0 { 625.0 } else { -625.0 } / 32_768.0;
        }
        for block in samples.chunks(160) {
            unsafe { state.process(block.as_ptr(), block.len() as u32, true) };
        }
        assert_eq!(state.decoded(), 11);
    }

    #[test]
    fn carrier_loss_and_f32_i16_mapping_preserve_compatibility_inputs() {
        let mut state = ReceiveState::default();
        state.configure(1_u64 << 11, false);
        state.test_prepare_detector(11);
        let samples = [100.0_f32 / 32_768.0];
        assert_eq!(ReceiveState::sample_to_i16(samples[0]), 100);
        assert_eq!(unsafe { state.process(samples.as_ptr(), 1, false) }, -1);
        assert_eq!(state.detectors[11].decode, 0);
    }
}
