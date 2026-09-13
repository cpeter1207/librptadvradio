//! Legacy-compatible mono unit-rate FIR on canonical F32 PCM.
//!
//! USBRadioPlus uses this primitive for the receiver high-pass filter and the
//! CTCSS low-pass filter.  Their coefficients and signed-16 history remain
//! caller-owned so a configuration reload can replace either table without
//! allocating or caching state in the shared object.

use super::{RADIO_INVALID_ARGUMENT, audio_meter::f32_to_pcm_code};

const PCM_SCALE: f32 = 32_768.0;
const LEGACY_GAIN_DIVISOR: i32 = 256;

/// Fixed-point controls for one bounded legacy FIR span.
#[derive(Clone, Copy)]
struct Controls {
    /// Active history entries and coefficient taps.
    nx: usize,
    /// Historical Q8 input gain.
    input_gain: i32,
    /// Historical post-accumulator Q8 output gain.
    output_gain: i32,
    /// Historical FIR accumulation normalization divisor.
    calc_adjust: i32,
}

/// Process one signed-16 sample through the bounded legacy FIR core.
///
/// `nx` deliberately selects both the active history entries and coefficient
/// taps.  It is not inferred from either slice length: the retained C stage
/// uses `nx` even when its allocated coefficient storage is larger.
fn process_code(
    input: i16,
    history: &mut [i16],
    coefficients: &[i16],
    controls: Controls,
) -> Result<i16, i32> {
    if controls.nx == 0
        || history.len() < controls.nx
        || coefficients.len() < controls.nx
        || controls.calc_adjust == 0
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    if controls.nx > 1 {
        // `copy_within` has memmove semantics, matching the overlapping
        // legacy history shift while retaining the unused tail unchanged.
        history.copy_within(0..controls.nx - 1, 1);
    }
    let scaled_input = i32::from(input).wrapping_mul(controls.input_gain) / LEGACY_GAIN_DIVISOR;
    history[0] = scaled_input as i16;

    let mut accumulator = 0_i64;
    for tap in 0..controls.nx {
        accumulator = accumulator
            .wrapping_add(i64::from(coefficients[tap]).wrapping_mul(i64::from(history[tap])));
    }
    let sample = (accumulator / i64::from(controls.calc_adjust))
        .wrapping_mul(i64::from(controls.output_gain))
        / i64::from(LEGACY_GAIN_DIVISOR);
    Ok(sample.clamp(-32_767, 32_767) as i16)
}

/// Process one bounded signed-16 mono/unit-rate span with legacy arithmetic.
///
/// This is the safe fixed-point core used by the F32 ABI adapter.  It mutates
/// only the first `nx` history entries and reads only the first `nx`
/// coefficients, preserving the generic stage's explicit tap-count contract.
/// It performs no allocation, locking, I/O, interpolation, mixing, or
/// amplitude detection.
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
fn process_codes(
    input: &[i16],
    output: &mut [i16],
    history: &mut [i16],
    coefficients: &[i16],
    controls: Controls,
) -> Result<(), i32> {
    if input.len() != output.len() {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    for (input, output) in input.iter().zip(output.iter_mut()) {
        *output = process_code(*input, history, coefficients, controls)?;
    }
    Ok(())
}

/// Process one bounded canonical-F32 mono/unit-rate span through the core.
///
/// Input is validated before the first history shift so a non-finite sample
/// leaves every mutable caller span unchanged.  The compatibility bridge uses
/// exact signed-16/32768 mappings, while other finite F32 input follows the
/// ABI's shared PCM quantizer.
fn process_normalized(
    input: &[f32],
    output: &mut [f32],
    history: &mut [i16],
    coefficients: &[i16],
    controls: Controls,
) -> Result<(), i32> {
    if input.len() != output.len() {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    if controls.nx == 0
        || history.len() < controls.nx
        || coefficients.len() < controls.nx
        || controls.calc_adjust == 0
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    for sample in input {
        f32_to_pcm_code(*sample)?;
    }
    for (input, output) in input.iter().zip(output.iter_mut()) {
        let input_code = f32_to_pcm_code(*input)? as i16;
        let sample = process_code(input_code, history, coefficients, controls)?;
        *output = f32::from(sample) / PCM_SCALE;
    }
    Ok(())
}

/// Borrowed PCM, history, and fixed-point controls for one mono FIR span.
pub(crate) struct Request {
    /// Readable canonical F32 input.
    pub(crate) input: *const f32,
    /// Writable canonical F32 output after legacy signed-16 clipping.
    pub(crate) output: *mut f32,
    /// Number of scalar samples to process.
    pub(crate) sample_count: u32,
    /// Caller-owned legacy signed-16 history.
    pub(crate) history: *mut i16,
    /// Number of history entries and active coefficient taps.
    pub(crate) history_count: u32,
    /// Readable fixed-point coefficient table with `history_count` entries.
    pub(crate) coefficients: *const i16,
    /// Historical Q8 input gain.
    pub(crate) input_gain: i32,
    /// Historical post-accumulator Q8 output gain.
    pub(crate) output_gain: i32,
    /// Historical FIR accumulation normalization divisor.
    pub(crate) calc_adjust: i32,
}

/// Run one mono, unit-rate FIR span with the historical fixed-point rules.
///
/// Input is quantized to signed-16 PCM before the historical Q8 scaling and
/// history assignment.  Coefficient products accumulate in signed-64 space;
/// the division/multiply/division order and the asymmetric `[-32767, 32767]`
/// rail clamp match the retained USBRadioPlus stage exactly.  Callers that
/// need interpolation, output mixing, or amplitude detection continue through
/// their compatibility implementation.
///
/// The complete F32 input is validated before history or output changes.  The
/// primitive allocates nothing, locks nothing, and performs no I/O.
///
/// # Safety
///
/// For nonzero `sample_count`, `input` and `output` designate non-overlapping
/// spans of that length. `history` and `coefficients` each designate
/// `history_count` signed-16 entries; all writable spans are disjoint from
/// readable coefficient storage, and `history_count` is nonzero.
pub unsafe fn process(request: Request) -> Result<(), i32> {
    let Request {
        input,
        output,
        sample_count,
        history,
        history_count,
        coefficients,
        input_gain,
        output_gain,
        calc_adjust,
    } = request;
    let sample_count = sample_count as usize;
    let history_count = history_count as usize;

    if sample_count == 0 {
        return Ok(());
    }
    if input.is_null()
        || output.is_null()
        || history.is_null()
        || coefficients.is_null()
        || history_count == 0
        || calc_adjust == 0
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // SAFETY: the caller's FFI contract establishes disjoint, valid spans of
    // exactly the lengths checked above.  Arithmetic and mutation after this
    // boundary are entirely slice-based.
    let input = unsafe { core::slice::from_raw_parts(input, sample_count) };
    // SAFETY: see the input span above; output is a disjoint writable span.
    let output = unsafe { core::slice::from_raw_parts_mut(output, sample_count) };
    // SAFETY: the validated caller-owned history has `history_count` entries.
    let history = unsafe { core::slice::from_raw_parts_mut(history, history_count) };
    // SAFETY: the validated coefficient storage is disjoint from mutable spans.
    let coefficients = unsafe { core::slice::from_raw_parts(coefficients, history_count) };

    process_normalized(
        input,
        output,
        history,
        coefficients,
        Controls {
            nx: history_count,
            input_gain,
            output_gain,
            calc_adjust,
        },
    )
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{Controls, Request, process, process_codes};

    #[test]
    fn each_safe_core_precondition_rejects_before_mutation() {
        let controls = Controls {
            nx: 2,
            input_gain: 256,
            output_gain: 256,
            calc_adjust: 1,
        };
        for field in 0..4 {
            let mut controls = controls;
            let mut history = [7, 8];
            let coefficients = [1, 1];
            let mut output = [9.0];
            if field == 0 {
                controls.nx = 0;
            }
            if field == 3 {
                controls.calc_adjust = 0;
            }
            let history_len = if field == 1 { 1 } else { 2 };
            let coefficient_len = if field == 2 { 1 } else { 2 };
            assert_eq!(
                super::process_code(
                    1,
                    &mut history[..history_len],
                    &coefficients[..coefficient_len],
                    controls
                ),
                Err(crate::RADIO_INVALID_ARGUMENT)
            );
            assert_eq!(
                super::process_normalized(
                    &[0.0],
                    &mut output,
                    &mut history[..history_len],
                    &coefficients[..coefficient_len],
                    controls
                ),
                Err(crate::RADIO_INVALID_ARGUMENT)
            );
            assert_eq!(history, [7, 8]);
            assert_eq!(output, [9.0]);
        }
        assert_eq!(
            super::process_normalized(&[], &mut [9.0], &mut [7, 8], &[1, 1], controls),
            Err(crate::RADIO_INVALID_ARGUMENT)
        );
        for field in 0..5 {
            let input = [0.0];
            let mut output = [9.0];
            let mut history = [7];
            let coefficients = [1];
            assert_eq!(
                unsafe {
                    process(Request {
                        input: input.as_ptr(),
                        output: if field == 0 {
                            core::ptr::null_mut()
                        } else {
                            output.as_mut_ptr()
                        },
                        sample_count: 1,
                        history: if field == 1 {
                            core::ptr::null_mut()
                        } else {
                            history.as_mut_ptr()
                        },
                        history_count: if field == 3 { 0 } else { 1 },
                        coefficients: if field == 2 {
                            core::ptr::null()
                        } else {
                            coefficients.as_ptr()
                        },
                        input_gain: 256,
                        output_gain: 256,
                        calc_adjust: if field == 4 { 0 } else { 1 },
                    })
                },
                Err(crate::RADIO_INVALID_ARGUMENT)
            );
            assert_eq!(output, [9.0]);
            assert_eq!(history, [7]);
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
        coefficients: &[i16],
        input_gain: i32,
        output_gain: i32,
        calc_adjust: i32,
        history: &mut [i16],
    ) -> Vec<i16> {
        let input = samples(input);
        let mut output = vec![0.0_f32; input.len()];

        unsafe {
            process(Request {
                input: input.as_ptr(),
                output: output.as_mut_ptr(),
                sample_count: input.len() as u32,
                history: history.as_mut_ptr(),
                history_count: history.len() as u32,
                coefficients: coefficients.as_ptr(),
                input_gain,
                output_gain,
                calc_adjust,
            })
        }
        .expect("valid FIR PCM");
        codes(&output)
    }

    #[test]
    fn fixed_point_history_and_gain_vector_matches_legacy_rules() {
        let mut history = [1200_i16, -700, 300];
        let output = run(
            &[-32_768, -1234, 0, 2345, 32_767],
            &[10_000, -5000, 3000],
            300,
            280,
            10_000,
            &mut history,
        );

        assert_eq!(output, [28794, -16027, 9693, 2530, -31185]);
        assert_eq!(history, [-27138, 2748, 0]);
    }

    #[test]
    fn every_partition_matches_one_complete_span() {
        let input = [-32_768, -1234, 0, 2345, 32_767, -17, 99];
        let coefficients = [10_000, -5000, 3000];
        let initial_history = [1200_i16, -700, 300];
        let mut whole_history = initial_history;
        let whole = run(&input, &coefficients, 300, 280, 10_000, &mut whole_history);

        for split in 1..input.len() {
            let mut history = initial_history;
            let mut output = run(
                &input[..split],
                &coefficients,
                300,
                280,
                10_000,
                &mut history,
            );
            output.extend(run(
                &input[split..],
                &coefficients,
                300,
                280,
                10_000,
                &mut history,
            ));
            assert_eq!(output, whole, "split {split}");
            assert_eq!(history, whole_history, "split {split}");
        }
    }

    #[test]
    fn both_legacy_rails_are_asymmetric() {
        let mut high_history = [0_i16];
        assert_eq!(
            run(&[32_767], &[32_767], 256, 256, 1, &mut high_history),
            [32_767]
        );

        let mut low_history = [0_i16];
        assert_eq!(
            run(&[-32_768], &[32_767], 256, 256, 1, &mut low_history),
            [-32_767]
        );
    }

    #[test]
    fn nx_selects_active_history_and_taps_not_coefficient_storage_length() {
        let input = [100_i16, -50];
        let mut output = [0_i16; 2];
        let mut history = [11_i16, 22, 33, 44];
        let coefficients = [2_i16, 3, 1_000, 2_000];

        process_codes(
            &input,
            &mut output,
            &mut history,
            &coefficients,
            Controls {
                nx: 2,
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 1,
            },
        )
        .expect("bounded nx FIR");

        assert_eq!(output, [233, 200]);
        assert_eq!(history, [-50, 100, 33, 44]);
    }

    #[test]
    fn zero_span_and_invalid_input_preserve_history_and_output() {
        let mut history = [17_i16, -3];
        let original_history = history;
        unsafe {
            process(Request {
                input: core::ptr::null(),
                output: core::ptr::null_mut(),
                sample_count: 0,
                history: core::ptr::null_mut(),
                history_count: 0,
                coefficients: core::ptr::null(),
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 1,
            })
        }
        .expect("zero span is a no-op");

        let input = [f32::NAN];
        let coefficients = [1_i16, 2];
        let mut output = [0.25_f32];
        assert!(
            unsafe {
                process(Request {
                    input: input.as_ptr(),
                    output: output.as_mut_ptr(),
                    sample_count: 1,
                    history: history.as_mut_ptr(),
                    history_count: history.len() as u32,
                    coefficients: coefficients.as_ptr(),
                    input_gain: 256,
                    output_gain: 256,
                    calc_adjust: 1,
                })
            }
            .is_err()
        );
        assert_eq!(history, original_history);
        assert_eq!(output, [0.25]);
    }
}
