//! Setup-validated client of the released CTCSS descriptor.

use std::{
    ffi::{CStr, c_char},
    mem::size_of,
    ptr::NonNull,
};

use crate::session::{PROVIDER_FAILED, UNSUPPORTED};

pub const TONE_COUNT: usize = 38;
const ABI_VERSION: u32 = 1;
const CAPABILITY_NAME: &[u8] = b"rptadv.ctcss";

#[repr(C)]
struct Rx {
    _private: [u8; 0],
}

#[repr(C)]
struct Tx {
    _private: [u8; 0],
}

#[repr(C)]
struct DescriptorPrefix {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
}

#[derive(Clone, Copy)]
#[repr(C)]
struct Descriptor {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
    rx_create: Option<extern "C" fn() -> *mut Rx>,
    rx_destroy: Option<unsafe extern "C" fn(*mut Rx)>,
    rx_configure: Option<unsafe extern "C" fn(*mut Rx, u64, bool)>,
    rx_enabled: Option<unsafe extern "C" fn(*const Rx) -> bool>,
    rx_decoded: Option<unsafe extern "C" fn(*const Rx) -> i16>,
    rx_tail_tone_active: Option<unsafe extern "C" fn(*const Rx) -> bool>,
    rx_clear_tail_on_carrier_loss: Option<unsafe extern "C" fn(*mut Rx, bool)>,
    rx_process: Option<unsafe extern "C" fn(*mut Rx, *const f32, u32, bool) -> i16>,
    tx_create: Option<extern "C" fn() -> *mut Tx>,
    tx_destroy: Option<unsafe extern "C" fn(*mut Tx)>,
    legacy_frequency: Option<extern "C" fn(f64) -> f64>,
    tx_generate: Option<unsafe extern "C" fn(*mut Tx, *mut f32, u32, f64, f32, u32, f64)>,
}

#[link(name = "rptadvctcss")]
unsafe extern "C" {
    fn rptadv_ctcss_descriptor() -> *const Descriptor;
}

#[cfg(test)]
thread_local! {
    static TEST_TX_DESCRIPTOR: std::cell::Cell<*const Descriptor> = const { std::cell::Cell::new(std::ptr::null()) };
}

fn transmit_descriptor() -> *const Descriptor {
    #[cfg(test)]
    {
        let pointer = TEST_TX_DESCRIPTOR.with(|slot| slot.get());
        if !pointer.is_null() {
            return pointer;
        }
    }
    // SAFETY: the linked provider owns this process-lifetime descriptor.
    unsafe { rptadv_ctcss_descriptor() }
}

#[cfg(test)]
extern "C" fn injected_null_tx() -> *mut Tx {
    std::ptr::null_mut()
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn with_test_tx_create_failure<T>(action: impl FnOnce() -> T) -> T {
    // SAFETY: the released provider returns a complete immutable descriptor.
    let mut table = unsafe { *rptadv_ctcss_descriptor() };
    table.tx_create = Some(injected_null_tx);
    TEST_TX_DESCRIPTOR.with(|slot| {
        struct Restore<'a>(&'a std::cell::Cell<*const Descriptor>, *const Descriptor);
        impl Drop for Restore<'_> {
            fn drop(&mut self) {
                self.0.set(self.1);
            }
        }
        let _restore = Restore(slot, slot.replace(&table));
        action()
    })
}

/// Validate the complete ABI once, before creating either callback-owned handle.
unsafe fn descriptor(pointer: *const Descriptor) -> Result<Descriptor, i32> {
    // SAFETY: a nonnull provider descriptor references at least the ABI prefix.
    let prefix = unsafe { pointer.cast::<DescriptorPrefix>().as_ref() }.ok_or(UNSUPPORTED)?;
    if prefix.abi_version != ABI_VERSION
        || prefix.struct_size < size_of::<Descriptor>() as u32
        || prefix.capability_name.is_null()
        // SAFETY: a nonnull capability points to a terminated provider string.
        || unsafe { CStr::from_ptr(prefix.capability_name) }.to_bytes() != CAPABILITY_NAME
    {
        return Err(UNSUPPORTED);
    }
    // SAFETY: the prefix size check established the complete descriptor span.
    let table = unsafe { *pointer };
    if table.rx_create.is_none()
        || table.rx_destroy.is_none()
        || table.rx_configure.is_none()
        || table.rx_enabled.is_none()
        || table.rx_decoded.is_none()
        || table.rx_tail_tone_active.is_none()
        || table.rx_clear_tail_on_carrier_loss.is_none()
        || table.rx_process.is_none()
        || table.tx_create.is_none()
        || table.tx_destroy.is_none()
        || table.legacy_frequency.is_none()
        || table.tx_generate.is_none()
    {
        return Err(UNSUPPORTED);
    }
    Ok(table)
}

/// One receive detector; all callback operations were cached during setup.
pub struct Receive {
    handle: NonNull<Rx>,
    destroy: unsafe extern "C" fn(*mut Rx),
    configure: unsafe extern "C" fn(*mut Rx, u64, bool),
    enabled: unsafe extern "C" fn(*const Rx) -> bool,
    decoded: unsafe extern "C" fn(*const Rx) -> i16,
    tail_tone_active: unsafe extern "C" fn(*const Rx) -> bool,
    clear_tail_on_carrier_loss: unsafe extern "C" fn(*mut Rx, bool),
    process: unsafe extern "C" fn(*mut Rx, *const f32, u32, bool) -> i16,
}

impl Receive {
    /// Create a detector from the released descriptor outside the callback.
    pub fn new() -> Result<Self, i32> {
        // SAFETY: the linked provider owns its process-lifetime descriptor.
        unsafe { Self::from_descriptor(rptadv_ctcss_descriptor()) }
    }

    unsafe fn from_descriptor(pointer: *const Descriptor) -> Result<Self, i32> {
        // SAFETY: setup owns a readable descriptor until its operations are copied.
        let table = unsafe { descriptor(pointer)? };
        let handle = NonNull::new(table.rx_create.expect("validated descriptor")())
            .ok_or(PROVIDER_FAILED)?;
        Ok(Self {
            handle,
            destroy: table.rx_destroy.expect("validated descriptor"),
            configure: table.rx_configure.expect("validated descriptor"),
            enabled: table.rx_enabled.expect("validated descriptor"),
            decoded: table.rx_decoded.expect("validated descriptor"),
            tail_tone_active: table.rx_tail_tone_active.expect("validated descriptor"),
            clear_tail_on_carrier_loss: table
                .rx_clear_tail_on_carrier_loss
                .expect("validated descriptor"),
            process: table.rx_process.expect("validated descriptor"),
        })
    }

    pub fn configure(&mut self, tone_mask: u64, relax: bool) {
        // SAFETY: setup owns this live handle and calls it serially.
        unsafe { (self.configure)(self.handle.as_ptr(), tone_mask, relax) }
    }

    pub fn enabled(&self) -> bool {
        // SAFETY: setup owns this live handle and calls it serially.
        unsafe { (self.enabled)(self.handle.as_ptr()) }
    }

    pub fn decoded(&self) -> i16 {
        // SAFETY: setup owns this live handle and calls it serially.
        unsafe { (self.decoded)(self.handle.as_ptr()) }
    }

    pub fn tail_tone_active(&self) -> bool {
        // SAFETY: setup owns this live handle and calls it serially.
        unsafe { (self.tail_tone_active)(self.handle.as_ptr()) }
    }

    pub fn clear_tail_on_carrier_loss(&mut self, carrier: bool) {
        // SAFETY: setup owns this live handle and calls it serially.
        unsafe { (self.clear_tail_on_carrier_loss)(self.handle.as_ptr(), carrier) }
    }

    /// Consume conditioned 8 kHz mono F32, including NULL zero spans for carrier loss.
    pub unsafe fn process(&mut self, samples: *const f32, count: u32, carrier: bool) -> i16 {
        // SAFETY: the caller provides count readable samples, or NULL only for zero count.
        unsafe { (self.process)(self.handle.as_ptr(), samples, count, carrier) }
    }
}

impl Drop for Receive {
    fn drop(&mut self) {
        // SAFETY: no callback owns this handle after its receiver is destroyed.
        unsafe { (self.destroy)(self.handle.as_ptr()) }
    }
}

/// One transmit phase accumulator; all callback operations were cached at setup.
pub struct Transmit {
    handle: NonNull<Tx>,
    destroy: unsafe extern "C" fn(*mut Tx),
    legacy_frequency: extern "C" fn(f64) -> f64,
    generate: unsafe extern "C" fn(*mut Tx, *mut f32, u32, f64, f32, u32, f64),
}

impl Transmit {
    /// Create phase state from the released descriptor outside the callback.
    pub fn new() -> Result<Self, i32> {
        // SAFETY: the linked provider owns its process-lifetime descriptor.
        unsafe { Self::from_descriptor(transmit_descriptor()) }
    }

    unsafe fn from_descriptor(pointer: *const Descriptor) -> Result<Self, i32> {
        // SAFETY: setup owns a readable descriptor until its operations are copied.
        let table = unsafe { descriptor(pointer)? };
        let handle = NonNull::new(table.tx_create.expect("validated descriptor")())
            .ok_or(PROVIDER_FAILED)?;
        Ok(Self {
            handle,
            destroy: table.tx_destroy.expect("validated descriptor"),
            legacy_frequency: table.legacy_frequency.expect("validated descriptor"),
            generate: table.tx_generate.expect("validated descriptor"),
        })
    }

    pub fn legacy_frequency(&self, frequency_hz: f64) -> f64 {
        (self.legacy_frequency)(frequency_hz)
    }

    /// Render 48 kHz mono F32 using this handle's persistent phase.
    pub unsafe fn generate(
        &mut self,
        output: *mut f32,
        frames: u32,
        frequency_hz: f64,
        peak: f32,
        enabled: u32,
        phase_shift_degrees: f64,
    ) {
        // SAFETY: the caller provides frames writable samples; setup owns this handle.
        unsafe {
            (self.generate)(
                self.handle.as_ptr(),
                output,
                frames,
                frequency_hz,
                peak,
                enabled,
                phase_shift_degrees,
            )
        }
    }
}

impl Drop for Transmit {
    fn drop(&mut self) {
        // SAFETY: no callback owns this handle after its transmitter is destroyed.
        unsafe { (self.destroy)(self.handle.as_ptr()) }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;
    use std::ptr;

    extern "C" fn null_rx() -> *mut Rx {
        ptr::null_mut()
    }

    extern "C" fn null_tx() -> *mut Tx {
        ptr::null_mut()
    }

    fn table() -> Descriptor {
        // SAFETY: the linked provider returns an immutable complete ABI-1 descriptor.
        unsafe { *rptadv_ctcss_descriptor() }
    }

    #[test]
    fn rejects_incomplete_descriptor_and_failed_creation() {
        assert!(matches!(
            unsafe { Receive::from_descriptor(ptr::null()) },
            Err(UNSUPPORTED)
        ));
        let mut prefix = DescriptorPrefix {
            struct_size: size_of::<DescriptorPrefix>() as u32,
            abi_version: ABI_VERSION,
            capability_name: c"rptadv.ctcss".as_ptr(),
        };
        assert!(matches!(
            unsafe { Receive::from_descriptor((&prefix as *const DescriptorPrefix).cast()) },
            Err(UNSUPPORTED)
        ));
        prefix.abi_version = 2;
        assert!(matches!(
            unsafe { Receive::from_descriptor((&prefix as *const DescriptorPrefix).cast()) },
            Err(UNSUPPORTED)
        ));
        let mut bad = table();
        bad.capability_name = ptr::null();
        assert!(matches!(
            unsafe { Receive::from_descriptor(&bad) },
            Err(UNSUPPORTED)
        ));
        bad.capability_name = c"wrong".as_ptr();
        assert!(matches!(
            unsafe { Receive::from_descriptor(&bad) },
            Err(UNSUPPORTED)
        ));

        macro_rules! absent {
            ($field:ident) => {{
                let mut bad = table();
                bad.$field = None;
                assert!(matches!(
                    unsafe { Receive::from_descriptor(&bad) },
                    Err(UNSUPPORTED)
                ));
            }};
        }
        absent!(rx_create);
        absent!(rx_destroy);
        absent!(rx_configure);
        absent!(rx_enabled);
        absent!(rx_decoded);
        absent!(rx_tail_tone_active);
        absent!(rx_clear_tail_on_carrier_loss);
        absent!(rx_process);
        absent!(tx_create);
        absent!(tx_destroy);
        absent!(legacy_frequency);
        absent!(tx_generate);

        let mut failed = table();
        failed.rx_create = Some(null_rx);
        assert!(matches!(
            unsafe { Receive::from_descriptor(&failed) },
            Err(PROVIDER_FAILED)
        ));
        failed = table();
        failed.tx_create = Some(null_tx);
        assert!(matches!(
            unsafe { Transmit::from_descriptor(&failed) },
            Err(PROVIDER_FAILED)
        ));
    }

    fn tone(output: &mut [f32], frequency: f32, phase_shift_at: Option<usize>) {
        for (index, sample) in output.iter_mut().enumerate() {
            let phase = 2.0 * core::f32::consts::PI * frequency * index as f32 / 8_000.0
                + if phase_shift_at.is_some_and(|at| index >= at) {
                    2.0 * core::f32::consts::PI / 3.0
                } else {
                    0.0
                };
            *sample = if phase.sin() >= 0.0 { 625.0 } else { -625.0 } / 32_768.0;
        }
    }

    #[test]
    fn shared_ctcss_matches_baseline() {
        let mut rx = Receive::new().expect("released RX descriptor");
        rx.configure(1 << 11, false);
        assert!(rx.enabled());
        let mut samples = [0.0; 8_000];
        tone(&mut samples, 100.0, None);
        for (index, block) in samples.chunks(160).enumerate() {
            let decoded = unsafe { rx.process(block.as_ptr(), block.len() as u32, true) };
            assert_eq!(decoded, if index < 10 { -1 } else { 11 });
            assert_eq!(decoded, rx.decoded());
        }
        let missing = [0.0; 160];
        assert_eq!(unsafe { rx.process(missing.as_ptr(), 160, true) }, 11);
        let mut shifted = [0.0; 480];
        tone(&mut shifted, 100.0, Some(320));
        assert_eq!(unsafe { rx.process(shifted.as_ptr(), 480, true) }, -1);
        rx.configure(1 << 11, false);
        for block in samples.chunks(160) {
            unsafe { rx.process(block.as_ptr(), block.len() as u32, true) };
        }
        let mut tail = [0.0; 256];
        tone(&mut tail, 55.0, None);
        assert_eq!(unsafe { rx.process(tail.as_ptr(), 256, true) }, -1);
        assert!(rx.tail_tone_active());
        // Zero-span carrier loss must reach the provider even with no audio pointer.
        assert_eq!(unsafe { rx.process(ptr::null(), 0, false) }, -1);
        assert!(!rx.tail_tone_active());
        rx.clear_tail_on_carrier_loss(false);
        assert!(!rx.tail_tone_active());

        let mut tx = Transmit::new().expect("released TX descriptor");
        let frequency = tx.legacy_frequency(114.8);
        assert_eq!(frequency.to_bits(), 0x405c_afc0_0000_0000);
        let mut output = [0.0; 960];
        unsafe { tx.generate(output.as_mut_ptr(), 960, frequency, 0.25, 1, 120.0) };
        let hash = output.iter().fold(0xcbf29ce484222325_u64, |hash, sample| {
            (hash ^ u64::from(sample.to_bits())).wrapping_mul(0x100000001b3)
        });
        assert_eq!(hash, 0xe8d7bfe4e4a1301e);

        let mut irregular = Receive::new().expect("independent RX handle");
        irregular.configure(1 << 11, false);
        let partitions = [17, 143, 1, 319, 80, 7, 233];
        let mut offset = 0;
        let mut partition = 0;
        while offset < samples.len() {
            let end = (offset + partitions[partition % partitions.len()]).min(samples.len());
            unsafe {
                irregular.process(samples[offset..end].as_ptr(), (end - offset) as u32, true)
            };
            offset = end;
            partition += 1;
        }
        assert_eq!(irregular.decoded(), 11);
    }
}
