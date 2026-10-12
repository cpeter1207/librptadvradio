//! Setup-validated client of the released DCS descriptor.

use std::{
    ffi::{CStr, c_char},
    mem::size_of,
    ptr::{self, NonNull},
};

use crate::session::{PROVIDER_FAILED, UNSUPPORTED};

const ABI_VERSION: u32 = 1;
const CAPABILITY_NAME: &[u8] = b"rptadv.dcs";

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
    code_supported: Option<extern "C" fn(i32) -> u32>,
    configured_code: Option<extern "C" fn(i32) -> i32>,
    receive_create: Option<unsafe extern "C" fn(*mut *mut Rx) -> i32>,
    receive_destroy: Option<unsafe extern "C" fn(*mut Rx)>,
    receive_configure: Option<unsafe extern "C" fn(*mut Rx, i32, u32) -> i32>,
    receive_valid: Option<unsafe extern "C" fn(*const Rx) -> u32>,
    receive_enabled: Option<unsafe extern "C" fn(*const Rx) -> u32>,
    receive_process: Option<unsafe extern "C" fn(*mut Rx, *const f32, u32, *mut u32) -> i32>,
    transmit_create: Option<unsafe extern "C" fn(*mut *mut Tx) -> i32>,
    transmit_destroy: Option<unsafe extern "C" fn(*mut Tx)>,
    transmit_configure: Option<unsafe extern "C" fn(*mut Tx, i32, u32) -> i32>,
    transmit_update_config_from: Option<unsafe extern "C" fn(*mut Tx, *const Tx) -> i32>,
    transmit_generate: Option<unsafe extern "C" fn(*mut Tx, *mut f32, u32, f32, u32, u32) -> i32>,
}

#[link(name = "rptadvdcs")]
unsafe extern "C" {
    fn rptadv_dcs_descriptor() -> *const Descriptor;
}

#[cfg(test)]
thread_local! {
    static TEST_DESCRIPTOR: std::cell::Cell<*const Descriptor> = const { std::cell::Cell::new(ptr::null()) };
}

fn provider_descriptor() -> *const Descriptor {
    #[cfg(test)]
    {
        let pointer = TEST_DESCRIPTOR.with(|slot| slot.get());
        if !pointer.is_null() {
            return pointer;
        }
    }
    // SAFETY: the linked provider owns this process-lifetime descriptor.
    unsafe { rptadv_dcs_descriptor() }
}

#[cfg(test)]
pub(crate) enum TestFault {
    RxCreateFailed,
    TxCreateFailed,
    TxConfigureFailed,
    TxUpdateFailed,
    TxGenerateFailed,
    Unsupported,
}

#[cfg(test)]
unsafe extern "C" fn injected_rx_create_failure(_: *mut *mut Rx) -> i32 {
    -1
}

#[cfg(test)]
unsafe extern "C" fn injected_tx_create_failure(_: *mut *mut Tx) -> i32 {
    -1
}

#[cfg(test)]
unsafe extern "C" fn injected_tx_configure_failure(_: *mut Tx, _: i32, _: u32) -> i32 {
    -1
}

#[cfg(test)]
unsafe extern "C" fn injected_tx_update_failure(_: *mut Tx, _: *const Tx) -> i32 {
    -1
}

#[cfg(test)]
unsafe extern "C" fn injected_tx_generate_failure(
    _: *mut Tx,
    _: *mut f32,
    _: u32,
    _: f32,
    _: u32,
    _: u32,
) -> i32 {
    -1
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
pub(crate) fn with_test_fault<T>(fault: TestFault, action: impl FnOnce() -> T) -> T {
    // SAFETY: the released provider returns a complete immutable descriptor.
    let mut table = unsafe { *rptadv_dcs_descriptor() };
    match fault {
        TestFault::RxCreateFailed => table.receive_create = Some(injected_rx_create_failure),
        TestFault::TxCreateFailed => table.transmit_create = Some(injected_tx_create_failure),
        TestFault::TxConfigureFailed => {
            table.transmit_configure = Some(injected_tx_configure_failure)
        }
        TestFault::TxUpdateFailed => {
            table.transmit_update_config_from = Some(injected_tx_update_failure)
        }
        TestFault::TxGenerateFailed => table.transmit_generate = Some(injected_tx_generate_failure),
        TestFault::Unsupported => table.abi_version = 2,
    }
    TEST_DESCRIPTOR.with(|slot| {
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

/// Resolve the complete function table before any handle enters a session.
unsafe fn descriptor(pointer: *const Descriptor) -> Result<Descriptor, i32> {
    // SAFETY: a nonnull provider descriptor references at least the ABI prefix.
    let prefix = unsafe { pointer.cast::<DescriptorPrefix>().as_ref() }.ok_or(UNSUPPORTED)?;
    if prefix.abi_version != ABI_VERSION
        || prefix.struct_size < size_of::<Descriptor>() as u32
        || prefix.capability_name.is_null()
        // SAFETY: a nonnull provider capability points to terminated storage.
        || unsafe { CStr::from_ptr(prefix.capability_name) }.to_bytes() != CAPABILITY_NAME
    {
        return Err(UNSUPPORTED);
    }
    // SAFETY: the prefix size check established the complete table span.
    let table = unsafe { *pointer };
    if table.code_supported.is_none()
        || table.configured_code.is_none()
        || table.receive_create.is_none()
        || table.receive_destroy.is_none()
        || table.receive_configure.is_none()
        || table.receive_valid.is_none()
        || table.receive_enabled.is_none()
        || table.receive_process.is_none()
        || table.transmit_create.is_none()
        || table.transmit_destroy.is_none()
        || table.transmit_configure.is_none()
        || table.transmit_update_config_from.is_none()
        || table.transmit_generate.is_none()
    {
        return Err(UNSUPPORTED);
    }
    Ok(table)
}

fn status(value: i32) -> Result<(), i32> {
    if value == 0 {
        Ok(())
    } else {
        Err(PROVIDER_FAILED)
    }
}

/// Validate a DCS code through the released provider during setup.
pub fn code_supported(code: i32) -> Result<bool, i32> {
    // SAFETY: setup reads the process-lifetime provider descriptor.
    let table = unsafe { descriptor(provider_descriptor())? };
    Ok(table.code_supported.expect("validated descriptor")(code) != 0)
}

/// One callback-owned DCS receiver with cached operations.
pub struct Receive {
    handle: NonNull<Rx>,
    destroy: unsafe extern "C" fn(*mut Rx),
    configure: unsafe extern "C" fn(*mut Rx, i32, u32) -> i32,
    valid: unsafe extern "C" fn(*const Rx) -> u32,
    enabled: unsafe extern "C" fn(*const Rx) -> u32,
    process: unsafe extern "C" fn(*mut Rx, *const f32, u32, *mut u32) -> i32,
}

impl Receive {
    /// Allocate receive state outside the audio callback.
    pub fn new() -> Result<Self, i32> {
        // SAFETY: the linked provider owns its process-lifetime descriptor.
        unsafe { Self::from_descriptor(provider_descriptor()) }
    }

    unsafe fn from_descriptor(pointer: *const Descriptor) -> Result<Self, i32> {
        // SAFETY: setup owns a readable descriptor until its operations are copied.
        let table = unsafe { descriptor(pointer)? };
        let mut handle = ptr::null_mut();
        // SAFETY: out points to writable setup storage.
        status(unsafe { table.receive_create.expect("validated descriptor")(&mut handle) })?;
        let handle = NonNull::new(handle).ok_or(PROVIDER_FAILED)?;
        Ok(Self {
            handle,
            destroy: table.receive_destroy.expect("validated descriptor"),
            configure: table.receive_configure.expect("validated descriptor"),
            valid: table.receive_valid.expect("validated descriptor"),
            enabled: table.receive_enabled.expect("validated descriptor"),
            process: table.receive_process.expect("validated descriptor"),
        })
    }

    /// Change receive code and polarity, clearing the provider's qualification.
    pub fn configure(&mut self, code: i32, inverted: u32) -> Result<(), i32> {
        // SAFETY: setup owns the live handle serially.
        status(unsafe { (self.configure)(self.handle.as_ptr(), code, inverted) })
    }

    pub fn valid(&self) -> bool {
        // SAFETY: the receive owner owns this live handle.
        unsafe { (self.valid)(self.handle.as_ptr()) != 0 }
    }

    pub fn enabled(&self) -> bool {
        // SAFETY: the receive owner owns this live handle.
        unsafe { (self.enabled)(self.handle.as_ptr()) != 0 }
    }

    /// Consume nonempty native stereo PCM; the caller keeps zero spans as no-ops.
    pub unsafe fn process(&mut self, stereo: *const f32, frames: u32) -> Result<bool, i32> {
        let mut valid = 0;
        // SAFETY: the caller provides two readable samples per frame, owned serially.
        status(unsafe { (self.process)(self.handle.as_ptr(), stereo, frames, &mut valid) })?;
        Ok(valid != 0)
    }
}

impl Drop for Receive {
    fn drop(&mut self) {
        // SAFETY: no audio callback owns this handle after its receiver is dropped.
        unsafe { (self.destroy)(self.handle.as_ptr()) }
    }
}

/// One callback-owned DCS transmitter with cached operations.
pub struct Transmit {
    handle: NonNull<Tx>,
    destroy: unsafe extern "C" fn(*mut Tx),
    configure: unsafe extern "C" fn(*mut Tx, i32, u32) -> i32,
    update_config_from: unsafe extern "C" fn(*mut Tx, *const Tx) -> i32,
    generate: unsafe extern "C" fn(*mut Tx, *mut f32, u32, f32, u32, u32) -> i32,
}

impl Transmit {
    /// Allocate transmit state outside the audio callback.
    pub fn new() -> Result<Self, i32> {
        // SAFETY: the linked provider owns its process-lifetime descriptor.
        unsafe { Self::from_descriptor(provider_descriptor()) }
    }

    unsafe fn from_descriptor(pointer: *const Descriptor) -> Result<Self, i32> {
        // SAFETY: setup owns a readable descriptor until its operations are copied.
        let table = unsafe { descriptor(pointer)? };
        let mut handle = ptr::null_mut();
        // SAFETY: out points to writable setup storage.
        status(unsafe { table.transmit_create.expect("validated descriptor")(&mut handle) })?;
        let handle = NonNull::new(handle).ok_or(PROVIDER_FAILED)?;
        Ok(Self {
            handle,
            destroy: table.transmit_destroy.expect("validated descriptor"),
            configure: table.transmit_configure.expect("validated descriptor"),
            update_config_from: table
                .transmit_update_config_from
                .expect("validated descriptor"),
            generate: table.transmit_generate.expect("validated descriptor"),
        })
    }

    /// Compatibility configure resets fractional bit timing during setup.
    pub fn configure(&mut self, code: i32, inverted: u32) -> Result<(), i32> {
        // SAFETY: setup owns the live handle serially.
        status(unsafe { (self.configure)(self.handle.as_ptr(), code, inverted) })
    }

    /// Copy prepared code/polarity while retaining live clocks and tail phase.
    pub fn update_config_from(&mut self, prepared: &Self) -> Result<(), i32> {
        // SAFETY: the two session-owned handles are distinct and borrowed serially.
        status(unsafe { (self.update_config_from)(self.handle.as_ptr(), prepared.handle.as_ptr()) })
    }

    /// Render mono native PCM from cached provider code without setup work.
    pub unsafe fn generate(
        &mut self,
        output: *mut f32,
        frames: u32,
        peak: f32,
        enabled: u32,
        turnoff: u32,
    ) -> Result<(), i32> {
        // SAFETY: caller supplies frames writable samples; TX owner calls serially.
        status(unsafe {
            (self.generate)(self.handle.as_ptr(), output, frames, peak, enabled, turnoff)
        })
    }
}

impl Drop for Transmit {
    fn drop(&mut self) {
        // SAFETY: no audio callback owns this handle after its transmitter is dropped.
        unsafe { (self.destroy)(self.handle.as_ptr()) }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    fn table() -> Descriptor {
        // SAFETY: the linked released provider returns a complete immutable descriptor.
        unsafe { *rptadv_dcs_descriptor() }
    }

    unsafe extern "C" fn failed_rx_create(_: *mut *mut Rx) -> i32 {
        -1
    }

    unsafe extern "C" fn null_rx_create(out: *mut *mut Rx) -> i32 {
        unsafe { out.write(ptr::null_mut()) };
        0
    }

    unsafe extern "C" fn failed_tx_create(_: *mut *mut Tx) -> i32 {
        -1
    }

    unsafe extern "C" fn null_tx_create(out: *mut *mut Tx) -> i32 {
        unsafe { out.write(ptr::null_mut()) };
        0
    }

    unsafe extern "C" fn failed_rx_configure(_: *mut Rx, _: i32, _: u32) -> i32 {
        -1
    }

    unsafe extern "C" fn failed_rx_process(_: *mut Rx, _: *const f32, _: u32, _: *mut u32) -> i32 {
        -1
    }

    unsafe extern "C" fn failed_tx_configure(_: *mut Tx, _: i32, _: u32) -> i32 {
        -1
    }

    unsafe extern "C" fn failed_tx_update(_: *mut Tx, _: *const Tx) -> i32 {
        -1
    }

    unsafe extern "C" fn failed_tx_generate(
        _: *mut Tx,
        _: *mut f32,
        _: u32,
        _: f32,
        _: u32,
        _: u32,
    ) -> i32 {
        -1
    }

    #[test]
    fn rejects_incomplete_descriptor_and_failed_lifecycle() {
        assert!(matches!(
            unsafe { Receive::from_descriptor(ptr::null()) },
            Err(UNSUPPORTED)
        ));
        let mut prefix = DescriptorPrefix {
            struct_size: size_of::<DescriptorPrefix>() as u32,
            abi_version: ABI_VERSION,
            capability_name: c"rptadv.dcs".as_ptr(),
        };
        assert!(matches!(
            unsafe { Receive::from_descriptor((&prefix as *const DescriptorPrefix).cast()) },
            Err(UNSUPPORTED)
        ));
        prefix.abi_version = 2;
        assert!(matches!(
            unsafe { Transmit::from_descriptor((&prefix as *const DescriptorPrefix).cast()) },
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
        absent!(code_supported);
        absent!(configured_code);
        absent!(receive_create);
        absent!(receive_destroy);
        absent!(receive_configure);
        absent!(receive_valid);
        absent!(receive_enabled);
        absent!(receive_process);
        absent!(transmit_create);
        absent!(transmit_destroy);
        absent!(transmit_configure);
        absent!(transmit_update_config_from);
        absent!(transmit_generate);

        let mut failed = table();
        failed.receive_create = Some(failed_rx_create);
        assert!(matches!(
            unsafe { Receive::from_descriptor(&failed) },
            Err(PROVIDER_FAILED)
        ));
        failed.receive_create = Some(null_rx_create);
        assert!(matches!(
            unsafe { Receive::from_descriptor(&failed) },
            Err(PROVIDER_FAILED)
        ));
        failed = table();
        failed.transmit_create = Some(failed_tx_create);
        assert!(matches!(
            unsafe { Transmit::from_descriptor(&failed) },
            Err(PROVIDER_FAILED)
        ));
        failed.transmit_create = Some(null_tx_create);
        assert!(matches!(
            unsafe { Transmit::from_descriptor(&failed) },
            Err(PROVIDER_FAILED)
        ));
    }

    #[test]
    fn shared_dcs_matches_baseline_and_preserves_prepared_update() {
        assert!(code_supported(0o023).unwrap());
        assert!(!code_supported(-1).unwrap());
        let table = table();
        assert_eq!(table.configured_code.unwrap()(-1), -1);
        assert_eq!(table.configured_code.unwrap()(0o777), 0o777);

        let mut rx = Receive::new().unwrap();
        rx.configure(0o023, 0).unwrap();
        assert!(rx.enabled());
        let mut pcm = vec![0.0_f32; 48_000 * 2];
        let word = 0x76_3813_u32;
        for (index, frame) in pcm.chunks_exact_mut(2).enumerate() {
            let bit = (index as u64 * 1_344 / 480_000) % 23;
            frame[0] = if word & (1 << bit) != 0 {
                0.375
            } else {
                -0.375
            };
            frame[1] = -frame[0];
        }
        for block in pcm.chunks(514) {
            unsafe { rx.process(block.as_ptr(), (block.len() / 2) as u32) }.unwrap();
        }
        assert!(rx.valid());

        let mut whole = Transmit::new().unwrap();
        let mut split = Transmit::new().unwrap();
        let mut reference_new = Transmit::new().unwrap();
        let mut prepared = Transmit::new().unwrap();
        for tx in [&mut whole, &mut split] {
            tx.configure(0o023, 0).unwrap();
        }
        for tx in [&mut reference_new, &mut prepared] {
            tx.configure(0o025, 1).unwrap();
        }
        let mut expected = [0.0; 9_217];
        let mut actual = [0.0; 9_217];
        let mut old = [0.0; 9_217];
        unsafe { whole.generate(expected.as_mut_ptr(), 9_217, 0.25, 1, 0) }.unwrap();
        for block in actual.chunks_mut(791) {
            unsafe { split.generate(block.as_mut_ptr(), block.len() as u32, 0.25, 1, 0) }.unwrap();
        }
        assert_eq!(actual, expected);
        assert_eq!(expected[0], 0.25);
        unsafe { reference_new.generate(expected.as_mut_ptr(), 9_217, 0.25, 1, 0) }.unwrap();
        split.update_config_from(&prepared).unwrap();
        unsafe { reference_new.generate(expected.as_mut_ptr(), 9_217, 0.25, 1, 0) }.unwrap();
        unsafe { split.generate(actual.as_mut_ptr(), 9_217, 0.25, 1, 0) }.unwrap();
        unsafe { whole.generate(old.as_mut_ptr(), 9_217, 0.25, 1, 0) }.unwrap();
        assert_eq!(actual, expected);
        assert_ne!(actual, old);
        assert!(rx.valid());
        rx.configure(0o777, 1).unwrap();
        assert!(!rx.valid());
    }

    #[test]
    fn maps_provider_operation_failures() {
        let mut rx = Receive::new().unwrap();
        rx.configure = failed_rx_configure;
        assert_eq!(rx.configure(0o023, 0), Err(PROVIDER_FAILED));
        rx.process = failed_rx_process;
        assert_eq!(unsafe { rx.process(ptr::null(), 0) }, Err(PROVIDER_FAILED));

        let mut tx = Transmit::new().unwrap();
        let prepared = Transmit::new().unwrap();
        tx.configure = failed_tx_configure;
        assert_eq!(tx.configure(0o023, 0), Err(PROVIDER_FAILED));
        tx.update_config_from = failed_tx_update;
        assert_eq!(tx.update_config_from(&prepared), Err(PROVIDER_FAILED));
        tx.generate = failed_tx_generate;
        assert_eq!(
            unsafe { tx.generate(ptr::null_mut(), 0, 0.25, 1, 0) },
            Err(PROVIDER_FAILED)
        );
    }
}
