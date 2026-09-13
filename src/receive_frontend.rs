//! Legacy-compatible native discriminator receive frontend.
//!
//! The compatibility adapter retains coefficient ownership, configuration, and
//! unusual diagnostic or VOX modes.  This bounded primitive owns the ordinary
//! DSP-squelch arithmetic only: it consumes native stereo F32 PCM, preserves
//! the established byte-count history shift, emits decimated mono F32 PCM,
//! and advances the fixed RSSI and MICOR state once per native sample.

use core::mem::size_of;

use super::{RADIO_INVALID_ARGUMENT, audio_meter::f32_to_pcm_code, micor_squelch};

const PCM_SCALE: f32 = 32_768.0;
const LEGACY_GAIN_DIVISOR: i64 = 256;
const RSSI_DIVISOR: f64 = 16.0;

/// Caller-owned state for one ordinary discriminator receive frontend.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct State {
    /// Current native-to-baseband decimator phase.
    pub decimator: i16,
    /// Current MICOR comparator result; nonzero means receiver closed.
    pub comparator_output: i16,
    /// Most recently completed RSSI calibration value in legacy PCM units.
    pub rssi_peak: i16,
    /// ABI-compatible scalar alignment reserved for later extensions.
    pub reserved: u16,
    /// Sum of squared filtered-noise samples in the current RSSI window.
    pub rssi_power: i64,
    /// Native samples accumulated in the current RSSI window.
    pub rssi_samples: u32,
    /// Retained sample-clocked MICOR comparator state.
    pub micor_squelch: micor_squelch::State,
}

/// Borrowed buffers and fixed controls for one native frontend span.
pub(crate) struct Request {
    /// Readable interleaved canonical-F32 native stereo input.
    pub(crate) input: *const f32,
    /// Writable canonical-F32 decimated mono output.
    pub(crate) baseband_output: *mut f32,
    /// Capacity of [`Self::baseband_output`] in scalar samples.
    pub(crate) baseband_output_capacity: u32,
    /// Writable per-native-frame carrier gate; one means receiver open.
    pub(crate) carrier_gate: *mut u8,
    /// Capacity of [`Self::carrier_gate`] in native frames.
    pub(crate) carrier_gate_capacity: u32,
    /// Native input frames to consume.
    pub(crate) native_frame_count: u32,
    /// Caller-owned signed-16 filter history.
    pub(crate) history: *mut i16,
    /// Entries in [`Self::history`] and the baseband coefficient table.
    pub(crate) history_count: u32,
    /// Readable signed-16 baseband FIR coefficient table.
    pub(crate) baseband_coefficients: *const i16,
    /// Historical baseband FIR normalization divisor.
    pub(crate) baseband_calc_adjust: i32,
    /// Historical Q8 baseband output gain.
    pub(crate) baseband_output_gain: i32,
    /// Readable signed-16 discriminator-noise FIR coefficients.
    pub(crate) noise_coefficients: *const i16,
    /// Entries in [`Self::noise_coefficients`].
    pub(crate) noise_coefficient_count: u32,
    /// Historical noise-filter normalization divisor.
    pub(crate) noise_divisor: i32,
    /// Whether this span advances the legacy noise-squelch and RSSI path.
    ///
    /// VOX receive mode retains native decimation but deliberately bypasses
    /// this detector, matching the C frontend's `doNoise` branch.
    pub(crate) noise_squelch: bool,
    /// Native frames per emitted baseband sample.
    pub(crate) decimate: u32,
    /// Native samples per fixed RSSI calibration window.
    pub(crate) calibration_window: u32,
    /// Legacy DSP-squelch open threshold.
    pub(crate) open_level: u32,
    /// Legacy DSP-squelch hysteresis.
    pub(crate) hysteresis: u32,
    /// Caller-owned phase, RSSI, and comparator state.
    pub(crate) state: *mut State,
}

/// One completed native frontend result.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Result {
    /// Decimated baseband samples written to the output span.
    pub(crate) baseband_output_count: u32,
    /// Nonzero when one fixed RSSI window completed in this span.
    pub(crate) rssi_updated: bool,
}

/// Return the greatest possible baseband output count for one valid span.
fn maximum_baseband_output_count(native_frame_count: usize, decimate: usize) -> usize {
    native_frame_count.saturating_add(decimate - 1) / decimate
}

/// Shift legacy signed-16 filter history with the deployed byte count.
///
/// The historical C frontend passed `nx - 1` directly as `memmove`'s byte
/// count.  That differs from a normal signed-16 element shift and is preserved
/// exactly for behavioral compatibility.
unsafe fn legacy_history_shift(history: *mut i16, history_count: usize) {
    if history_count > 1 {
        let bytes = history.cast::<u8>();

        // SAFETY: validation established a writable history span with at least
        // `history_count` signed-16 entries. `ptr::copy` has memmove semantics
        // for the intentionally overlapping legacy byte ranges.
        unsafe { core::ptr::copy(bytes, bytes.add(size_of::<i16>()), history_count - 1) };
    }
}

/// Run one native discriminator frontend span with historical fixed-point rules.
///
/// The full selected input span is validated before history, output, carrier
/// gates, or state changes.  Once validation succeeds the calculation has no
/// fallible operations, so a compatibility bridge can retain its C fallback
/// without a partially changed frontend state.  The operation allocates
/// nothing, locks nothing, and performs no I/O.
///
/// # Safety
///
/// Nonzero native spans require every pointer in [`Request`] to identify the
/// documented readable or writable storage.  The history span has
/// `history_count` signed-16 entries; baseband and noise tables have their
/// stated counts; state is a valid mutable [`State`].
pub unsafe fn process(request: Request) -> core::result::Result<Result, i32> {
    let Request {
        input,
        baseband_output,
        baseband_output_capacity,
        carrier_gate,
        carrier_gate_capacity,
        native_frame_count,
        history,
        history_count,
        baseband_coefficients,
        baseband_calc_adjust,
        baseband_output_gain,
        noise_coefficients,
        noise_coefficient_count,
        noise_divisor,
        noise_squelch,
        decimate,
        calibration_window,
        open_level,
        hysteresis,
        state,
    } = request;
    let native_frame_count =
        usize::try_from(native_frame_count).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    let history_count = usize::try_from(history_count).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    let noise_coefficient_count =
        usize::try_from(noise_coefficient_count).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    let decimate = usize::try_from(decimate).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    let calibration_window =
        usize::try_from(calibration_window).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    let baseband_output_capacity =
        usize::try_from(baseband_output_capacity).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    let carrier_gate_capacity =
        usize::try_from(carrier_gate_capacity).map_err(|_| RADIO_INVALID_ARGUMENT)?;

    if history.is_null()
        || baseband_coefficients.is_null()
        || (noise_squelch && noise_coefficients.is_null())
        || state.is_null()
        || history_count == 0
        || (noise_squelch
            && (noise_coefficient_count == 0 || noise_coefficient_count > history_count))
        || baseband_calc_adjust == 0
        || (noise_squelch && noise_divisor == 0)
        || decimate == 0
        || decimate > i32::MAX as usize
        || calibration_window == 0
        || baseband_output_capacity < maximum_baseband_output_count(native_frame_count, decimate)
        || carrier_gate_capacity < native_frame_count
        || (native_frame_count != 0
            && (input.is_null() || baseband_output.is_null() || carrier_gate.is_null()))
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // The legacy frontend consumes only the left native channel. Validate it
    // before modifying any caller-owned output or state; the right channel is
    // intentionally ignored just as it is in the C implementation.
    for index in 0..native_frame_count {
        // SAFETY: validation confirmed an interleaved stereo span for every
        // native input frame.
        let sample = unsafe { input.add(index * 2).read() };
        f32_to_pcm_code(sample)?;
    }

    // SAFETY: validation established a valid state pointer. Retain a local
    // copy until all output processing has completed.
    let mut next = unsafe { state.read() };
    let mut output_count = 0_usize;
    let mut rssi_updated = false;
    let baseband_calc_adjust = i64::from(baseband_calc_adjust);
    let baseband_output_gain = i64::from(baseband_output_gain);

    for index in 0..native_frame_count {
        // SAFETY: validation covered every selected input sample.
        let input_code = f32_to_pcm_code(unsafe { input.add(index * 2).read() })?;
        unsafe { legacy_history_shift(history, history_count) };
        // SAFETY: the front history always has at least one validated entry.
        unsafe { history.write(input_code as i16) };

        if noise_squelch {
            let mut noise_accumulator = 0_i32;
            for tap in 0..noise_coefficient_count {
                // SAFETY: validation established both table spans and the noise
                // count does not exceed the history count.
                let coefficient = unsafe { noise_coefficients.add(tap).read() };
                let sample = unsafe { history.add(tap).read() };
                noise_accumulator = noise_accumulator
                    .wrapping_add(i32::from(coefficient).wrapping_mul(i32::from(sample)));
            }
            let noise = noise_accumulator / noise_divisor;
            next.rssi_power = next
                .rssi_power
                .wrapping_add(i64::from(noise) * i64::from(noise));
            next.rssi_samples = next.rssi_samples.wrapping_add(1);
            if usize::try_from(next.rssi_samples).expect("u32 fits usize") == calibration_window {
                next.rssi_peak = ((next.rssi_power as f64).sqrt() / RSSI_DIVISOR) as i16;
                next.rssi_power = 0;
                next.rssi_samples = 0;
                rssi_updated = true;
            }
            let closed = micor_squelch::update(
                &mut next.micor_squelch,
                next.comparator_output != 0,
                f64::from(noise) * f64::from(noise) * 3.75,
                open_level,
                hysteresis,
            );
            next.comparator_output = if closed { 1 } else { 0 };
            // SAFETY: validation established the full native gate span.
            unsafe { carrier_gate.add(index).write(u8::from(!closed)) };
        }

        let mut decimator = i32::from(next.decimator).wrapping_sub(1);
        if decimator <= 0 {
            decimator = decimate as i32;
            let mut accumulator = 0_i64;

            for tap in 0..history_count {
                // SAFETY: validation established matching baseband coefficient
                // and history spans of `history_count` entries.
                let coefficient = unsafe { baseband_coefficients.add(tap).read() };
                let sample = unsafe { history.add(tap).read() };
                accumulator = accumulator
                    .wrapping_add(i64::from(coefficient).wrapping_mul(i64::from(sample)));
            }
            let sample = (accumulator / baseband_calc_adjust).wrapping_mul(baseband_output_gain)
                / LEGACY_GAIN_DIVISOR;
            let sample = sample.clamp(-32_767, 32_767) as i16;
            // SAFETY: capacity validation bounds output_count for this exact
            // decimator sequence.
            unsafe {
                baseband_output
                    .add(output_count)
                    .write(f32::from(sample) / PCM_SCALE)
            };
            output_count += 1;
        }
        next.decimator = decimator as i16;
    }

    // SAFETY: all caller-owned result spans were validated before processing.
    unsafe { state.write(next) };
    Ok(Result {
        baseband_output_count: output_count as u32,
        rssi_updated,
    })
}

#[cfg(test)]
mod tests {
    use super::{Request, State, process};
    use crate::micor_squelch;

    fn stereo_left(codes: &[i16]) -> Vec<f32> {
        let mut output = Vec::with_capacity(codes.len() * 2);

        for sample in codes {
            output.push(f32::from(*sample) / 32_768.0);
            output.push(0.25);
        }
        output
    }

    fn codes(samples: &[f32]) -> Vec<i16> {
        samples
            .iter()
            .map(|sample| (sample * 32_768.0) as i16)
            .collect()
    }

    fn run(
        input: &[i16],
        history: &mut [i16],
        state: &mut State,
        baseband: &[i16],
        noise: &[i16],
        decimate: u32,
    ) -> (Vec<i16>, Vec<u8>, super::Result) {
        let input = stereo_left(input);
        let mut output = vec![0.0_f32; input.len() / 2];
        let mut gate = vec![0_u8; input.len() / 2];
        let result = unsafe {
            process(Request {
                input: input.as_ptr(),
                baseband_output: output.as_mut_ptr(),
                baseband_output_capacity: output.len() as u32,
                carrier_gate: gate.as_mut_ptr(),
                carrier_gate_capacity: gate.len() as u32,
                native_frame_count: gate.len() as u32,
                history: history.as_mut_ptr(),
                history_count: history.len() as u32,
                baseband_coefficients: baseband.as_ptr(),
                baseband_calc_adjust: 1,
                baseband_output_gain: 256,
                noise_coefficients: noise.as_ptr(),
                noise_coefficient_count: noise.len() as u32,
                noise_divisor: 1,
                noise_squelch: true,
                decimate,
                calibration_window: 6,
                open_level: 1_000,
                hysteresis: 100,
                state,
            })
        }
        .expect("valid frontend request");
        (
            codes(&output[..result.baseband_output_count as usize]),
            gate,
            result,
        )
    }

    #[test]
    fn byte_shift_decimation_and_rssi_window_match_legacy_contract() {
        let mut history = [0x1234_i16, -0x2345, 0x3456, -0x4567];
        let mut state = State {
            decimator: 3,
            comparator_output: 1,
            ..State::default()
        };
        let (output, gate, result) = run(
            &[100, -200, 300, -400, 500, -600],
            &mut history,
            &mut state,
            &[1, 2, -3, 4],
            &[2, -1],
            3,
        );

        assert_eq!(output, [-32_767, -32_767]);
        assert_eq!(gate.len(), 6);
        assert!(gate.iter().all(|sample| *sample <= 1));
        assert!(result.rssi_updated);
        assert_eq!(state.rssi_samples, 0);
        assert_eq!(state.rssi_power, 0);
        assert_eq!(history, [-600, 500, 13_424, -17_767]);
    }

    #[test]
    fn callback_partitions_preserve_pcm_gates_and_state() {
        let input = [
            -32_768, -7_000, -100, 0, 100, 7_000, 32_767, 4_000, -4_000, 1_234, -1_234,
        ];
        let baseband = [4_i16, -3, 2, -1];
        let noise = [2_i16, -1, 1];
        let initial_history = [17_i16, -23, 31, -37];
        let initial_state = State {
            decimator: 2,
            comparator_output: 1,
            micor_squelch: micor_squelch::State {
                settling_samples: 480,
                ..micor_squelch::State::default()
            },
            ..State::default()
        };

        let mut whole_history = initial_history;
        let mut whole_state = initial_state;
        let (whole_output, whole_gate, whole_result) = run(
            &input,
            &mut whole_history,
            &mut whole_state,
            &baseband,
            &noise,
            3,
        );

        for split in 1..input.len() {
            let mut history = initial_history;
            let mut state = initial_state;
            let (mut output, mut gate, first_result) = run(
                &input[..split],
                &mut history,
                &mut state,
                &baseband,
                &noise,
                3,
            );
            let (second_output, second_gate, second_result) = run(
                &input[split..],
                &mut history,
                &mut state,
                &baseband,
                &noise,
                3,
            );

            output.extend(second_output);
            gate.extend(second_gate);
            assert_eq!(output, whole_output, "output split {split}");
            assert_eq!(gate, whole_gate, "gate split {split}");
            assert_eq!(history, whole_history, "history split {split}");
            assert_eq!(state, whole_state, "state split {split}");
            assert_eq!(
                u32::from(first_result.rssi_updated) + u32::from(second_result.rssi_updated),
                u32::from(whole_result.rssi_updated),
                "RSSI split {split}"
            );
        }
    }

    #[test]
    fn invalid_input_preserves_all_mutable_frontend_storage() {
        let input = [f32::NAN, 0.0];
        let mut output = [0.25_f32];
        let mut gate = [0x5a_u8];
        let mut history = [17_i16];
        let initial_history = history;
        let mut state = State {
            decimator: 1,
            comparator_output: 1,
            ..State::default()
        };
        let initial_state = state;
        let baseband = [1_i16];
        let noise = [1_i16];

        assert!(
            unsafe {
                process(Request {
                    input: input.as_ptr(),
                    baseband_output: output.as_mut_ptr(),
                    baseband_output_capacity: 1,
                    carrier_gate: gate.as_mut_ptr(),
                    carrier_gate_capacity: 1,
                    native_frame_count: 1,
                    history: history.as_mut_ptr(),
                    history_count: 1,
                    baseband_coefficients: baseband.as_ptr(),
                    baseband_calc_adjust: 1,
                    baseband_output_gain: 256,
                    noise_coefficients: noise.as_ptr(),
                    noise_coefficient_count: 1,
                    noise_divisor: 1,
                    noise_squelch: true,
                    decimate: 1,
                    calibration_window: 1,
                    open_level: 0,
                    hysteresis: 0,
                    state: &mut state,
                })
            }
            .is_err()
        );
        assert_eq!(output, [0.25]);
        assert_eq!(gate, [0x5a]);
        assert_eq!(history, initial_history);
        assert_eq!(state, initial_state);
    }

    #[test]
    fn zero_span_is_a_valid_state_preserving_no_op() {
        let mut state = State {
            decimator: 3,
            comparator_output: 1,
            rssi_power: 77,
            rssi_samples: 5,
            ..State::default()
        };
        let expected = state;
        let mut history = [12_i16];
        let coefficients = [1_i16];

        let result = unsafe {
            process(Request {
                input: core::ptr::null(),
                baseband_output: core::ptr::null_mut(),
                baseband_output_capacity: 0,
                carrier_gate: core::ptr::null_mut(),
                carrier_gate_capacity: 0,
                native_frame_count: 0,
                history: history.as_mut_ptr(),
                history_count: 1,
                baseband_coefficients: coefficients.as_ptr(),
                baseband_calc_adjust: 1,
                baseband_output_gain: 256,
                noise_coefficients: coefficients.as_ptr(),
                noise_coefficient_count: 1,
                noise_divisor: 1,
                noise_squelch: true,
                decimate: 6,
                calibration_window: 960,
                open_level: 1,
                hysteresis: 0,
                state: &mut state,
            })
        }
        .expect("zero span");

        assert_eq!(result, super::Result::default());
        assert_eq!(state, expected);
        assert_eq!(history, [12]);
    }

    #[test]
    fn vox_frontend_branch_decimates_without_touching_noise_detector_state() {
        let input = stereo_left(&[1_000, -2_000, 3_000]);
        let mut output = [0.0_f32; 2];
        let mut gate = [0x5a_u8; 3];
        let mut history = [11_i16, -22];
        let mut state = State {
            decimator: 1,
            comparator_output: 1,
            rssi_peak: 77,
            rssi_power: 88,
            rssi_samples: 5,
            micor_squelch: micor_squelch::State {
                settling_samples: 99,
                ..micor_squelch::State::default()
            },
            ..State::default()
        };
        let expected_detector = state;
        let baseband = [1_i16, 1];

        let result = unsafe {
            process(Request {
                input: input.as_ptr(),
                baseband_output: output.as_mut_ptr(),
                baseband_output_capacity: output.len() as u32,
                carrier_gate: gate.as_mut_ptr(),
                carrier_gate_capacity: gate.len() as u32,
                native_frame_count: 3,
                history: history.as_mut_ptr(),
                history_count: history.len() as u32,
                baseband_coefficients: baseband.as_ptr(),
                baseband_calc_adjust: 1,
                baseband_output_gain: 256,
                noise_coefficients: core::ptr::null(),
                noise_coefficient_count: 0,
                noise_divisor: 0,
                noise_squelch: false,
                decimate: 2,
                calibration_window: 960,
                open_level: 0,
                hysteresis: 0,
                state: &mut state,
            })
        }
        .expect("valid VOX frontend");

        assert_eq!(result.baseband_output_count, 2);
        assert!(!result.rssi_updated);
        assert_eq!(gate, [0x5a; 3]);
        assert_eq!(state.comparator_output, expected_detector.comparator_output);
        assert_eq!(state.rssi_peak, expected_detector.rssi_peak);
        assert_eq!(state.rssi_power, expected_detector.rssi_power);
        assert_eq!(state.rssi_samples, expected_detector.rssi_samples);
        assert_eq!(state.micor_squelch, expected_detector.micor_squelch);
        assert_eq!(state.decimator, 2);
        assert_ne!(history, [11, -22]);
    }
}
