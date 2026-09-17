//! Portable whole-session radio engine for rpt_advanced.
//!
//! ABI 4 owns independent variable-frame receive and transmit workers at the
//! fixed 48 kHz native rate.  Platform adapters provide only prepared external
//! processing and program-ring ports plus hardware/control snapshots.

#![deny(warnings)]
#![cfg_attr(coverage, feature(coverage_attribute))]

use std::ffi::{c_char, c_int};
use std::mem::size_of;

mod audio_meter;
mod calibrated_test_tone;
mod center_slicer;
mod ctcss_receive;
mod ctcss_render_state;
mod ctcss_transmit;
mod dcs_receive;
mod dcs_transmit;
mod dcs_turnoff;
mod deemphasis_integrator;
mod delay_line;
mod envelope_meter;
mod fir;
mod micor_squelch;
mod receive_frontend;
mod receive_path;
mod receive_profiles;
mod receive_qualification;
mod rx_blanking;
mod rx_cpu_saver;
mod session;
mod signal_mode;
mod timer;
mod transmit_signaling;
mod tx_complete;
mod tx_cpu_saver;
mod tx_finish;

pub(crate) use ctcss_render_state::{
    CtcssRenderState, CtcssRenderStateConfig, CtcssRenderStateInput,
};
pub(crate) use dcs_turnoff::{DcsTurnoffConfig, DcsTurnoffInput, DcsTurnoffState};
pub(crate) use rx_cpu_saver::{RxCpuSaverInput, RxCpuSaverState};
pub(crate) use signal_mode::{SignalModeConfig, SignalModeInput, SignalModeState};
pub(crate) use tx_complete::{TxCompleteConfig, TxCompleteState};
pub(crate) use tx_cpu_saver::{TxCpuSaverInput, TxCpuSaverState};
pub(crate) use tx_finish::{TxFinishInput, TxFinishState};

const ABI_VERSION: u32 = session::ABI_VERSION;
#[cfg(test)]
const RADIO_OK: c_int = 0;
const RADIO_INVALID_ARGUMENT: c_int = -1;
const CAPABILITY_NAME: &[u8] = b"rptadv.radio-core\0";

/// ABI 4 whole-session function table.
#[repr(C)]
pub struct RadioDescriptor {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
    session_create: unsafe extern "C" fn(
        *const session::SessionConfig,
        *const session::SessionPorts,
        *mut *mut session::Session,
    ) -> c_int,
    session_warm: unsafe extern "C" fn(*mut session::Session) -> c_int,
    session_receive: unsafe extern "C" fn(
        *mut session::Session,
        *const f32,
        *mut f32,
        u32,
        *const session::ReceiveInput,
        *mut session::ReceiveResult,
    ) -> c_int,
    session_transmit: unsafe extern "C" fn(
        *mut session::Session,
        *mut f32,
        u32,
        *const session::TransmitInput,
        *mut session::TransmitResult,
    ) -> c_int,
    session_snapshot:
        unsafe extern "C" fn(*const session::Session, *mut session::Snapshot) -> c_int,
    session_pop_receive_event:
        unsafe extern "C" fn(*const session::Session, *mut session::Event) -> u32,
    session_pop_transmit_event:
        unsafe extern "C" fn(*const session::Session, *mut session::Event) -> u32,
    session_destroy: unsafe extern "C" fn(*mut session::Session),
}

// SAFETY: the descriptor and all addresses referenced by it are immutable.
unsafe impl Sync for RadioDescriptor {}

static DESCRIPTOR: RadioDescriptor = RadioDescriptor {
    struct_size: size_of::<RadioDescriptor>() as u32,
    abi_version: ABI_VERSION,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    session_create: session::create,
    session_warm: session::warm,
    session_receive: session::receive,
    session_transmit: session::transmit,
    session_snapshot: session::snapshot,
    session_pop_receive_event: session::pop_receive_event,
    session_pop_transmit_event: session::pop_transmit_event,
    session_destroy: session::destroy,
};

/// Return the immutable ABI 4 whole-session function table.
#[unsafe(no_mangle)]
pub extern "C" fn rptadv_radio_descriptor() -> *const RadioDescriptor {
    &DESCRIPTOR
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn descriptor_is_immutable_and_identifies_the_current_abi() {
        let pointer = rptadv_radio_descriptor();
        assert_eq!(pointer, rptadv_radio_descriptor());
        let descriptor = unsafe { &*pointer };
        assert_eq!(
            descriptor.struct_size as usize,
            size_of::<RadioDescriptor>()
        );
        assert_eq!(descriptor.abi_version, 4);
        assert_eq!(
            unsafe { std::ffi::CStr::from_ptr(descriptor.capability_name) }.to_bytes(),
            b"rptadv.radio-core"
        );
    }
}
