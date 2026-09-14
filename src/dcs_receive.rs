//! Legacy-compatible native DCS receive qualification.
//!
//! The decoder retains the established 16 timing hypotheses, Golay correction,
//! slow Q15 DC tracker, and 134.4 Hz turn-off detector.  Its PCM input is
//! canonical F32 that originated as signed-16-bit PCM divided by 32768; that
//! mapping reconstructs every legacy discriminator code exactly.

use std::ptr;

use crate::timer::NATIVE_SAMPLE_RATE_HZ;

const WORD_MASK: u32 = 0x7f_ffff;
const GENERATOR: u32 = 0x0c75;
const CLOCK_SCALE: u32 = 10;
const CLOCK_INCREMENT: u32 = 1_344;
const SYMBOL_COUNT: u32 = 23;
const PHASE_COUNT: usize = 16;
const TURNOFF_FREQUENCY_HZ: f64 = 134.4;
const TURNOFF_WINDOW_MS: u32 = 20;
const TURNOFF_MINIMUM_MS: u32 = 100;
const TURNOFF_MINIMUM_RMS: f64 = 200.0;
const TURNOFF_MINIMUM_COHERENCE: f64 = 0.25;
const DC_TRACKER_SCALE: i64 = 32_768;
const TAU: f64 = std::f64::consts::TAU;

/// State for one fixed DCS receive-symbol timing hypothesis.
#[derive(Clone, Copy, Default)]
struct ReceivePhase {
    word: u32,
    bit_accumulator: u32,
    hold: u32,
    symbols_since_match: u32,
    match_count: u8,
    qualified: bool,
    sample_accumulator: i64,
}

/// Callback-owned DCS receive decoder state.
pub(crate) struct ReceiveState {
    receive_phase: [ReceivePhase; PHASE_COUNT],
    dc_estimate_q15: i64,
    turnoff_coefficient: f64,
    turnoff_one: f64,
    turnoff_two: f64,
    turnoff_energy: f64,
    turnoff_window_samples: u32,
    turnoff_window_length: u32,
    turnoff_consecutive_samples: u32,
    turnoff_minimum_samples: u32,
    turnoff_active: bool,
    syndrome: [u32; 2_048],
    receive_code: i32,
    receive_inverted: bool,
    enabled: bool,
    valid: bool,
}

impl Default for ReceiveState {
    fn default() -> Self {
        let mut state = Self {
            receive_phase: [ReceivePhase::default(); PHASE_COUNT],
            dc_estimate_q15: 0,
            turnoff_coefficient: 0.0,
            turnoff_one: 0.0,
            turnoff_two: 0.0,
            turnoff_energy: 0.0,
            turnoff_window_samples: 0,
            turnoff_window_length: 0,
            turnoff_consecutive_samples: 0,
            turnoff_minimum_samples: 0,
            turnoff_active: false,
            syndrome: syndrome_table(),
            receive_code: -1,
            receive_inverted: false,
            enabled: false,
            valid: false,
        };
        state.reset_phase_bank();
        state
    }
}

/// Return whether a numeric code fits the established three-octal-digit field.
pub(crate) fn code_supported(code: i32) -> bool {
    (0..=0o777).contains(&code)
}

fn remainder(mut word: u32) -> u32 {
    for bit in (11..=22).rev() {
        if word & (1_u32 << bit) != 0 {
            word ^= GENERATOR << (bit - 11);
        }
    }
    word & 0x07ff
}

fn install_error(table: &mut [u32; 2_048], error: u32) {
    table[remainder(error) as usize] = error;
}

fn syndrome_table() -> [u32; 2_048] {
    let mut table = [0_u32; 2_048];
    for first in 0..23 {
        install_error(&mut table, 1_u32 << first);
        for second in (first + 1)..23 {
            install_error(&mut table, (1_u32 << first) | (1_u32 << second));
            for third in (second + 1)..23 {
                install_error(
                    &mut table,
                    (1_u32 << first) | (1_u32 << second) | (1_u32 << third),
                );
            }
        }
    }
    install_error(&mut table, 0);
    table
}

impl ReceiveState {
    /// Replace the configured receive code and clear all runtime qualification.
    pub(crate) fn configure(&mut self, code: i32, inverted: u32) {
        self.receive_code = if code_supported(code) { code } else { -1 };
        self.receive_inverted = inverted != 0;
        self.enabled = self.receive_code >= 0;
        self.reset_phase_bank();
    }

    /// Return whether a configured receive code is currently qualified.
    pub(crate) fn valid(&self) -> bool {
        self.valid
    }

    /// Return whether receive decoding is configured for a supported code.
    pub(crate) fn enabled(&self) -> bool {
        self.enabled
    }

    /// Process one bounded, interleaved canonical-F32 native PCM span.
    ///
    /// # Safety
    ///
    /// `stereo` is readable for `frame_count * 2` canonical F32 samples.  It
    /// represents raw signed-16-bit discriminator PCM divided by 32768.0, and
    /// all access to this state is serialized by its owning native callback.
    pub(crate) unsafe fn process(&mut self, stereo: *const f32, frame_count: u32) -> bool {
        let clock_limit = NATIVE_SAMPLE_RATE_HZ * CLOCK_SCALE;
        for index in 0..frame_count as usize {
            let normalized = unsafe { ptr::read(stereo.add(index * 2)) };
            let sample = legacy_pcm_code(normalized);
            let centered = self.remove_dc(sample);
            let tail_was_active = self.turnoff_active;
            let tail_active = self.process_turnoff_sample(centered);
            if tail_active && !tail_was_active {
                self.clear_qualification();
            }
            for receiver in &mut self.receive_phase {
                receiver.sample_accumulator = receiver
                    .sample_accumulator
                    .wrapping_add(i64::from(centered));
                receiver.bit_accumulator = receiver.bit_accumulator.wrapping_add(CLOCK_INCREMENT);
                if receiver.bit_accumulator < clock_limit {
                    continue;
                }
                receiver.bit_accumulator = receiver.bit_accumulator.wrapping_sub(clock_limit);
                let mut bit = receiver.sample_accumulator >= 0;
                if self.receive_inverted {
                    bit = !bit;
                }
                receiver.sample_accumulator = 0;
                if !tail_active {
                    process_received_symbol(&self.syndrome, self.receive_code, receiver, bit);
                }
            }
        }
        self.valid = self.phase_bank_valid();
        self.valid
    }

    fn reset_phase_bank(&mut self) {
        let clock_limit = u64::from(NATIVE_SAMPLE_RATE_HZ) * u64::from(CLOCK_SCALE);
        self.receive_phase = [ReceivePhase::default(); PHASE_COUNT];
        for (phase, receiver) in self.receive_phase.iter_mut().enumerate() {
            receiver.bit_accumulator =
                ((clock_limit * u64::from(phase as u32)) / PHASE_COUNT as u64) as u32;
        }
        self.dc_estimate_q15 = 0;
        self.reset_turnoff_detector();
        self.valid = false;
    }

    fn reset_turnoff_detector(&mut self) {
        // Stream setup fixes the rate; detector timing is never reconfigured
        // in the sample loop and its windows are always nonzero.
        let phase = TAU * TURNOFF_FREQUENCY_HZ / f64::from(NATIVE_SAMPLE_RATE_HZ);
        self.turnoff_coefficient = 2.0 * phase.cos();
        self.turnoff_one = 0.0;
        self.turnoff_two = 0.0;
        self.turnoff_energy = 0.0;
        self.turnoff_window_samples = 0;
        self.turnoff_window_length = NATIVE_SAMPLE_RATE_HZ * TURNOFF_WINDOW_MS / 1_000;
        self.turnoff_consecutive_samples = 0;
        self.turnoff_minimum_samples = NATIVE_SAMPLE_RATE_HZ * TURNOFF_MINIMUM_MS / 1_000;
        self.turnoff_active = false;
    }

    fn process_turnoff_sample(&mut self, sample: i32) -> bool {
        let current =
            f64::from(sample) + self.turnoff_coefficient * self.turnoff_one - self.turnoff_two;
        self.turnoff_two = self.turnoff_one;
        self.turnoff_one = current;
        self.turnoff_energy += f64::from(sample) * f64::from(sample);
        self.turnoff_window_samples = self.turnoff_window_samples.wrapping_add(1);
        if self.turnoff_window_samples < self.turnoff_window_length {
            return self.turnoff_active;
        }
        let samples = f64::from(self.turnoff_window_samples);
        let power = self.turnoff_one * self.turnoff_one + self.turnoff_two * self.turnoff_two
            - self.turnoff_coefficient * self.turnoff_one * self.turnoff_two;
        let minimum_energy = samples * TURNOFF_MINIMUM_RMS * TURNOFF_MINIMUM_RMS;
        let tone = self.turnoff_energy >= minimum_energy
            && power >= self.turnoff_energy * samples * TURNOFF_MINIMUM_COHERENCE;
        if tone {
            self.turnoff_consecutive_samples = self
                .turnoff_consecutive_samples
                .wrapping_add(self.turnoff_window_samples);
            if self.turnoff_consecutive_samples >= self.turnoff_minimum_samples {
                self.turnoff_active = true;
            }
        } else {
            self.turnoff_consecutive_samples = 0;
            self.turnoff_active = false;
        }
        self.turnoff_one = 0.0;
        self.turnoff_two = 0.0;
        self.turnoff_energy = 0.0;
        self.turnoff_window_samples = 0;
        self.turnoff_active
    }

    fn remove_dc(&mut self, sample: i32) -> i32 {
        let target = i64::from(sample) * DC_TRACKER_SCALE;
        self.dc_estimate_q15 += (target - self.dc_estimate_q15) / DC_TRACKER_SCALE;
        sample - (self.dc_estimate_q15 / DC_TRACKER_SCALE) as i32
    }

    fn clear_qualification(&mut self) {
        for receiver in &mut self.receive_phase {
            receiver.hold = 0;
            receiver.symbols_since_match = 0;
            receiver.match_count = 0;
            receiver.qualified = false;
        }
        self.valid = false;
    }

    fn phase_bank_valid(&self) -> bool {
        self.receive_phase.iter().any(|receiver| receiver.qualified)
    }
}

fn legacy_pcm_code(sample: f32) -> i32 {
    (sample * 32_768.0) as i32
}

fn decode_word(syndrome: &[u32; 2_048], receive_code: i32, received: u32) -> bool {
    let error = syndrome[remainder(received) as usize];
    let data = (received ^ error) & 0x0fff;
    data & 0x0e00 == 0x0800 && (data & 0x01ff) as i32 == receive_code
}

fn process_received_symbol(
    syndrome: &[u32; 2_048],
    receive_code: i32,
    receiver: &mut ReceivePhase,
    bit: bool,
) {
    receiver.symbols_since_match = receiver.symbols_since_match.wrapping_add(1);
    receiver.word = ((receiver.word >> 1) | (u32::from(bit) << 22)) & WORD_MASK;
    if decode_word(syndrome, receive_code, receiver.word) {
        if receiver.match_count != 0 && receiver.symbols_since_match == SYMBOL_COUNT {
            if receiver.match_count < 2 {
                receiver.match_count += 1;
            }
        } else {
            receiver.match_count = 1;
        }
        receiver.symbols_since_match = 0;
        if receiver.match_count >= 2 {
            receiver.qualified = true;
            receiver.hold = SYMBOL_COUNT;
        }
        return;
    }
    if receiver.qualified {
        receiver.hold = receiver.hold.wrapping_sub(1);
        if receiver.hold == 0 {
            receiver.qualified = false;
            receiver.match_count = 0;
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    const WORD_023N: u32 = 0x76_3813;

    /// Existing 023N wire-word fixture, independent of the transmitter.
    fn discriminator_pcm(frames: usize, inverted: bool) -> Vec<f32> {
        (0..frames)
            .flat_map(|index| {
                let symbol = (index as u64 * u64::from(CLOCK_INCREMENT)
                    / u64::from(NATIVE_SAMPLE_RATE_HZ * CLOCK_SCALE))
                    % u64::from(SYMBOL_COUNT);
                let high = ((WORD_023N >> symbol) & 1 != 0) ^ inverted;
                let sample = if high { 0.375 } else { -0.375 };
                // The right channel is deliberately unrelated to discriminator input.
                [sample, -sample]
            })
            .collect()
    }

    #[test]
    fn golay_corrects_every_three_bit_error_and_rejects_other_codes() {
        let syndrome = syndrome_table();
        assert!(decode_word(&syndrome, 0o023, WORD_023N));
        for first in 0..23 {
            let one = 1 << first;
            assert!(decode_word(&syndrome, 0o023, WORD_023N ^ one));
            for second in first + 1..23 {
                let two = one | (1 << second);
                assert!(decode_word(&syndrome, 0o023, WORD_023N ^ two));
                for third in second + 1..23 {
                    assert!(decode_word(
                        &syndrome,
                        0o023,
                        WORD_023N ^ two ^ (1 << third)
                    ));
                }
            }
        }
        assert!(!decode_word(&syndrome, 0o025, WORD_023N));
        assert!(!decode_word(&syndrome, 0o023, 0));
    }

    #[test]
    fn consecutive_words_qualify_refresh_hold_and_expire_after_missing_symbols() {
        let syndrome = syndrome_table();
        let mut receiver = ReceivePhase::default();
        for word in 1..=3 {
            for symbol in 0..SYMBOL_COUNT {
                process_received_symbol(
                    &syndrome,
                    0o023,
                    &mut receiver,
                    WORD_023N & (1 << symbol) != 0,
                );
            }
            assert_eq!(receiver.match_count, word.min(2));
            assert_eq!(receiver.qualified, word >= 2);
            assert_eq!(receiver.symbols_since_match, 0);
            if word >= 2 {
                assert_eq!(receiver.hold, SYMBOL_COUNT);
            }
        }
        // A non-code shift register cannot refresh qualification during dropout.
        receiver.word = 0;
        for remaining in (0..SYMBOL_COUNT).rev() {
            process_received_symbol(&syndrome, 0o023, &mut receiver, false);
            assert_eq!(receiver.hold, remaining);
            assert_eq!(receiver.qualified, remaining != 0);
        }
        assert_eq!(receiver.match_count, 0);
    }

    #[test]
    fn native_pcm_acquires_both_polarities_and_configuration_resets_state() {
        for inverted in [false, true] {
            let mut state = ReceiveState::default();
            assert!(!state.enabled());
            assert!(!state.valid());
            state.configure(0o023, u32::from(inverted));
            assert!(state.enabled());
            let pcm = discriminator_pcm(NATIVE_SAMPLE_RATE_HZ as usize, inverted);
            // An odd block size exercises retained fractional clocks at boundaries.
            for block in pcm.chunks(514) {
                unsafe { state.process(block.as_ptr(), (block.len() / 2) as u32) };
            }
            assert!(state.valid(), "polarity {inverted}");
            assert!(state.phase_bank_valid());
            state.configure(0o777, 0);
            assert!(state.enabled());
            assert!(!state.valid());
            assert_eq!(state.dc_estimate_q15, 0);
            assert!(state.receive_phase.iter().all(|phase| !phase.qualified));
            for invalid in [-1, 0o1000] {
                state.configure(invalid, 0);
                assert!(!state.enabled());
                assert_eq!(state.receive_code, -1);
                assert!(!unsafe { state.process(ptr::null(), 0) });
            }
        }
        assert!(code_supported(0));
        assert!(code_supported(0o777));
    }

    #[test]
    fn turnoff_requires_sustained_coherent_energy_and_clears_qualified_dcs() {
        let mut state = ReceiveState::default();
        state.configure(0o023, 0);
        let pcm = discriminator_pcm(NATIVE_SAMPLE_RATE_HZ as usize, false);
        assert!(unsafe { state.process(pcm.as_ptr(), NATIVE_SAMPLE_RATE_HZ) });
        // Start the detector at an exact window boundary without resetting DCS.
        state.reset_turnoff_detector();
        let window = state.turnoff_window_length as usize;
        for block in 0..6 {
            let tone: Vec<f32> = (0..window)
                .flat_map(|index| {
                    let phase = TAU * TURNOFF_FREQUENCY_HZ * (block * window + index) as f64
                        / f64::from(NATIVE_SAMPLE_RATE_HZ);
                    [(phase.sin() * 0.375) as f32, 0.0]
                })
                .collect();
            unsafe { state.process(tone.as_ptr(), window as u32) };
            assert_eq!(state.turnoff_active, block >= 4);
        }
        assert!(!state.valid());
        assert!(
            state
                .receive_phase
                .iter()
                .all(|phase| { !phase.qualified && phase.hold == 0 && phase.match_count == 0 })
        );
        let silence = vec![0.0; window * 2];
        unsafe { state.process(silence.as_ptr(), window as u32) };
        assert!(!state.turnoff_active);
        assert_eq!(state.turnoff_consecutive_samples, 0);
        // Strong off-frequency input exceeds the RMS gate but not coherence.
        state.reset_turnoff_detector();
        for index in 0..window {
            let sample = (12_000.0
                * (TAU * 1_000.0 * index as f64 / f64::from(NATIVE_SAMPLE_RATE_HZ)).sin())
                as i32;
            assert!(!state.process_turnoff_sample(sample));
        }
        // Pure silence separately fails the energy gate.
        for _ in 0..window {
            assert!(!state.process_turnoff_sample(0));
        }
    }

    /// A matching word at the wrong spacing restarts, rather than advances,
    /// legacy two-word qualification.
    #[test]
    fn nonconsecutive_matching_word_restarts_qualification() {
        let state = ReceiveState::default();
        let mut receiver = ReceivePhase {
            word: (WORD_023N << 1) & WORD_MASK,
            symbols_since_match: 1,
            match_count: 1,
            ..ReceivePhase::default()
        };

        process_received_symbol(
            &state.syndrome,
            0o023,
            &mut receiver,
            WORD_023N & (1_u32 << 22) != 0,
        );
        assert_eq!(receiver.match_count, 1);
        assert_eq!(receiver.symbols_since_match, 0);
        assert!(!receiver.qualified);
    }
}
