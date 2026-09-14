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

/// Reproduce the XPMR 8 kHz fixed-increment oscillator frequency.
pub(crate) fn legacy_frequency(frequency_hz: f64) -> f64 {
    // The compatibility interface only passes supported finite CTCSS tones.
    // The intermediate signed-integer arithmetic intentionally matches XPMR's
    // truncation at every division.
    let frequency_tenths = (frequency_hz * 10.0) as i64;
    let step = LEGACY_SINE_STEPS * frequency_tenths * 128 / 8_000 / 10;
    step as f64 * 8_000.0 / (LEGACY_SINE_STEPS * 128) as f64
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn disabled_nonpositive_frequency_and_nonpositive_peak_silence_without_advancing() {
        for (enabled, frequency, peak) in [
            (0, 114.8, 0.25),
            (1, 0.0, 0.25),
            (1, -1.0, 0.25),
            (1, 114.8, 0.0),
            (1, 114.8, -0.25),
        ] {
            let mut phase = 0.75;
            let mut output = [1.0; 3];
            unsafe {
                generate(
                    &mut phase,
                    output.as_mut_ptr(),
                    3,
                    frequency,
                    peak,
                    enabled,
                    120.0,
                );
                generate(
                    &mut phase,
                    ptr::null_mut(),
                    0,
                    frequency,
                    peak,
                    enabled,
                    120.0,
                );
            }
            assert!(output.iter().all(|sample| sample.to_bits() == 0));
            assert_eq!(phase, 0.75);
        }
    }

    #[test]
    fn legacy_frequency_and_phase_shift_preserve_the_calibrated_waveform() {
        assert_eq!(legacy_frequency(67.0), 66.89453125);
        assert_eq!(legacy_frequency(114.8), 114.74609375);
        assert_eq!(legacy_frequency(250.3), 250.244140625);
        let frequency = legacy_frequency(114.8);
        let initial = TAU - 0.01;
        let shifted = (initial + TAU / 3.0) % TAU;
        let mut whole_phase = initial;
        let mut split_phase = initial;
        let mut whole = [0.0; 960];
        let mut split = [0.0; 960];
        unsafe {
            generate(
                &mut whole_phase,
                whole.as_mut_ptr(),
                960,
                frequency,
                0.25,
                1,
                120.0,
            );
        }
        for (block_index, block) in split.chunks_mut(137).enumerate() {
            unsafe {
                generate(
                    &mut split_phase,
                    block.as_mut_ptr(),
                    block.len() as u32,
                    frequency,
                    0.25,
                    1,
                    if block_index == 0 { 120.0 } else { 0.0 },
                );
            }
        }
        for (index, (whole, split)) in whole.iter().zip(split).enumerate() {
            let expected =
                (0.25 * (shifted + TAU * frequency * index as f64 / 48_000.0).sin()) as f32;
            assert!((whole - expected).abs() < 1e-7);
            assert!((whole - split).abs() < 1e-7);
        }
        assert!((whole_phase - split_phase).abs() < 1e-12);
        let phase = whole_phase;
        unsafe {
            generate(
                &mut whole_phase,
                ptr::null_mut(),
                0,
                frequency,
                0.25,
                1,
                0.0,
            )
        };
        assert_eq!(whole_phase, phase);
    }
}
