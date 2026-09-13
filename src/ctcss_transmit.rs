//! Legacy-compatible, phase-continuous CTCSS transmit signaling.
//!
//! This module deliberately retains the legacy oscillator quantization and
//! measured calibration tables.  Those values define transmitter deviation on
//! deployed radio interfaces, so replacing them with idealized calculations
//! would be an observable behavior change.

use std::ptr;

const CTCSS_RATE_HZ: f64 = 48_000.0;
const PI: f64 = std::f64::consts::PI;
const TAU: f64 = 2.0 * PI;
const LEGACY_SINE_STEPS: i64 = 256;

/// Reference CTCSS frequencies in hertz.
pub(crate) const FREQUENCIES: [f64; 38] = [
    67.0, 71.9, 74.4, 77.0, 79.7, 82.5, 85.4, 88.5, 91.5, 94.8, 97.4, 100.0, 103.5, 107.2, 110.9,
    114.8, 118.8, 123.0, 127.3, 131.8, 136.5, 141.3, 146.2, 151.4, 156.7, 162.2, 167.9, 173.8,
    179.9, 186.2, 192.8, 203.5, 210.7, 218.1, 225.7, 233.6, 241.8, 250.3,
];

/// Measured XPMR reference peaks for the legacy 215 Hz detector filter.
const PEAK_215: [i64; 38] = [
    16_573, 16_619, 16_638, 16_670, 16_701, 16_734, 16_768, 16_812, 16_843, 16_943, 16_915, 16_952,
    16_981, 17_020, 17_049, 17_083, 17_104, 17_101, 17_096, 17_065, 17_015, 16_960, 16_824, 16_653,
    16_467, 16_173, 15_855, 15_456, 14_954, 14_429, 13_719, 12_475, 11_519, 10_591, 9_428, 8_287,
    7_070, 5_876,
];

/// Measured XPMR reference peaks for the legacy 250 Hz detector filter.
const PEAK_250: [i64; 38] = [
    17_435, 17_558, 17_614, 17_684, 17_751, 17_819, 17_887, 17_965, 18_024, 18_157, 18_148, 18_204,
    18_258, 18_318, 18_365, 18_417, 18_451, 18_465, 18_476, 18_470, 18_444, 18_425, 18_329, 18_218,
    18_106, 17_903, 17_698, 17_443, 17_118, 16_813, 16_338, 15_540, 14_926, 14_348, 13_524, 12_731,
    11_836, 10_905,
];

/// Measured positive/negative peak differences for the 215 Hz table.
const BIAS_215: [i64; 38] = [
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, 0, -1, 0, 0, 0, -2, 0, 0, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, -1, 0, 0, 0, 0, 0, 0,
];

/// Measured positive/negative peak differences for the 250 Hz table.
const BIAS_250: [i64; 38] = [
    0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, 0, 0, 0, 0, 0, 0,
    -1, 0, 0, 0, 0, 0, 0,
];

/// Return whether a frequency is an exact signaling-table entry.
pub(crate) fn frequency_supported(frequency_hz: f32) -> bool {
    FREQUENCIES
        .iter()
        .any(|reference| frequency_hz == *reference as f32)
}

/// Reproduce the XPMR 8 kHz fixed-increment oscillator frequency.
pub(crate) fn legacy_frequency(frequency_hz: f64) -> f64 {
    // The compatibility interface only passes supported finite CTCSS tones.
    // The intermediate signed-integer arithmetic intentionally matches XPMR's
    // truncation at every division.
    let frequency_tenths = (frequency_hz * 10.0) as i64;
    let step = LEGACY_SINE_STEPS * frequency_tenths * 128 / 8_000 / 10;
    step as f64 * 8_000.0 / (LEGACY_SINE_STEPS * 128) as f64
}

fn closest_frequency(frequency_hz: f64) -> usize {
    let mut best = 0_usize;
    for (index, reference) in FREQUENCIES.iter().enumerate().skip(1) {
        if (*reference - frequency_hz).abs() < (FREQUENCIES[best] - frequency_hz).abs() {
            best = index;
        }
    }
    best
}

/// Return the measured reference peak in signed-16 PCM codes.
pub(crate) fn legacy_peak(frequency_hz: f64, filter_250: bool) -> f64 {
    let index = closest_frequency(frequency_hz);
    if filter_250 {
        PEAK_250[index] as f64
    } else {
        PEAK_215[index] as f64
    }
}

/// Calculate legacy calibrated amplitude and DC bias in signed-16 PCM codes.
pub(crate) fn legacy_scaled_levels(
    frequency_hz: f64,
    filter_250: bool,
    tone_gain_q8: i32,
    output_gain_q8: i32,
) -> (f64, f64) {
    let index = closest_frequency(frequency_hz);
    let maximum = if filter_250 {
        PEAK_250[index]
    } else {
        PEAK_215[index]
    };
    let difference = if filter_250 {
        BIAS_250[index]
    } else {
        BIAS_215[index]
    };
    let mut positive = if difference > 0 {
        maximum
    } else {
        maximum + difference
    };
    let mut negative = if difference < 0 {
        maximum
    } else {
        maximum - difference
    };

    // Preserve XPMR's two separately truncated Q8 gain stages.
    positive = positive * i64::from(tone_gain_q8) / 256;
    positive = positive * i64::from(output_gain_q8) / 256;
    negative = negative * i64::from(tone_gain_q8) / 256;
    negative = negative * i64::from(output_gain_q8) / 256;
    (
        (positive + negative) as f64 / 2.0,
        (positive - negative) as f64 / 2.0,
    )
}

/// Calculate the maximum absolute legacy calibrated level in PCM codes.
pub(crate) fn legacy_scaled_peak(
    frequency_hz: f64,
    filter_250: bool,
    tone_gain_q8: i32,
    output_gain_q8: i32,
) -> f64 {
    let (amplitude, bias) =
        legacy_scaled_levels(frequency_hz, filter_250, tone_gain_q8, output_gain_q8);
    amplitude + bias.abs()
}

/// Render a continuous sine wave in canonical f32 PCM without allocating.
///
/// # Safety
///
/// `output` is writable for `frame_count` f32 samples when that count is
/// nonzero.  The caller serializes access to `phase` through its radio object.
pub(crate) unsafe fn generate(
    phase: &mut f64,
    output: *mut f32,
    frame_count: u32,
    frequency_hz: f64,
    peak: f32,
    enabled: u32,
    phase_shift_degrees: f64,
) {
    if enabled == 0 || frequency_hz <= 0.0 || peak <= 0.0 {
        // A positive-zero byte fill retains the canonical silent f32 value.
        if frame_count != 0 {
            unsafe { ptr::write_bytes(output, 0, frame_count as usize) };
        }
        return;
    }
    if phase_shift_degrees != 0.0 {
        *phase += TAU * phase_shift_degrees / 360.0;
        *phase %= TAU;
    }
    let step = TAU * frequency_hz / CTCSS_RATE_HZ;
    let mut sine = phase.sin();
    let mut cosine = phase.cos();
    let sine_step = step.sin();
    let cosine_step = step.cos();
    for index in 0..frame_count as usize {
        unsafe { output.add(index).write(peak * sine as f32) };
        let next_sine = sine * cosine_step + cosine * sine_step;
        let next_cosine = cosine * cosine_step - sine * sine_step;
        sine = next_sine;
        cosine = next_cosine;
    }
    *phase = (*phase + step * f64::from(frame_count)) % TAU;
}
