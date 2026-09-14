//! Legacy-compatible CTCSS center slicer on canonical F32 PCM.
//!
//! USBRadioPlus centers the filtered low-speed-data signal before its CTCSS
//! detector reads it. This primitive retains the legacy signed-16 min/max
//! tracker and its two distinct centered outputs while keeping the shared-core
//! PCM boundary canonical F32.

use super::{RADIO_INVALID_ARGUMENT, audio_meter::f32_to_pcm_code};

const PCM_SCALE: f32 = 32_768.0;

/// Caller-owned state for one legacy-compatible center slicer.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Current positive input extremum in legacy PCM codes.
    pub maximum: i16,
    /// Current negative input extremum in legacy PCM codes.
    pub minimum: i16,
    /// Most recent half peak-to-peak envelope in legacy PCM codes.
    pub peak: i16,
    /// Retained compatibility upper detector counter.
    pub upper_decay_counter: i32,
    /// Retained compatibility lower detector counter.
    pub lower_decay_counter: i32,
}

/// Borrowed PCM and fixed-point controls for one center-slicer operation.
pub(crate) struct Request<'a> {
    /// Readable canonical F32 input.
    pub(crate) input: *const f32,
    /// Writable centered canonical F32 output.
    pub(crate) centered_output: *mut f32,
    /// Writable limited canonical F32 output.
    pub(crate) limited_output: *mut f32,
    /// Number of scalar samples to process.
    pub(crate) sample_count: u32,
    /// Legacy signed-PCM limiter magnitude.
    pub(crate) limit: i32,
    /// Legacy peak-tracking set point.
    pub(crate) setpoint: i16,
    /// Legacy per-sample extrema discharge amount.
    pub(crate) decay_factor: i32,
    /// Caller-owned extrema and retained counter state.
    pub(crate) state: &'a mut State,
}

/// Optional preallocated diagnostic trace for one center-slicer span.
///
/// The compatibility trace alternates retained lower and upper extrema in
/// eight-sample groups.  The owned receive path keeps that diagnostic data
/// alongside its F32 pipeline without involving the legacy adapter.
pub(crate) struct Trace<'a> {
    /// Writable canonical-F32 trace span with `sample_count` entries.
    pub(crate) output: *mut f32,
    /// Persistent pre-sample legacy trace counter.
    pub(crate) phase: &'a mut u32,
}

/// Center and limit one F32 PCM span with the historical fixed-point rules.
///
/// The complete input span is quantized and validated before changing caller
/// state or either output. This protects a compatibility caller's exact C
/// fallback from a malformed direct F32 request. Both outputs receive the
/// value after its historical signed-16 assignment, not the wider temporary
/// accumulator used for the min/max calculation.
///
/// # Safety
///
/// For nonzero `sample_count`, all pointers must designate readable or
/// writable F32 spans of that size. `state` is a valid mutable state object.
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
pub unsafe fn process(request: Request<'_>) -> Result<(), i32> {
    // SAFETY: `process_with_trace` retains this request's documented span and
    // state requirements. No trace storage is requested by this wrapper.
    unsafe { process_with_trace(request, None) }
}

/// Center and limit one span, optionally retaining the legacy extrema trace.
///
/// The trace is Rust-internal composition state rather than a new adapter ABI.
/// Its output receives the historical signed-16 extrema mapped to canonical
/// F32 after each centered sample.  The operation otherwise has the same
/// validation, arithmetic, and mutation contract as the non-trace operation.
///
/// # Safety
///
/// For nonzero `sample_count`, the request pointers identify readable or
/// writable F32 spans of that size and `state` is valid. When `trace` is
/// present, its output identifies another writable span of that size.
pub(crate) unsafe fn process_with_trace(
    request: Request<'_>,
    mut trace: Option<Trace<'_>>,
) -> Result<(), i32> {
    let Request {
        input,
        centered_output,
        limited_output,
        sample_count,
        limit,
        setpoint,
        decay_factor,
        state,
    } = request;
    let sample_count = sample_count as usize;
    if sample_count != 0
        && (input.is_null() || centered_output.is_null() || limited_output.is_null())
    {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    if sample_count != 0 && trace.as_ref().is_some_and(|entry| entry.output.is_null()) {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // Validate before changing any output or caller-owned state. The exact
    // i16/F32 bridge maps without rounding, while direct callers retain a
    // bounded, deterministic F32 contract.
    for index in 0..sample_count {
        f32_to_pcm_code(unsafe { input.add(index).read() })?;
    }

    let mut next = *state;
    let mut maximum = i32::from(next.maximum);
    let mut minimum = i32::from(next.minimum);
    let mut peak = i32::from(next.peak);
    let setpoint = i32::from(setpoint);

    for index in 0..sample_count {
        let mut accumulator = f32_to_pcm_code(unsafe { input.add(index).read() })?;

        if accumulator > maximum {
            maximum = accumulator;
            if minimum < maximum.wrapping_sub(setpoint) {
                minimum = maximum.wrapping_sub(setpoint);
            }
        } else if accumulator < minimum {
            minimum = accumulator;
            if maximum > minimum.wrapping_add(setpoint) {
                maximum = minimum.wrapping_add(setpoint);
            }
        }

        maximum = maximum.wrapping_sub(decay_factor);
        if maximum < minimum {
            maximum = minimum;
        }

        minimum = minimum.wrapping_add(decay_factor);
        if minimum > maximum {
            minimum = maximum;
        }

        peak = maximum.wrapping_sub(minimum) / 2;
        let center = maximum.wrapping_add(minimum) / 2;
        accumulator = accumulator.wrapping_sub(center);
        let centered = accumulator as i16;

        unsafe {
            centered_output
                .add(index)
                .write(f32::from(centered) / PCM_SCALE)
        };

        if accumulator > limit {
            accumulator = limit;
        } else if accumulator < limit.wrapping_neg() {
            accumulator = limit.wrapping_neg();
        }
        let limited = accumulator as i16;
        unsafe {
            limited_output
                .add(index)
                .write(f32::from(limited) / PCM_SCALE)
        };
        if let Some(trace) = trace.as_mut() {
            let trace_value = if (*trace.phase / 8) & 1 != 0 {
                maximum as i16
            } else {
                minimum as i16
            };
            // SAFETY: validation confirmed the caller's complete trace span.
            unsafe {
                trace
                    .output
                    .add(index)
                    .write(f32::from(trace_value) / PCM_SCALE)
            };
            *trace.phase = trace.phase.wrapping_add(1);
        }
    }

    next.maximum = maximum as i16;
    next.minimum = minimum as i16;
    next.peak = peak as i16;
    *state = next;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    #[test]
    fn null_pcm_spans_reject_before_mutation() {
        for missing in 0..3 {
            let input = [0.0];
            let mut centered = [0.25];
            let mut limited = [-0.25];
            let mut state = super::State::default();
            assert_eq!(
                unsafe {
                    super::process(super::Request {
                        input: if missing == 0 {
                            core::ptr::null()
                        } else {
                            input.as_ptr()
                        },
                        centered_output: if missing == 1 {
                            core::ptr::null_mut()
                        } else {
                            centered.as_mut_ptr()
                        },
                        limited_output: if missing == 2 {
                            core::ptr::null_mut()
                        } else {
                            limited.as_mut_ptr()
                        },
                        sample_count: 1,
                        limit: 100,
                        setpoint: 1000,
                        decay_factor: 1,
                        state: &mut state,
                    })
                },
                Err(crate::RADIO_INVALID_ARGUMENT)
            );
            assert_eq!(state, super::State::default());
            assert_eq!(centered, [0.25]);
            assert_eq!(limited, [-0.25]);
        }
    }

    #[test]
    fn trace_validation_and_alternating_extrema_remain_bounded() {
        let input = [0.0_f32; 16];
        let mut centered = [9.0; 16];
        let mut limited = [9.0; 16];
        let mut state = super::State::default();
        let mut phase = 0;
        let request = super::Request {
            input: input.as_ptr(),
            centered_output: centered.as_mut_ptr(),
            limited_output: limited.as_mut_ptr(),
            sample_count: 16,
            limit: 10,
            setpoint: 10,
            decay_factor: 1,
            state: &mut state,
        };
        assert_eq!(
            unsafe {
                super::process_with_trace(
                    request,
                    Some(super::Trace {
                        output: core::ptr::null_mut(),
                        phase: &mut phase,
                    }),
                )
            },
            Err(crate::RADIO_INVALID_ARGUMENT)
        );
        assert_eq!(centered, [9.0; 16]);
        let mut trace = [9.0; 16];
        let request = super::Request {
            input: input.as_ptr(),
            centered_output: centered.as_mut_ptr(),
            limited_output: limited.as_mut_ptr(),
            sample_count: 16,
            limit: 10,
            setpoint: 10,
            decay_factor: 1,
            state: &mut state,
        };
        assert_eq!(
            unsafe {
                super::process_with_trace(
                    request,
                    Some(super::Trace {
                        output: trace.as_mut_ptr(),
                        phase: &mut phase,
                    }),
                )
            },
            Ok(())
        );
        assert_eq!(phase, 16);
        assert_eq!(trace, [0.0; 16]);
        assert_eq!(centered, [0.0; 16]);
    }
    use super::{Request, State, process};

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
        limit: i32,
        setpoint: i16,
        decay_factor: i32,
        state: &mut State,
    ) -> (Vec<i16>, Vec<i16>) {
        let input = samples(input);
        let mut centered = vec![0.0_f32; input.len()];
        let mut limited = vec![0.0_f32; input.len()];

        unsafe {
            process(Request {
                input: input.as_ptr(),
                centered_output: centered.as_mut_ptr(),
                limited_output: limited.as_mut_ptr(),
                sample_count: input.len() as u32,
                limit,
                setpoint,
                decay_factor,
                state,
            })
        }
        .expect("valid center slicer");
        (codes(&centered), codes(&limited))
    }

    #[test]
    fn rails_center_and_limit_match_legacy_fixed_point_steps() {
        let mut state = State {
            maximum: 0,
            minimum: 0,
            peak: 777,
            upper_decay_counter: 123,
            lower_decay_counter: -456,
        };
        let (centered, limited) = run(
            &[
                i16::MIN,
                i16::MAX,
                -2_000,
                2_000,
                -1,
                1,
                0,
                i16::MIN,
                i16::MAX,
                123,
                -123,
            ],
            100,
            1_000,
            1,
            &mut state,
        );

        assert_eq!(
            centered,
            [
                -500, 500, -500, 500, -500, -498, -498, -500, 500, -500, -500
            ]
        );
        assert_eq!(
            limited,
            [
                -100, 100, -100, 100, -100, -100, -100, -100, 100, -100, -100
            ]
        );
        assert_eq!(
            state,
            State {
                maximum: 876,
                minimum: -122,
                peak: 499,
                upper_decay_counter: 123,
                lower_decay_counter: -456,
            }
        );
    }

    #[test]
    fn every_split_boundary_matches_one_complete_span() {
        let input = [
            i16::MIN,
            i16::MAX,
            -2_000,
            2_000,
            -1,
            1,
            0,
            i16::MIN,
            i16::MAX,
            123,
            -123,
        ];
        let initial = State {
            maximum: 0,
            minimum: 0,
            peak: 777,
            upper_decay_counter: 123,
            lower_decay_counter: -456,
        };
        let mut whole_state = initial;
        let (whole_centered, whole_limited) = run(&input, 100, 1_000, 1, &mut whole_state);

        for split in 1..input.len() {
            let mut state = initial;
            let (mut centered, mut limited) = run(&input[..split], 100, 1_000, 1, &mut state);
            let (second_centered, second_limited) = run(&input[split..], 100, 1_000, 1, &mut state);

            centered.extend(second_centered);
            limited.extend(second_limited);
            assert_eq!(centered, whole_centered, "centered split {split}");
            assert_eq!(limited, whole_limited, "limited split {split}");
            assert_eq!(state, whole_state, "state split {split}");
        }
    }

    #[test]
    fn preserved_counters_and_zero_span_do_not_change_state() {
        let mut state = State {
            maximum: 200,
            minimum: -100,
            peak: 150,
            upper_decay_counter: 37,
            lower_decay_counter: -19,
        };
        let expected = state;

        unsafe {
            process(Request {
                input: core::ptr::null(),
                centered_output: core::ptr::null_mut(),
                limited_output: core::ptr::null_mut(),
                sample_count: 0,
                limit: 50,
                setpoint: 400,
                decay_factor: 3,
                state: &mut state,
            })
        }
        .expect("zero span is a valid no-op");
        assert_eq!(state, expected);

        let (_, _) = run(&[100, -100], 50, 400, 3, &mut state);
        assert_eq!(state.upper_decay_counter, 37);
        assert_eq!(state.lower_decay_counter, -19);
    }

    #[test]
    fn signed_division_and_negative_limit_match_legacy_branches() {
        let mut state = State {
            maximum: -5,
            minimum: -6,
            peak: 999,
            upper_decay_counter: 3,
            lower_decay_counter: -4,
        };
        let (centered, limited) = run(&[-5, -6], 100, 100, 0, &mut state);

        assert_eq!(centered, [0, -1]);
        assert_eq!(limited, [0, -1]);
        assert_eq!(state.maximum, -5);
        assert_eq!(state.minimum, -6);
        assert_eq!(state.peak, 0);

        state = State::default();
        let (centered, limited) = run(&[10, -10], -2, 1_000, 0, &mut state);
        assert_eq!(centered, [5, -10]);
        assert_eq!(limited, [-2, 2]);
    }

    #[test]
    fn invalid_f32_rejects_without_mutating_caller_state_or_output() {
        let input = [f32::NAN];
        let mut centered = [0.25_f32];
        let mut limited = [-0.25_f32];
        let mut state = State {
            maximum: 123,
            minimum: -456,
            peak: 289,
            upper_decay_counter: 4,
            lower_decay_counter: 5,
        };
        let expected = state;

        assert!(
            unsafe {
                process(Request {
                    input: input.as_ptr(),
                    centered_output: centered.as_mut_ptr(),
                    limited_output: limited.as_mut_ptr(),
                    sample_count: 1,
                    limit: 100,
                    setpoint: 1_000,
                    decay_factor: 1,
                    state: &mut state,
                })
            }
            .is_err()
        );
        assert_eq!(state, expected);
        assert_eq!(centered, [0.25]);
        assert_eq!(limited, [-0.25]);
    }
}
