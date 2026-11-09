//! Setup-validated mono source-to-16 kHz client of released samplerate ABI 2.

use crate::session::{INVALID_ARGUMENT, PROVIDER_FAILED, UNSUPPORTED};
use std::{
    ffi::{CStr, c_char},
    mem::size_of,
    ptr::NonNull,
};

#[repr(C)]
struct Converter {
    _private: [u8; 0],
}
#[repr(C)]
struct DescriptorPrefix {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
}
type Reset = unsafe extern "C" fn(*mut Converter) -> i32;
type Process = unsafe extern "C" fn(
    *mut Converter,
    *const f32,
    u32,
    *mut f32,
    u32,
    f64,
    *mut u32,
    *mut u32,
) -> i32;
type Delay = unsafe extern "C" fn(*mut Converter, *mut i64) -> i32;
#[derive(Clone, Copy)]
#[repr(C)]
struct Descriptor {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
    create: Option<unsafe extern "C" fn(u32, u32, u32, u32, *mut *mut Converter) -> i32>,
    reset: Option<Reset>,
    process: Option<Process>,
    destroy: Option<unsafe extern "C" fn(*mut Converter)>,
    _queued_input: Option<unsafe extern "C" fn(*mut Converter, *mut u32) -> i32>,
    _output_delay: Option<unsafe extern "C" fn(*mut Converter, *mut u32) -> i32>,
    input_delay: Option<Delay>,
}
#[link(name = "rptadv_samplerate_adapter")]
unsafe extern "C" {
    fn rptadv_samplerate_adapter_descriptor() -> *const Descriptor;
}

/// A serial converter owner with checked and cached callback-safe operations.
pub struct Client {
    handle: NonNull<Converter>,
    ratio: f64,
    maximum_frames: usize,
    accepted: u64,
    reset: Reset,
    process: Process,
    destroy: unsafe extern "C" fn(*mut Converter),
    input_delay: Delay,
}
/// Actual consumption and at most one source-positioned output from one call.
pub struct Step {
    /// Accepted native input prefix; resubmit the unaccepted tail.
    pub consumed: usize,
    /// Produced analysis sample, or None when the converter needs more input.
    pub sample: Option<f32>,
    /// Output source position in billionths of a native frame since the last reset.
    /// Valid only with a sample; zero-extended startup positions may be negative.
    pub native_position_billionths: i128,
}
impl Client {
    /// Validate ABI 2 including live delay metadata and prepare storage off callback.
    pub fn new(source_rate: u32, maximum_frames: usize) -> Result<Self, i32> {
        // SAFETY: The linked provider owns an immutable process-lifetime descriptor.
        unsafe {
            Self::from_descriptor(
                rptadv_samplerate_adapter_descriptor(),
                source_rate,
                maximum_frames,
            )
        }
    }
    unsafe fn from_descriptor(
        pointer: *const Descriptor,
        source_rate: u32,
        maximum_frames: usize,
    ) -> Result<Self, i32> {
        if !(8000..=192000).contains(&source_rate) || !(1..=4096).contains(&maximum_frames) {
            return Err(INVALID_ARGUMENT);
        }
        // SAFETY: A nonnull provider pointer supplies at least the readable ABI prefix.
        let prefix = unsafe { pointer.cast::<DescriptorPrefix>().as_ref() }.ok_or(UNSUPPORTED)?;
        if prefix.abi_version != 2
            || prefix.struct_size < size_of::<Descriptor>() as u32
            || prefix.capability_name.is_null()
            // SAFETY: A nonnull provider capability is a terminated C string.
            || unsafe { CStr::from_ptr(prefix.capability_name) }.to_bytes() != b"rptadv.samplerate"
        {
            return Err(UNSUPPORTED);
        }
        // SAFETY: The size check established the complete readable descriptor span.
        let descriptor = unsafe { &*pointer };
        let (Some(create), Some(reset), Some(process), Some(destroy), Some(input_delay)) = (
            descriptor.create,
            descriptor.reset,
            descriptor.process,
            descriptor.destroy,
            descriptor.input_delay,
        ) else {
            return Err(UNSUPPORTED);
        };
        #[cfg(test)]
        let (create, reset, process): (
            unsafe extern "C" fn(u32, u32, u32, u32, *mut *mut Converter) -> i32,
            Reset,
            Process,
            // SAFETY: The linked descriptor has process lifetime and is used only for pointer identity.
        ) = if pointer == unsafe { rptadv_samplerate_adapter_descriptor() } {
            (test_create, test_reset, test_process)
        } else {
            (create, reset, process)
        };
        let mut raw = std::ptr::null_mut();
        // SAFETY: The checked callback receives valid setup bounds and an initialized output pointer.
        if unsafe { create(source_rate, 16000, maximum_frames as u32, 1, &mut raw) } != 0 {
            return Err(PROVIDER_FAILED);
        }
        let handle = NonNull::new(raw).ok_or(PROVIDER_FAILED)?;
        Ok(Self {
            handle,
            ratio: 16000.0 / f64::from(source_rate),
            maximum_frames,
            accepted: 0,
            reset,
            process,
            destroy,
            input_delay,
        })
    }
    /// Replace ended-burst FIR history and restart the accepted-input source origin.
    /// The provider retains fractional phase; live metadata continues to describe it.
    pub fn reset(&mut self) -> Result<(), i32> {
        // SAFETY: The serial receive owner retains the live prepared handle.
        if unsafe { (self.reset)(self.handle.as_ptr()) } != 0 {
            return Err(PROVIDER_FAILED);
        }
        self.accepted = 0;
        Ok(())
    }
    /// Consume actual input counts and produce at most one analysis sample.
    /// Empty input drains available output; reset after a provider failure.
    pub fn process_one(&mut self, input: &[f32]) -> Result<Step, i32> {
        if input.len() > self.maximum_frames || input.iter().any(|s| !s.is_finite()) {
            return Err(INVALID_ARGUMENT);
        }
        let mut delay = 0;
        // SAFETY: The cached callback receives the live handle and initialized delay storage.
        if unsafe { (self.input_delay)(self.handle.as_ptr(), &mut delay) } != 0 {
            return Err(PROVIDER_FAILED);
        }
        let position = i128::from(self.accepted) * 1_000_000_000 - i128::from(delay);
        let mut output = 0.0_f32;
        let mut consumed = 0_u32;
        let mut generated = 0_u32;
        // SAFETY: Input and output are disjoint bounded spans; output counters are valid.
        if unsafe {
            (self.process)(
                self.handle.as_ptr(),
                input.as_ptr(),
                input.len() as u32,
                &mut output,
                1,
                self.ratio,
                &mut consumed,
                &mut generated,
            )
        } != 0
            || consumed as usize > input.len()
            || generated > 1
            || (generated == 1 && !output.is_finite())
        {
            return Err(PROVIDER_FAILED);
        }
        self.accepted = self
            .accepted
            .checked_add(u64::from(consumed))
            .ok_or(PROVIDER_FAILED)?;
        Ok(Step {
            consumed: consumed as usize,
            sample: (generated == 1).then_some(output),
            native_position_billionths: position,
        })
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: Control-plane destruction follows release by the serial receive owner.
        unsafe { (self.destroy)(self.handle.as_ptr()) };
    }
}
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum TestFailure {
    None,
    Create,
    Reset,
    Process,
    PartialInput,
    Stall,
    Flood,
}
#[cfg(test)]
thread_local! {static TEST_FAILURE:std::cell::Cell<TestFailure>=const {std::cell::Cell::new(TestFailure::None)};}
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn with_test_failure<T>(failure: TestFailure, action: impl FnOnce() -> T) -> T {
    TEST_FAILURE.with(|slot| {
        struct Restore<'a>(&'a std::cell::Cell<TestFailure>, TestFailure);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _restore = Restore(slot, slot.replace(failure));
        action()
    })
}
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
unsafe extern "C" fn test_create(
    input: u32,
    output: u32,
    max_input: u32,
    max_output: u32,
    out: *mut *mut Converter,
) -> i32 {
    if TEST_FAILURE.with(|v| v.get() == TestFailure::Create) {
        return PROVIDER_FAILED;
    }
    // SAFETY: Setup validated the immutable linked table and exact cached callback signatures.
    unsafe {
        ((*rptadv_samplerate_adapter_descriptor()).create.unwrap())(
            input, output, max_input, max_output, out,
        )
    }
}
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
unsafe extern "C" fn test_reset(handle: *mut Converter) -> i32 {
    if TEST_FAILURE.with(|v| v.get() == TestFailure::Reset) {
        return PROVIDER_FAILED;
    }
    // SAFETY: Setup validated the immutable linked table and exact cached callback signatures.
    unsafe { ((*rptadv_samplerate_adapter_descriptor()).reset.unwrap())(handle) }
}
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
unsafe extern "C" fn test_process(
    handle: *mut Converter,
    input: *const f32,
    count: u32,
    output: *mut f32,
    capacity: u32,
    ratio: f64,
    used: *mut u32,
    generated: *mut u32,
) -> i32 {
    if TEST_FAILURE.with(|v| v.get() == TestFailure::Process) {
        return PROVIDER_FAILED;
    }
    let failure = TEST_FAILURE.with(|v| v.get());
    if failure == TestFailure::Stall || failure == TestFailure::Flood {
        // SAFETY: Client supplies initialized writable output/count storage for capacity one.
        unsafe {
            *used = 0;
            *generated = u32::from(failure == TestFailure::Flood);
            *output = 0.0;
        }
        return 0;
    }
    let count = if failure == TestFailure::PartialInput {
        count.min(1)
    } else {
        count
    }; // SAFETY: Setup validated the immutable linked table and exact cached callback signatures.
    unsafe {
        ((*rptadv_samplerate_adapter_descriptor()).process.unwrap())(
            handle, input, count, output, capacity, ratio, used, generated,
        )
    }
}
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::Client;
    use crate::session::INVALID_ARGUMENT;
    #[test]
    fn real_impulse_tracks_actual_counts_and_native_source_positions() {
        let mut client = Client::new(48000, 960).unwrap();
        let mut input = [0.0; 960];
        input[0] = 1.0;
        let mut consumed = 0;
        let mut outputs = 0;
        let mut previous = None;
        let mut peak = (0.0_f32, i128::MAX);
        for _ in 0..1024 {
            let step = client.process_one(&input[consumed..]).unwrap();
            assert!(step.consumed <= 960 - consumed);
            consumed += step.consumed;
            if let Some(sample) = step.sample {
                if let Some(previous) = previous {
                    assert_eq!(step.native_position_billionths - previous, 3_000_000_000);
                }
                previous = Some(step.native_position_billionths);
                outputs += 1;
                if sample.abs() > peak.0 {
                    peak = (sample.abs(), step.native_position_billionths);
                }
            } else if consumed == 960 {
                break;
            } else {
                assert!(step.consumed > 0);
            }
        }
        assert_eq!(consumed, 960);
        assert!(outputs > 300);
        assert!(peak.0 > 0.1);
        assert!(peak.1.abs() <= 2_000_000_000, "peak={peak:?}");
        client.reset().unwrap();
        let mut consumed = 0;
        for _ in 0..1024 {
            let step = client.process_one(&[0.0; 960][consumed..]).unwrap();
            consumed += step.consumed;
            if let Some(sample) = step.sample {
                assert_eq!(sample, 0.0);
            } else if consumed == 960 {
                break;
            } else {
                assert!(step.consumed > 0);
            }
        }
        assert_eq!(consumed, 960);
    }
    #[test]
    fn invalid_prepared_bounds_are_rejected_before_provider_setup() {
        for maximum in [0, 4097] {
            assert!(matches!(Client::new(48000, maximum), Err(INVALID_ARGUMENT)));
        }
        for source_rate in [0, 7999, 192001, u32::MAX] {
            assert!(matches!(
                Client::new(source_rate, 256),
                Err(INVALID_ARGUMENT)
            ));
        }
    }
    use super::{Converter, Descriptor, DescriptorPrefix, Process};
    use crate::session::{PROVIDER_FAILED, UNSUPPORTED};
    use std::{cell::Cell, mem::size_of, ptr};
    thread_local! {static FREES:Cell<usize>=const {Cell::new(0)};}
    unsafe extern "C" fn create(
        input: u32,
        output: u32,
        max_input: u32,
        max_output: u32,
        out: *mut *mut Converter,
    ) -> i32 {
        if (input, output, max_input, max_output) != (48000, 16000, 4, 1) {
            return -1;
        }
        // SAFETY: The fixture supplies writable outputs and handles matching this callback ABI.
        unsafe { *out = Box::into_raw(Box::new(0_u8)).cast() };
        0
    }
    unsafe extern "C" fn create_null(
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        out: *mut *mut Converter,
    ) -> i32 {
        // SAFETY: The fixture supplies writable outputs and handles matching this callback ABI.
        unsafe { *out = ptr::null_mut() };
        0
    }
    unsafe extern "C" fn create_error(
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: *mut *mut Converter,
    ) -> i32 {
        -1
    }
    unsafe extern "C" fn destroy(handle: *mut Converter) {
        // SAFETY: The fixture created this Box handle and transfers its ownership exactly once.
        drop(unsafe { Box::from_raw(handle.cast::<u8>()) });
        FREES.with(|v| v.set(v.get() + 1));
    }
    unsafe extern "C" fn reset(_: *mut Converter) -> i32 {
        0
    }
    unsafe extern "C" fn reset_error(_: *mut Converter) -> i32 {
        -1
    }
    unsafe extern "C" fn delay(_: *mut Converter, out: *mut i64) -> i32 {
        // SAFETY: The fixture supplies writable outputs and handles matching this callback ABI.
        unsafe { *out = 1_000_000_000 };
        0
    }
    unsafe extern "C" fn delay_error(_: *mut Converter, _: *mut i64) -> i32 {
        -1
    }
    unsafe extern "C" fn process(
        _: *mut Converter,
        _: *const f32,
        count: u32,
        out: *mut f32,
        _: u32,
        _: f64,
        used: *mut u32,
        generated: *mut u32,
    ) -> i32 {
        // SAFETY: The fixture supplies writable outputs and handles matching this callback ABI.
        unsafe {
            *used = count;
            *generated = u32::from(count != 0);
            *out = 0.25;
        }
        0
    }
    unsafe extern "C" fn process_error(
        _: *mut Converter,
        _: *const f32,
        _: u32,
        _: *mut f32,
        _: u32,
        _: f64,
        _: *mut u32,
        _: *mut u32,
    ) -> i32 {
        -1
    }
    unsafe extern "C" fn bad_used(
        _: *mut Converter,
        _: *const f32,
        count: u32,
        _: *mut f32,
        _: u32,
        _: f64,
        used: *mut u32,
        generated: *mut u32,
    ) -> i32 {
        // SAFETY: The fixture supplies writable outputs and handles matching this callback ABI.
        unsafe {
            *used = count + 1;
            *generated = 0
        };
        0
    }
    unsafe extern "C" fn bad_generated(
        _: *mut Converter,
        _: *const f32,
        count: u32,
        _: *mut f32,
        _: u32,
        _: f64,
        used: *mut u32,
        generated: *mut u32,
    ) -> i32 {
        // SAFETY: The fixture supplies writable outputs and handles matching this callback ABI.
        unsafe {
            *used = count;
            *generated = 2
        };
        0
    }
    unsafe extern "C" fn bad_sample(
        _: *mut Converter,
        _: *const f32,
        count: u32,
        out: *mut f32,
        _: u32,
        _: f64,
        used: *mut u32,
        generated: *mut u32,
    ) -> i32 {
        // SAFETY: The fixture supplies writable outputs and handles matching this callback ABI.
        unsafe {
            *used = count;
            *generated = 1;
            *out = f32::NAN
        };
        0
    }
    fn descriptor() -> Descriptor {
        Descriptor {
            struct_size: size_of::<Descriptor>() as u32,
            abi_version: 2,
            capability_name: c"rptadv.samplerate".as_ptr(),
            create: Some(create),
            reset: Some(reset),
            process: Some(process),
            destroy: Some(destroy),
            _queued_input: None,
            _output_delay: None,
            input_delay: Some(delay),
        }
    }
    #[test]
    fn rejects_short_wrong_and_incomplete_descriptors_before_reading_callbacks() {
        assert!(matches!(
            // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
            unsafe { Client::from_descriptor(ptr::null(), 48000, 4) },
            Err(UNSUPPORTED)
        ));
        let prefix = DescriptorPrefix {
            struct_size: size_of::<DescriptorPrefix>() as u32,
            abi_version: 2,
            capability_name: c"rptadv.samplerate".as_ptr(),
        };
        assert!(matches!(
            // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
            unsafe {
                Client::from_descriptor((&prefix as *const DescriptorPrefix).cast(), 48000, 4)
            },
            Err(UNSUPPORTED)
        ));
        for field in 0..9 {
            let mut table = descriptor();
            match field {
                0 => table.abi_version = 1,
                1 => table.struct_size -= 1,
                2 => table.capability_name = ptr::null(),
                3 => table.capability_name = c"wrong".as_ptr(),
                4 => table.create = None,
                5 => table.reset = None,
                6 => table.process = None,
                7 => table.destroy = None,
                _ => table.input_delay = None,
            }
            assert!(matches!(
                // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
                unsafe { Client::from_descriptor(&table, 48000, 4) },
                Err(UNSUPPORTED)
            ));
        }
    }
    #[test]
    fn owned_handle_is_released_and_reset_restarts_source_clock() {
        FREES.with(|v| v.set(0));
        let table = descriptor();
        // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
        let mut client = unsafe { Client::from_descriptor(&table, 48000, 4) }.unwrap();
        let first = client.process_one(&[0.0; 2]).unwrap();
        assert_eq!(first.consumed, 2);
        assert_eq!(first.sample, Some(0.25));
        assert_eq!(first.native_position_billionths, -1_000_000_000);
        assert_eq!(
            client.process_one(&[]).unwrap().native_position_billionths,
            1_000_000_000
        );
        client.reset().unwrap();
        assert_eq!(
            client
                .process_one(&[0.0])
                .unwrap()
                .native_position_billionths,
            -1_000_000_000
        );
        for input in [&[0.0; 5][..], &[f32::NAN][..], &[f32::INFINITY][..]] {
            assert!(matches!(client.process_one(input), Err(INVALID_ARGUMENT)));
        }
        client.accepted = u64::MAX;
        assert!(matches!(client.process_one(&[0.0]), Err(PROVIDER_FAILED)));
        drop(client);
        FREES.with(|v| assert_eq!(v.get(), 1));
    }
    #[test]
    fn failed_provider_operations_and_malformed_counts_are_not_trusted() {
        for callback in [
            create_error as unsafe extern "C" fn(u32, u32, u32, u32, *mut *mut Converter) -> i32,
            create_null,
        ] {
            let mut table = descriptor();
            table.create = Some(callback);
            assert!(matches!(
                // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
                unsafe { Client::from_descriptor(&table, 48000, 4) },
                Err(PROVIDER_FAILED)
            ));
        }
        for callback in [
            process_error as Process,
            bad_used,
            bad_generated,
            bad_sample,
        ] {
            let mut table = descriptor();
            table.process = Some(callback);
            // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
            let mut client = unsafe { Client::from_descriptor(&table, 48000, 4) }.unwrap();
            assert!(matches!(client.process_one(&[0.0]), Err(PROVIDER_FAILED)));
        }
        let mut table = descriptor();
        table.reset = Some(reset_error);
        // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
        let mut client = unsafe { Client::from_descriptor(&table, 48000, 4) }.unwrap();
        assert!(matches!(client.reset(), Err(PROVIDER_FAILED)));
        table.reset = Some(reset);
        table.input_delay = Some(delay_error);
        // SAFETY: Fixture storage is alive; short prefixes are rejected before tail access.
        let mut client = unsafe { Client::from_descriptor(&table, 48000, 4) }.unwrap();
        assert!(matches!(client.process_one(&[0.0]), Err(PROVIDER_FAILED)));
    }
}
