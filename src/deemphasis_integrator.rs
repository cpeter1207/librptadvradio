//! Legacy-compatible receiver deemphasis integrator on canonical F32 PCM.
//!
//! USBRadioPlus uses this one-pole fixed-point stage after the receiver
//! high-pass filter when flat discriminator audio requires deemphasis. The
//! portable primitive retains its signed-32 recursive accumulator, signed
//! truncation, wrapping arithmetic, and signed-16 output narrowing exactly.

use super::{RADIO_INVALID_ARGUMENT, audio_meter::f32_to_pcm_code};

const PCM_SCALE: f32 = 32_768.0;
const LEGACY_FEEDBACK_DIVISOR: i32 = 32_768;
const LEGACY_OUTPUT_DIVISOR: i32 = 8_192;
const LEGACY_GAIN_DIVISOR: i32 = 256;

/// Caller-owned recursive state for one legacy receiver-deemphasis stage.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Signed-32 recursive accumulator in historical fixed-point units.
    pub accumulator: i32,
}

/// Borrowed PCM and fixed-point controls for one deemphasis operation.
pub(crate) struct Request<'a> {
    /// Readable canonical F32 input.
    pub(crate) input: *const f32,
    /// Writable canonical F32 output after historical i16 narrowing.
    pub(crate) output: *mut f32,
    /// Number of scalar samples to process.
    pub(crate) sample_count: u32,
    /// Historical feed-forward coefficient.
    pub(crate) output_coefficient: i16,
    /// Historical recursive feedback coefficient.
    pub(crate) feedback_coefficient: i16,
    /// Historical Q8 output gain.
    pub(crate) output_gain: i32,
    /// Caller-owned recursive accumulator.
    pub(crate) state: &'a mut State,
}

/// Process one F32 span with the historical fixed-point deemphasis rules.
///
/// Every input sample is quantized into the legacy signed-16 domain before
/// processing. Arithmetic uses explicit wrapping signed-32 multiplication and
/// addition, then Rust's truncation-toward-zero signed division, matching the
/// deployed compatibility stage. Each resulting output narrows to signed-16
/// before it is mapped back to canonical F32.
///
/// The complete input span is validated before caller state or output changes.
/// This preserves a C caller's exact fallback when a direct F32 request is
/// malformed.
///
/// # Safety
///
/// For nonzero `sample_count`, `input` and `output` designate readable and
/// writable F32 spans of that size. `state` is a valid mutable state object.
pub unsafe fn process(request: Request<'_>) -> Result<(), i32> {
    let Request {
        input,
        output,
        sample_count,
        output_coefficient,
        feedback_coefficient,
        output_gain,
        state,
    } = request;
    let sample_count = sample_count as usize;
    if sample_count != 0 && (input.is_null() || output.is_null()) {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // Reject direct malformed F32 input before modifying state or output. The
    // compatibility bridge supplies exact i16/32768 mappings, but this keeps
    // the public primitive deterministic for future canonical-F32 callers.
    for index in 0..sample_count {
        f32_to_pcm_code(unsafe { input.add(index).read() })?;
    }

    let mut next = *state;
    let output_coefficient = i32::from(output_coefficient);
    let feedback_coefficient = i32::from(feedback_coefficient);
    for index in 0..sample_count {
        let input_code = f32_to_pcm_code(unsafe { input.add(index).read() })?;
        let feedback =
            next.accumulator.wrapping_mul(feedback_coefficient) / LEGACY_FEEDBACK_DIVISOR;
        next.accumulator = input_code.wrapping_add(feedback);
        let filtered = next.accumulator.wrapping_mul(output_coefficient) / LEGACY_OUTPUT_DIVISOR;
        let scaled = filtered.wrapping_mul(output_gain) / LEGACY_GAIN_DIVISOR;
        let output_code = scaled as i16;

        unsafe { output.add(index).write(f32::from(output_code) / PCM_SCALE) };
    }

    *state = next;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{Request, State, process};

    #[test]
    fn null_input_or_output_preserves_the_accumulator() {
        for missing_input in [true, false] {
            let input = [0.0];
            let mut output = [0.25];
            let mut state = State { accumulator: 123 };
            assert_eq!(
                unsafe {
                    process(Request {
                        input: if missing_input {
                            core::ptr::null()
                        } else {
                            input.as_ptr()
                        },
                        output: if missing_input {
                            output.as_mut_ptr()
                        } else {
                            core::ptr::null_mut()
                        },
                        sample_count: 1,
                        output_coefficient: 6878,
                        feedback_coefficient: 25889,
                        output_gain: 256,
                        state: &mut state,
                    })
                },
                Err(crate::RADIO_INVALID_ARGUMENT)
            );
            assert_eq!(state.accumulator, 123);
            assert_eq!(output, [0.25]);
        }
    }

    fn samples(codes: &[i16]) -> Vec<f32> {
        codes
            .iter()
            .map(|code| f32::from(*code) / 32_768.0)
            .collect()
    }

    fn codes(samples: &[f32]) -> Vec<i16> {
        samples
            .iter()
            .map(|sample| (sample * 32_768.0) as i16)
            .collect()
    }

    fn run(
        input: &[i16],
        output_coefficient: i16,
        feedback_coefficient: i16,
        output_gain: i32,
        state: &mut State,
    ) -> Vec<i16> {
        let input = samples(input);
        let mut output = vec![0.0_f32; input.len()];

        unsafe {
            process(Request {
                input: input.as_ptr(),
                output: output.as_mut_ptr(),
                sample_count: input.len() as u32,
                output_coefficient,
                feedback_coefficient,
                output_gain,
                state,
            })
        }
        .expect("valid deemphasis PCM");
        codes(&output)
    }

    #[test]
    fn deployed_coefficients_match_fixed_point_reference_vector() {
        let mut state = State {
            accumulator: -12_345,
        };
        let output = run(
            &[0, 20_000, -10_000, 32_767, -32_768, 1_000, -1],
            6_878,
            25_889,
            256,
            &mut state,
        );

        assert_eq!(output, [-8188, 10322, -240, 27321, -5926, -3842, -3036]);
        assert_eq!(state.accumulator, -3617);
    }

    #[test]
    fn division_wrapping_and_i16_narrowing_match_legacy_rules() {
        let mut signed = State { accumulator: -17 };
        assert_eq!(
            run(&[-1, 1, -32_768, 32_767], -6_878, 25_889, -257, &mut signed),
            [-11, -8, -27624, 5792]
        );
        assert_eq!(signed.accumulator, 6873);

        let mut wrapped = State {
            accumulator: i32::MAX,
        };
        assert_eq!(
            run(
                &[32_767, -32_768, 12_345],
                32_767,
                -32_768,
                i32::MAX,
                &mut wrapped,
            ),
            [-511, 1023, 832]
        );
        assert_eq!(wrapped.accumulator, -53_191);
    }

    #[test]
    fn every_partition_matches_one_complete_span() {
        let input = [0, 20_000, -10_000, 32_767, -32_768, 1_000, -1];
        let initial = State {
            accumulator: -12_345,
        };
        let mut whole_state = initial;
        let whole = run(&input, 6_878, 25_889, 256, &mut whole_state);

        for split in 1..input.len() {
            let mut state = initial;
            let mut output = run(&input[..split], 6_878, 25_889, 256, &mut state);
            output.extend(run(&input[split..], 6_878, 25_889, 256, &mut state));
            assert_eq!(output, whole, "split {split}");
            assert_eq!(state, whole_state, "split {split}");
        }
    }

    #[test]
    fn zero_span_and_invalid_f32_do_not_change_state_or_output() {
        let mut state = State { accumulator: 123 };
        let original = state;
        unsafe {
            process(Request {
                input: core::ptr::null(),
                output: core::ptr::null_mut(),
                sample_count: 0,
                output_coefficient: 6_878,
                feedback_coefficient: 25_889,
                output_gain: 256,
                state: &mut state,
            })
        }
        .expect("zero span is a valid no-op");
        assert_eq!(state, original);

        let input = [f32::NAN];
        let mut output = [0.25_f32];
        assert!(
            unsafe {
                process(Request {
                    input: input.as_ptr(),
                    output: output.as_mut_ptr(),
                    sample_count: 1,
                    output_coefficient: 6_878,
                    feedback_coefficient: 25_889,
                    output_gain: 256,
                    state: &mut state,
                })
            }
            .is_err()
        );
        assert_eq!(state, original);
        assert_eq!(output, [0.25]);
    }
}
