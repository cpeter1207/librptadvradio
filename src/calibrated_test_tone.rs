//! Legacy-compatible calibrated transmitter test-tone generation.
//!
//! USBRadioPlus uses this fixed one-kilohertz source to calibrate transmitter
//! voice deviation.  Its amplitude and one-subtraction phase wrap are part of
//! that calibration contract and therefore remain intentionally fixed here.

const NATIVE_RATE_HZ: f64 = 48_000.0;
const FREQUENCY_HZ: f64 = 1_000.0;
const PEAK_PCM_CODES: f64 = 7_518.0;
const PCM_CODE_SCALE: f64 = 32_767.0;
const TAU: f64 = std::f64::consts::TAU;

/// Callback-owned state for the calibrated one-kilohertz transmitter source.
#[derive(Default)]
pub(crate) struct State {
    phase_radians: f64,
}

impl State {
    /// Replace one bounded canonical program span or reset when disabled.
    ///
    /// # Safety
    ///
    /// `program` is writable for `frame_count` canonical f32 samples whenever
    /// that count is nonzero.  The caller serializes access through one radio
    /// object from its owning native tick.
    pub(crate) unsafe fn render(&mut self, program: *mut f32, frame_count: u32, enabled: u32) {
        if enabled == 0 {
            // Legacy tuning removes the source and primes its next key-up to
            // begin at the same zero crossing; program audio remains intact.
            self.phase_radians = 0.0;
            return;
        }
        let step = TAU * FREQUENCY_HZ / NATIVE_RATE_HZ;
        for index in 0..frame_count as usize {
            // Calculate in f64 before the sole canonical-f32 boundary. This
            // exactly mirrors the old double PCM-code program workspace.
            let sample = PEAK_PCM_CODES * self.phase_radians.sin() / PCM_CODE_SCALE;
            unsafe { program.add(index).write(sample as f32) };
            self.phase_radians += step;
            // Deliberately subtract once rather than using remainder: this is
            // the deployed oscillator's exact phase-wrap behavior.
            if self.phase_radians >= TAU {
                self.phase_radians -= TAU;
            }
        }
    }

    /// Return the callback-owned phase without changing it.
    pub(crate) fn phase_radians(&self) -> f64 {
        self.phase_radians
    }
}
