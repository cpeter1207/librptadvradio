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

    /// Return the independent DCS turn-off oscillator phase.
    pub(crate) fn tail_phase_radians(&self) -> f64 {
        self.tail_phase_radians
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

/// Expose a known vector to module-local tests without expanding the C ABI.
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn test_wire_word(code: i32) -> u32 {
    wire_word(code)
}
