//! Legacy-compatible decaying peak envelope on canonical F32 PCM.
//!
//! USBRadioPlus uses this compact detector for VOX qualification and tuning
//! measurements.  The portable primitive retains the established signed-16
//! extrema, decay, and threshold behavior while keeping the core PCM boundary
//! canonical F32.

use super::{RADIO_INVALID_ARGUMENT, audio_meter::f32_to_pcm_code};

const PCM_SCALE: f32 = 32_768.0;
const ENVELOPE_DECAY_NUMERATOR: i32 = 32_700;
const ENVELOPE_DECAY_DENOMINATOR: i32 = 32_768;

/// Caller-owned state for one legacy-compatible envelope detector.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Current positive envelope extremum in legacy PCM codes.
    pub maximum: i16,
    /// Current negative envelope extremum in legacy PCM codes.
    pub minimum: i16,
    /// Most recent half peak-to-peak envelope in legacy PCM codes.
    pub peak: i16,
    /// Samples remaining before positive-envelope decay.
    pub upper_decay_counter: i32,
    /// Samples remaining before negative-envelope decay.
    pub lower_decay_counter: i32,
}

/// Measure one F32 PCM span with the historical signed-16 envelope rules.
///
/// The input must remain readable for `sample_count` scalar samples. `output`
/// may be null; when present it receives the peak after each corresponding
/// input sample. Input validation completes before mutating either `state` or
/// output so malformed F32 input cannot leave a partial detector update.
///
/// # Safety
///
/// Non-null input and output pointers must designate the stated number of
/// readable or writable F32 elements, respectively.
pub unsafe fn measure(
    input: *const f32,
    output: *mut f32,
    sample_count: u32,
    decay_factor: i32,
    threshold: i16,
    state: &mut State,
) -> Result<bool, i32> {
    let sample_count = usize::try_from(sample_count).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    if sample_count != 0 && input.is_null() {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // Verify the complete span before advancing caller-owned state. The C
    // compatibility path reaches this primitive only through exact i16/F32
    // conversion, but a direct F32 caller still gets atomic failure behavior.
    for index in 0..sample_count {
        let sample = unsafe { input.add(index).read() };
        f32_to_pcm_code(sample)?;
    }

    let mut next = *state;
    let mut peak = 0_i16;
    for index in 0..sample_count {
        let sample = unsafe { input.add(index).read() };
        let sample = f32_to_pcm_code(sample)? as i16;

        if sample > next.maximum {
            next.maximum = sample;
            next.upper_decay_counter = decay_factor;
        } else {
            next.upper_decay_counter = next.upper_decay_counter.wrapping_sub(1);
            if next.upper_decay_counter <= 0 {
                next.upper_decay_counter = decay_factor;
                next.maximum = ((i32::from(next.maximum) * ENVELOPE_DECAY_NUMERATOR)
                    / ENVELOPE_DECAY_DENOMINATOR) as i16;
            }
        }

        if sample < next.minimum {
            next.minimum = sample;
            next.lower_decay_counter = decay_factor;
        } else {
            next.lower_decay_counter = next.lower_decay_counter.wrapping_sub(1);
            if next.lower_decay_counter <= 0 {
                next.lower_decay_counter = decay_factor;
                next.minimum = ((i32::from(next.minimum) * ENVELOPE_DECAY_NUMERATOR)
                    / ENVELOPE_DECAY_DENOMINATOR) as i16;
            }
        }

        peak = ((i32::from(next.maximum) - i32::from(next.minimum)) / 2) as i16;
        if !output.is_null() {
            unsafe { output.add(index).write(f32::from(peak) / PCM_SCALE) };
        }
    }

    next.peak = peak;
    *state = next;
    Ok(peak >= threshold)
}

#[cfg(test)]
mod tests {
    use super::{State, measure};

    fn samples(codes: &[i16]) -> Vec<f32> {
        codes
            .iter()
            .map(|code| f32::from(*code) / 32_768.0)
            .collect()
    }

    #[test]
    fn decaying_envelope_matches_legacy_integer_steps() {
        let input = samples(&[100, -100, 0, 0]);
        let mut output = [0.0_f32; 4];
        let mut state = State::default();

        assert!(
            !unsafe { measure(input.as_ptr(), output.as_mut_ptr(), 4, 1, 100, &mut state) }
                .expect("valid PCM")
        );
        assert_eq!(
            output,
            [
                50.0 / 32_768.0,
                99.0 / 32_768.0,
                98.0 / 32_768.0,
                97.0 / 32_768.0,
            ]
        );
        assert_eq!(
            state,
            State {
                maximum: 97,
                minimum: -98,
                peak: 97,
                upper_decay_counter: 1,
                lower_decay_counter: 1,
            }
        );
    }

    #[test]
    fn partitioning_and_rails_preserve_the_same_state_and_output() {
        let input = samples(&[i16::MIN, i16::MAX, 0, -500, 500, 0, -1, 1]);
        let mut whole_output = [0.0_f32; 8];
        let mut split_output = [0.0_f32; 8];
        let mut whole = State::default();
        let mut split = State::default();

        let whole_result = unsafe {
            measure(
                input.as_ptr(),
                whole_output.as_mut_ptr(),
                input.len() as u32,
                3,
                1,
                &mut whole,
            )
        }
        .expect("valid whole PCM");
        let first_result = unsafe {
            measure(
                input.as_ptr(),
                split_output.as_mut_ptr(),
                3,
                3,
                1,
                &mut split,
            )
        }
        .expect("valid first PCM partition");
        let second_result = unsafe {
            measure(
                input.as_ptr().wrapping_add(3),
                split_output.as_mut_ptr().wrapping_add(3),
                5,
                3,
                1,
                &mut split,
            )
        }
        .expect("valid second PCM partition");

        assert_eq!(whole_output, split_output);
        assert_eq!(whole, split);
        assert_eq!(whole_result, second_result);
        assert!(first_result);
    }

    #[test]
    fn invalid_f32_does_not_mutate_state() {
        let mut state = State {
            maximum: 123,
            minimum: -456,
            peak: 289,
            upper_decay_counter: 4,
            lower_decay_counter: 5,
        };
        let original = state;
        let input = [f32::NAN];

        assert!(
            unsafe { measure(input.as_ptr(), std::ptr::null_mut(), 1, 1, 1, &mut state) }.is_err()
        );
        assert_eq!(state, original);
    }
}
