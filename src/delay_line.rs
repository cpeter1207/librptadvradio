//! Legacy-compatible circular delay line on canonical F32 PCM.
//!
//! USBRadioPlus uses this stage to delay receiver audio until carrier
//! qualification is known.  The caller retains both the circular storage and
//! the cursor, so the primitive remains allocation-free and can be called at
//! an audio callback boundary.

use super::RADIO_INVALID_ARGUMENT;

/// Caller-owned cursor and reset state for one circular delay line.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct State {
    /// Next circular-storage position to receive a sample.
    pub input_index: u32,
    /// Nonzero while historical samples must be cleared on a silent reset.
    pub dirty: u32,
}

/// Borrowed PCM and control values for one delay-line operation.
pub(crate) struct Request<'a> {
    pub(crate) input: *const f32,
    pub(crate) output: *mut f32,
    pub(crate) sample_count: u32,
    pub(crate) storage: *mut f32,
    pub(crate) storage_capacity: u32,
    pub(crate) lead: u32,
    pub(crate) state: &'a mut State,
    pub(crate) enabled: bool,
    pub(crate) outzero: bool,
}

/// Process one canonical-F32 delay-line span with historical reset semantics.
///
/// A disabled or `outzero` call clears storage and output only when `dirty` is
/// set.  Otherwise it leaves every caller-owned buffer unchanged.  An enabled
/// call writes input before reading the delayed position, retaining zero-delay
/// behavior when `lead` is zero and the original cursor value at a wrap edge.
///
/// # Safety
///
/// Non-null pointers must designate the stated number of readable or writable
/// elements. `storage` must designate `storage_capacity` writable F32 values.
pub unsafe fn process(request: Request<'_>) -> Result<(), i32> {
    let Request {
        input,
        output,
        sample_count,
        storage,
        storage_capacity,
        lead,
        state,
        enabled,
        outzero,
    } = request;

    if storage_capacity == 0 || lead > storage_capacity {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    let sample_count = sample_count as usize;

    if !enabled || outzero {
        if state.dirty == 0 {
            return Ok(());
        }
        if storage.is_null() || (sample_count != 0 && output.is_null()) {
            return Err(RADIO_INVALID_ARGUMENT);
        }
        for index in 0..storage_capacity as usize {
            unsafe { storage.add(index).write(0.0) };
        }
        for index in 0..sample_count {
            unsafe { output.add(index).write(0.0) };
        }
        state.input_index = 0;
        state.dirty = 0;
        return Ok(());
    }

    if sample_count != 0 && (input.is_null() || output.is_null()) {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    if sample_count != 0 && storage.is_null() {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // Reject a malformed F32 input before changing cursor, storage, or output.
    for index in 0..sample_count {
        if !unsafe { input.add(index).read() }.is_finite() {
            return Err(RADIO_INVALID_ARGUMENT);
        }
    }

    let capacity = storage_capacity as usize;
    let mut input_index = state.input_index as usize;
    let mut output_index = (input_index + capacity - lead as usize) % capacity;
    for index in 0..sample_count {
        input_index %= capacity;
        output_index %= capacity;
        let sample = unsafe { input.add(index).read() };

        unsafe { storage.add(input_index).write(sample) };
        let delayed = unsafe { storage.add(output_index).read() };
        unsafe { output.add(index).write(delayed) };
        input_index += 1;
        output_index += 1;
    }
    state.input_index = input_index as u32;
    state.dirty = 1;
    Ok(())
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{Request, State, process};

    #[test]
    fn invalid_storage_and_pcm_spans_fail_transactionally() {
        // Each malformed boundary gets otherwise-valid storage and dirty state.
        for case in 0..7 {
            let input = [1.0];
            let mut output = [9.0];
            let mut storage = [3.0];
            let mut state = State {
                input_index: 0,
                dirty: 1,
            };
            assert_eq!(
                unsafe {
                    process(Request {
                        input: if case == 4 {
                            core::ptr::null()
                        } else {
                            input.as_ptr()
                        },
                        output: if case == 3 || case == 5 {
                            core::ptr::null_mut()
                        } else {
                            output.as_mut_ptr()
                        },
                        sample_count: 1,
                        storage: if case == 2 || case == 6 {
                            core::ptr::null_mut()
                        } else {
                            storage.as_mut_ptr()
                        },
                        storage_capacity: if case == 0 { 0 } else { 1 },
                        lead: if case == 1 { 2 } else { 0 },
                        state: &mut state,
                        enabled: case != 2 && case != 3,
                        outzero: false,
                    })
                },
                Err(crate::RADIO_INVALID_ARGUMENT),
                "case {case}"
            );
            assert_eq!(
                state,
                State {
                    input_index: 0,
                    dirty: 1
                }
            );
            assert_eq!(output, [9.0]);
            assert_eq!(storage, [3.0]);
        }
    }

    #[test]
    fn empty_active_and_dirty_reset_spans_accept_absent_pcm() {
        for enabled in [true, false] {
            let mut storage = [3.0];
            let mut state = State {
                input_index: 0,
                dirty: 1,
            };
            assert_eq!(
                unsafe {
                    process(Request {
                        input: core::ptr::null(),
                        output: core::ptr::null_mut(),
                        sample_count: 0,
                        storage: storage.as_mut_ptr(),
                        storage_capacity: 1,
                        lead: 0,
                        state: &mut state,
                        enabled,
                        outzero: false,
                    })
                },
                Ok(())
            );
            assert_eq!(storage, [if enabled { 3.0 } else { 0.0 }]);
            assert_eq!(state.dirty, u32::from(enabled));
        }
    }

    fn run(input: &[f32], storage: &mut [f32], lead: u32, state: &mut State) -> Vec<f32> {
        let mut output = vec![0.0; input.len()];

        unsafe {
            process(Request {
                input: input.as_ptr(),
                output: output.as_mut_ptr(),
                sample_count: input.len() as u32,
                storage: storage.as_mut_ptr(),
                storage_capacity: storage.len() as u32,
                lead,
                state,
                enabled: true,
                outzero: false,
            })
        }
        .expect("valid delay line");
        output
    }

    #[test]
    fn wrap_and_lead_match_legacy_sample_order() {
        let input = [10.0, 20.0, 30.0, 40.0, 50.0, 60.0, 70.0, 80.0];
        let mut storage = [0.0; 5];
        let mut state = State::default();

        assert_eq!(
            run(&input, &mut storage, 3, &mut state),
            [0.0, 0.0, 0.0, 10.0, 20.0, 30.0, 40.0, 50.0]
        );
        assert_eq!(storage, [60.0, 70.0, 80.0, 40.0, 50.0]);
        assert_eq!(
            state,
            State {
                input_index: 3,
                dirty: 1,
            }
        );
    }

    #[test]
    fn every_split_boundary_matches_one_complete_span() {
        let input = [10.0, -20.0, 30.0, -40.0, 50.0, -60.0, 70.0, -80.0];
        let mut whole_storage = [0.0; 5];
        let mut whole_state = State::default();
        let whole_output = run(&input, &mut whole_storage, 3, &mut whole_state);

        for split in 1..input.len() {
            let mut storage = [0.0; 5];
            let mut state = State::default();
            let mut output = run(&input[..split], &mut storage, 3, &mut state);

            output.extend(run(&input[split..], &mut storage, 3, &mut state));
            assert_eq!(output, whole_output, "split {split}");
            assert_eq!(storage, whole_storage, "split {split}");
            assert_eq!(state, whole_state, "split {split}");
        }
    }

    #[test]
    fn above_capacity_cursor_normalizes_at_the_first_sample_like_legacy_c() {
        let input = [10.0, 20.0, 30.0];
        let mut storage = [1.0, 2.0, 3.0, 4.0, 5.0];
        let mut state = State {
            input_index: 7,
            dirty: 1,
        };

        assert_eq!(run(&input, &mut storage, 3, &mut state), [5.0, 1.0, 2.0]);
        assert_eq!(storage, [1.0, 2.0, 10.0, 20.0, 30.0]);
        assert_eq!(
            state,
            State {
                input_index: 5,
                dirty: 1,
            }
        );
    }

    #[test]
    fn disabled_and_outzero_retain_dirty_reset_behavior() {
        let mut storage = [1.0, 2.0, 3.0, 4.0];
        let mut output = [99.0, 99.0];
        let mut state = State {
            input_index: 3,
            dirty: 1,
        };

        unsafe {
            process(Request {
                input: std::ptr::null(),
                output: output.as_mut_ptr(),
                sample_count: output.len() as u32,
                storage: storage.as_mut_ptr(),
                storage_capacity: storage.len() as u32,
                lead: 2,
                state: &mut state,
                enabled: false,
                outzero: false,
            })
        }
        .expect("dirty disabled reset");
        assert_eq!(storage, [0.0; 4]);
        assert_eq!(output, [0.0; 2]);
        assert_eq!(state, State::default());

        output = [99.0, 99.0];
        unsafe {
            process(Request {
                input: std::ptr::null(),
                output: output.as_mut_ptr(),
                sample_count: output.len() as u32,
                storage: storage.as_mut_ptr(),
                storage_capacity: storage.len() as u32,
                lead: 2,
                state: &mut state,
                enabled: false,
                outzero: false,
            })
        }
        .expect("clean disabled no-op");
        assert_eq!(output, [99.0; 2]);

        unsafe {
            process(Request {
                input: std::ptr::null(),
                output: output.as_mut_ptr(),
                sample_count: output.len() as u32,
                storage: storage.as_mut_ptr(),
                storage_capacity: storage.len() as u32,
                lead: 2,
                state: &mut state,
                enabled: true,
                outzero: true,
            })
        }
        .expect("clean outzero no-op");
        assert_eq!(output, [99.0; 2]);

        storage = [4.0, 3.0, 2.0, 1.0];
        output = [99.0, 99.0];
        state = State {
            input_index: 2,
            dirty: 1,
        };
        unsafe {
            process(Request {
                input: std::ptr::null(),
                output: output.as_mut_ptr(),
                sample_count: output.len() as u32,
                storage: storage.as_mut_ptr(),
                storage_capacity: storage.len() as u32,
                lead: 2,
                state: &mut state,
                enabled: true,
                outzero: true,
            })
        }
        .expect("dirty outzero reset");
        assert_eq!(storage, [0.0; 4]);
        assert_eq!(output, [0.0; 2]);
        assert_eq!(state, State::default());
    }

    #[test]
    fn invalid_input_does_not_change_caller_state() {
        let mut storage = [1.0, 2.0, 3.0, 4.0];
        let original_storage = storage;
        let mut output = [99.0, 99.0];
        let original_output = output;
        let mut state = State {
            input_index: 2,
            dirty: 1,
        };
        let original_state = state;
        let input = [f32::NAN, 1.0];

        assert!(
            unsafe {
                process(Request {
                    input: input.as_ptr(),
                    output: output.as_mut_ptr(),
                    sample_count: input.len() as u32,
                    storage: storage.as_mut_ptr(),
                    storage_capacity: storage.len() as u32,
                    lead: 2,
                    state: &mut state,
                    enabled: true,
                    outzero: false,
                })
            }
            .is_err()
        );
        assert_eq!(storage, original_storage);
        assert_eq!(output, original_output);
        assert_eq!(state, original_state);
    }
}
