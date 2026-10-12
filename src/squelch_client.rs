//! Setup-validated client of the released squelch descriptor.

use std::{
    ffi::{CStr, c_char},
    mem::size_of,
    ptr::NonNull,
};

use crate::session::{PROVIDER_FAILED, UNSUPPORTED};

const ABI_VERSION: u32 = 2;
const CAPABILITY_NAME: &[u8] = b"rptadv.squelch";

#[repr(C)]
struct Squelch {
    _private: [u8; 0],
}

#[repr(C)]
struct DescriptorPrefix {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
}

#[repr(C)]
struct Descriptor {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
    create: Option<unsafe extern "C" fn(*mut *mut Squelch) -> i32>,
    destroy: Option<unsafe extern "C" fn(*mut Squelch)>,
    noise_update: Option<unsafe extern "C" fn(*mut Squelch, u32, f64, u32, u32) -> u32>,
    vox_advance: Option<unsafe extern "C" fn(*mut Squelch, u32, u32, u32)>,
    vox_finish: Option<unsafe extern "C" fn(*mut Squelch, u32) -> u32>,
}

#[link(name = "rptadvsquelch")]
unsafe extern "C" {
    fn rptadv_squelch_descriptor() -> *const Descriptor;
}

/// One setup-owned detector handle with cached callback-safe operations.
pub struct Client {
    handle: NonNull<Squelch>,
    destroy: unsafe extern "C" fn(*mut Squelch),
    noise_update: unsafe extern "C" fn(*mut Squelch, u32, f64, u32, u32) -> u32,
    vox_advance: unsafe extern "C" fn(*mut Squelch, u32, u32, u32),
    vox_finish: unsafe extern "C" fn(*mut Squelch, u32) -> u32,
}

impl Client {
    /// Resolve ABI 2 and allocate one handle outside the audio callback.
    pub fn new() -> Result<Self, i32> {
        // SAFETY: the linked provider owns its process-lifetime descriptor.
        unsafe { Self::from_descriptor(rptadv_squelch_descriptor()) }
    }

    /// Validate the descriptor prefix before reading its complete function table.
    unsafe fn from_descriptor(pointer: *const Descriptor) -> Result<Self, i32> {
        // SAFETY: a nonnull descriptor pointer references at least the ABI prefix.
        let prefix = unsafe { pointer.cast::<DescriptorPrefix>().as_ref() }.ok_or(UNSUPPORTED)?;
        if prefix.abi_version != ABI_VERSION
            || prefix.struct_size < size_of::<Descriptor>() as u32
            || prefix.capability_name.is_null()
            // SAFETY: a nonnull provider capability is a terminated C string.
            || unsafe { CStr::from_ptr(prefix.capability_name) }.to_bytes() != CAPABILITY_NAME
        {
            return Err(UNSUPPORTED);
        }
        // SAFETY: the size check above established the complete descriptor span.
        let descriptor = unsafe { &*pointer };
        let (Some(create), Some(destroy), Some(noise_update), Some(vox_advance), Some(vox_finish)) = (
            descriptor.create,
            descriptor.destroy,
            descriptor.noise_update,
            descriptor.vox_advance,
            descriptor.vox_finish,
        ) else {
            return Err(UNSUPPORTED);
        };
        let mut raw = std::ptr::null_mut();
        if unsafe { create(&mut raw) } != 0 {
            return Err(PROVIDER_FAILED);
        }
        let handle = NonNull::new(raw).ok_or(PROVIDER_FAILED)?;
        Ok(Self {
            handle,
            destroy,
            noise_update,
            vox_advance,
            vox_finish,
        })
    }

    /// Advance one prevalidated discriminator sample and return the closed gate.
    pub fn noise_update(
        &mut self,
        previous_closed: bool,
        power: f64,
        level: u32,
        hysteresis: u32,
    ) -> bool {
        // SAFETY: setup created this handle; its receive owner calls serially.
        unsafe {
            (self.noise_update)(
                self.handle.as_ptr(),
                u32::from(previous_closed),
                power,
                level,
                hysteresis,
            ) != 0
        }
    }

    /// Advance the VOX timer to an emitted sample's increasing native offset.
    pub fn vox_advance(&mut self, native_offset: u32, qualified: bool, hang_frames: u32) {
        // SAFETY: setup created this handle; its receive owner calls serially.
        unsafe {
            (self.vox_advance)(
                self.handle.as_ptr(),
                native_offset,
                u32::from(qualified),
                hang_frames,
            )
        }
    }

    /// Consume a span's trailing frames and return whether VOX hold remains.
    pub fn vox_finish(&mut self, native_frames: u32) -> bool {
        // SAFETY: setup created this handle; its receive owner calls serially.
        unsafe { (self.vox_finish)(self.handle.as_ptr(), native_frames) != 0 }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // SAFETY: no callback owns this handle when its receiver is destroyed.
        unsafe { (self.destroy)(self.handle.as_ptr()) };
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{Client, Descriptor, DescriptorPrefix, Squelch};
    use crate::session::{PROVIDER_FAILED, UNSUPPORTED};
    use std::{
        mem::size_of,
        ptr,
        sync::atomic::{AtomicUsize, Ordering},
    };

    static DESTROYS: AtomicUsize = AtomicUsize::new(0);

    unsafe extern "C" fn create(out: *mut *mut Squelch) -> i32 {
        unsafe { *out = Box::into_raw(Box::new(0_u8)).cast() };
        0
    }

    unsafe extern "C" fn create_null(out: *mut *mut Squelch) -> i32 {
        unsafe { *out = ptr::null_mut() };
        0
    }

    unsafe extern "C" fn create_error(_: *mut *mut Squelch) -> i32 {
        -1
    }

    unsafe extern "C" fn destroy(handle: *mut Squelch) {
        // SAFETY: create returned this Box allocation.
        drop(unsafe { Box::from_raw(handle.cast::<u8>()) });
        DESTROYS.fetch_add(1, Ordering::Relaxed);
    }

    unsafe extern "C" fn noise_update(
        _: *mut Squelch,
        previous: u32,
        _: f64,
        _: u32,
        _: u32,
    ) -> u32 {
        if previous == 0 { 2 } else { 0 }
    }

    unsafe extern "C" fn vox_advance(_: *mut Squelch, _: u32, _: u32, _: u32) {}

    unsafe extern "C" fn vox_finish(_: *mut Squelch, frames: u32) -> u32 {
        if frames != 0 { 2 } else { 0 }
    }

    fn descriptor() -> Descriptor {
        Descriptor {
            struct_size: size_of::<Descriptor>() as u32,
            abi_version: 2,
            capability_name: c"rptadv.squelch".as_ptr(),
            create: Some(create),
            destroy: Some(destroy),
            noise_update: Some(noise_update),
            vox_advance: Some(vox_advance),
            vox_finish: Some(vox_finish),
        }
    }

    #[test]
    fn released_abi2_descriptor_creates_a_live_squelch_handle() {
        let mut client = Client::new().expect("released ABI 2 descriptor");
        assert!(client.noise_update(false, 0.0, 0, 0));
    }

    #[test]
    fn rejects_incomplete_or_wrong_descriptor_before_allocation() {
        assert!(matches!(
            unsafe { Client::from_descriptor(ptr::null()) },
            Err(UNSUPPORTED)
        ));
        let mut prefix = DescriptorPrefix {
            struct_size: size_of::<DescriptorPrefix>() as u32,
            abi_version: 2,
            capability_name: c"rptadv.squelch".as_ptr(),
        };
        assert!(matches!(
            unsafe { Client::from_descriptor((&prefix as *const DescriptorPrefix).cast()) },
            Err(UNSUPPORTED)
        ));
        prefix.abi_version = 1;
        assert!(matches!(
            unsafe { Client::from_descriptor((&prefix as *const DescriptorPrefix).cast()) },
            Err(UNSUPPORTED)
        ));

        let mut table = descriptor();
        table.capability_name = ptr::null();
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(UNSUPPORTED)
        ));
        table.capability_name = c"other".as_ptr();
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(UNSUPPORTED)
        ));

        let mut table = descriptor();
        table.create = None;
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(UNSUPPORTED)
        ));
        let mut table = descriptor();
        table.destroy = None;
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(UNSUPPORTED)
        ));
        let mut table = descriptor();
        table.noise_update = None;
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(UNSUPPORTED)
        ));
        let mut table = descriptor();
        table.vox_advance = None;
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(UNSUPPORTED)
        ));
        let mut table = descriptor();
        table.vox_finish = None;
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(UNSUPPORTED)
        ));
    }

    #[test]
    fn failed_create_is_reported_and_valid_handle_is_released() {
        let mut table = descriptor();
        table.create = Some(create_null);
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(PROVIDER_FAILED)
        ));
        table.create = Some(create_error);
        assert!(matches!(
            unsafe { Client::from_descriptor(&table) },
            Err(PROVIDER_FAILED)
        ));

        let previous_destroys = DESTROYS.load(Ordering::Relaxed);
        let mut client = unsafe { Client::from_descriptor(&descriptor()) }.expect("valid table");
        assert!(!client.noise_update(true, 0.0, 0, 0));
        assert!(client.noise_update(false, 0.0, 0, 0));
        client.vox_advance(1, true, 2);
        assert!(client.vox_finish(1));
        drop(client);
        assert_eq!(DESTROYS.load(Ordering::Relaxed), previous_destroys + 1);
    }
}
