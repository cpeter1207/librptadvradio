//! Legacy-compatible DCS transmit signaling.
//!
//! DCS is a 23-bit Golay word sent least-significant bit first at 134.4 baud.
//! The fractional integer clock, word construction, and separate 134.4 Hz
//! turn-off oscillator intentionally match the established native transmitter.

use std::ptr;

const DCS_WIRE_GENERATOR: u32 = 0x08ea;
const DCS_CLOCK_SCALE: u32 = 10;
const DCS_CLOCK_INCREMENT: u32 = 1_344;
const DCS_SYMBOL_COUNT: u32 = 23;
const DCS_TURNOFF_FREQUENCY_HZ: f64 = 134.4;
const TAU: f64 = std::f64::consts::TAU;

/// Callback-owned state for one native DCS transmitter.
#[derive(Default)]
pub(crate) struct TransmitState {
    code: i32,
    inverted: bool,
    phase: u32,
    bit_accumulator: u32,
    tail_phase_radians: f64,
}

/// Per-block DCS generation controls supplied by the portable caller.
#[derive(Clone, Copy)]
pub(crate) struct GenerateConfig {
    pub(crate) peak: f32,
    pub(crate) enabled: u32,
    pub(crate) turnoff: u32,
}

/// Return whether a numeric DCS value fits the legacy three-octal-digit field.
pub(crate) fn code_supported(code: i32) -> bool {
    (0..=0o777).contains(&code)
}

/// Preserve the legacy configuration outcome for a possibly invalid code.
pub(crate) fn configured_code(code: i32) -> i32 {
    if code_supported(code) { code } else { -1 }
}

impl TransmitState {
    /// Apply a compatibility configuration change without disturbing phase.
    pub(crate) fn configure(&mut self, code: i32, inverted: u32) {
        self.code = configured_code(code);
        self.inverted = inverted != 0;
        self.bit_accumulator = 0;
    }
}

/// Encode one TIA/ETSI DCS word in least-significant-bit-first wire layout.
fn wire_word(code: i32) -> u32 {
    let data = (code as u32 & 0x01ff) | 0x0800;
    let mut remainder = data;
    for _ in 0..12 {
        remainder <<= 1;
        if remainder & 0x1000 != 0 {
            remainder ^= DCS_WIRE_GENERATOR;
        }
    }
    data | ((remainder & 0x0ffe) << 11)
}

/// Render one bounded DCS block without allocating or interacting with I/O.
///
/// # Safety
///
/// `output` is writable for `frame_count` canonical f32 samples when that
/// count is nonzero.  The caller serializes access to all retained state by
/// using the radio object from one native tick at a time.
pub(crate) unsafe fn generate(
    state: &mut TransmitState,
    output: *mut f32,
    frame_count: u32,
    config: GenerateConfig,
) {
    if config.enabled == 0 || config.peak <= 0.0 {
        if frame_count != 0 {
            // A positive-zero byte fill retains the canonical silent f32 value.
            unsafe { ptr::write_bytes(output, 0, frame_count as usize) };
        }
        return;
    }
    if config.turnoff != 0 {
        let step = TAU * DCS_TURNOFF_FREQUENCY_HZ / f64::from(crate::timer::NATIVE_SAMPLE_RATE_HZ);
        for index in 0..frame_count as usize {
            unsafe {
                output
                    .add(index)
                    .write((f64::from(config.peak) * state.tail_phase_radians.sin()) as f32)
            };
            state.tail_phase_radians += step;
            // Deliberately subtract once, matching the established generator.
            if state.tail_phase_radians >= TAU {
                state.tail_phase_radians -= TAU;
            }
        }
        return;
    }

    let word = wire_word(state.code);
    let clock_limit = crate::timer::NATIVE_SAMPLE_RATE_HZ * DCS_CLOCK_SCALE;
    for index in 0..frame_count as usize {
        let mut bit = (word >> state.phase) & 1 != 0;
        if state.inverted {
            bit = !bit;
        }
        unsafe {
            output
                .add(index)
                .write(if bit { config.peak } else { -config.peak })
        };
        state.bit_accumulator = state.bit_accumulator.wrapping_add(DCS_CLOCK_INCREMENT);
        if state.bit_accumulator >= clock_limit {
            state.bit_accumulator = state.bit_accumulator.wrapping_sub(clock_limit);
            state.phase = (state.phase + 1) % DCS_SYMBOL_COUNT;
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn known_wire_word_and_fractional_clock_survive_partitioning_and_inversion() {
        const WORD_023N: u32 = 0x76_3813;
        assert_eq!(wire_word(0o023), WORD_023N);
        assert_eq!(configured_code(-1), -1);
        assert_eq!(configured_code(0o1000), -1);
        assert_eq!(configured_code(0), 0);
        assert_eq!(configured_code(0o777), 0o777);
        for inverted in [0, 1] {
            let mut whole = TransmitState::default();
            let mut split = TransmitState::default();
            whole.configure(0o023, inverted);
            split.configure(0o023, inverted);
            let config = GenerateConfig {
                peak: 0.25,
                enabled: 1,
                turnoff: 0,
            };
            let mut expected = [0.0; 9217];
            let mut actual = [0.0; 9217];
            unsafe { generate(&mut whole, expected.as_mut_ptr(), 9217, config) };
            for block in actual.chunks_mut(791) {
                unsafe { generate(&mut split, block.as_mut_ptr(), block.len() as u32, config) };
            }
            assert_eq!(actual, expected);
            for (index, sample) in actual.iter().enumerate() {
                let bit = (index as u64 * 1344 / 480_000) % 23;
                let high = (WORD_023N & (1 << bit) != 0) ^ (inverted != 0);
                assert_eq!(*sample, if high { 0.25 } else { -0.25 });
            }
            assert_eq!(whole.phase, (9217 * 1344 / 480_000) % 23);
            assert_eq!(whole.bit_accumulator, (9217 * 1344) % 480_000);
            assert_eq!(whole.phase, split.phase);
            assert_eq!(whole.bit_accumulator, split.bit_accumulator);
            let phase = split.phase;
            split.configure(0o777, inverted);
            assert_eq!(split.phase, phase);
            assert_eq!(split.bit_accumulator, 0);
        }
    }

    #[test]
    fn inactive_and_nonpositive_peak_clear_output_without_advancing_clocks() {
        for (enabled, peak) in [(0, 0.25), (1, 0.0), (1, -0.25)] {
            let mut state = TransmitState {
                phase: 3,
                bit_accumulator: 42,
                tail_phase_radians: 0.75,
                ..TransmitState::default()
            };
            let config = GenerateConfig {
                peak,
                enabled,
                turnoff: 1,
            };
            let mut output = [1.0; 3];
            unsafe {
                generate(&mut state, output.as_mut_ptr(), 3, config);
                generate(&mut state, ptr::null_mut(), 0, config);
            }
            assert!(output.iter().all(|sample| sample.to_bits() == 0));
            assert_eq!(state.phase, 3);
            assert_eq!(state.bit_accumulator, 42);
            assert_eq!(state.tail_phase_radians, 0.75);
        }
    }

    #[test]
    fn turnoff_sine_wraps_without_advancing_the_digital_word() {
        let mut state = TransmitState {
            phase: 3,
            bit_accumulator: 42,
            tail_phase_radians: 0.25,
            ..TransmitState::default()
        };
        let config = GenerateConfig {
            peak: 0.25,
            enabled: 1,
            turnoff: 1,
        };
        let mut output = [0.0; 1000];
        unsafe { generate(&mut state, output.as_mut_ptr(), 1000, config) };
        for (index, sample) in output.iter().enumerate() {
            let expected = (0.25 * (0.25 + TAU * 134.4 * index as f64 / 48_000.0).sin()) as f32;
            assert!((sample - expected).abs() < 1e-7);
        }
        assert_eq!(state.phase, 3);
        assert_eq!(state.bit_accumulator, 42);
        assert!((0.0..TAU).contains(&state.tail_phase_radians));
    }
}
