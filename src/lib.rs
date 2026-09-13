//! Portable, Asterisk-free native-tick boundary for rpt_advanced radio ports.
//!
//! The first ABI intentionally provides only setup validation and safe silent
//! output.  It establishes the real-time, variable-frame f32 contract before
//! radio behavior migrates from compatibility adapters.

#![deny(warnings)]
#![cfg_attr(coverage, feature(coverage_attribute))]

use std::ffi::{c_char, c_int};
use std::mem::size_of;
use std::ptr::{self, NonNull};

#[cfg(test)]
#[path = "tests/allocation.rs"]
mod allocation_test;
mod audio_meter;
mod calibrated_test_tone;
mod center_slicer;
mod config_parse;
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
mod parrot;
mod receive_frontend;
/// Native receive-path composition prepared for the owned M1 engine.
pub mod receive_path;
/// Receive-qualification policy prepared for the forthcoming owned radio engine.
///
/// This Rust-only composition module has no C ABI entry point.  It is public
/// within the Rust crate surface so the owned engine can reuse one policy
/// implementation without a compatibility-adapter scalar callback.
pub mod receive_qualification;
mod rx_blanking;
mod rx_cpu_saver;
mod signal_mode;
/// Owned receive/transmit signaling composition; no additional scalar C ABI.
pub mod signaling_engine;
mod timer;
mod transmit_renderer;
/// Transmit-signaling composition prepared for the owned M1 engine.
pub mod transmit_signaling;
mod tx_complete;
mod tx_cpu_saver;
mod tx_finish;
mod vox_carrier;

pub use ctcss_render_state::{CtcssRenderState, CtcssRenderStateConfig, CtcssRenderStateInput};
pub use dcs_turnoff::{DcsTurnoffConfig, DcsTurnoffInput, DcsTurnoffState};
pub use rx_blanking::{RxBlankingInput, RxBlankingState};
pub use rx_cpu_saver::{RxCpuSaverInput, RxCpuSaverState};
pub use signal_mode::{SignalModeConfig, SignalModeInput, SignalModeState};
pub use tx_complete::{TxCompleteConfig, TxCompleteState};
pub use tx_cpu_saver::{TxCpuSaverInput, TxCpuSaverState};
pub use tx_finish::{TxFinishInput, TxFinishState};
pub use vox_carrier::{VoxCarrierInput, VoxCarrierState};

const ABI_VERSION: u32 = 1;
const CANONICAL_CHANNELS: u32 = 2;
const RADIO_OK: c_int = 0;
const RADIO_INVALID_ARGUMENT: c_int = -1;
const RADIO_FRAME_COUNT_EXCEEDED: c_int = -3;
const RADIO_UNSUPPORTED: c_int = -4;
const CAPABILITY_NAME: &[u8] = b"rptadv.radio-core\0";

/// Number of historical 20 ms audio-meter slots in the public ABI.
const AUDIO_STATS_LEN: usize = audio_meter::STATS_LEN;

/// Opaque state retained for one fixed-rate native stream.
#[repr(C)]
pub struct Radio {
    maximum_frame_count: u32,
    interleaved_channels: u32,
    ctcss_phase_radians: f64,
    ctcss_receive: ctcss_receive::ReceiveState,
    dcs_transmit: dcs_transmit::TransmitState,
    dcs_receive: dcs_receive::ReceiveState,
    calibrated_test_tone: calibrated_test_tone::State,
    native_parrot: parrot::State,
}

/// C-compatible setup parameters for one native radio stream.
#[repr(C)]
struct RadioConfig {
    struct_size: u32,
    abi_version: u32,
    native_sample_rate_hz: u32,
    maximum_frame_count: u32,
    interleaved_channels: u32,
}

/// C-compatible routing and calibration for the transmitter renderer.
#[repr(C)]
#[derive(Clone, Copy)]
struct TransmitRenderConfig {
    struct_size: u32,
    output_a_route: u32,
    output_b_route: u32,
    ctcss_peak_a: f32,
    ctcss_bias_a: f32,
    ctcss_peak_b: f32,
    ctcss_bias_b: f32,
}

/// C-compatible receiver measurements returned by one extract operation.
#[repr(C)]
struct ReceiveExtractStats {
    peak: f32,
    rail_samples: u64,
}

/// C-compatible native-parrot cursor and recording status.
#[repr(C)]
struct NativeParrotStatus {
    recorded_samples: u64,
    playback_offset: u64,
    playing: u32,
    truncated: u32,
}

/// C-compatible selected-tone configuration for receive CTCSS detection.
#[repr(C)]
struct CtcssReceiveConfig {
    struct_size: u32,
    tone_mask: u64,
    relax: u32,
}

/// C-compatible ASL3 audio-meter ring retained by compatibility adapters.
#[repr(C)]
#[derive(Clone, Copy)]
struct AudioStatistics {
    maxbuf: [u16; AUDIO_STATS_LEN],
    clipbuf: [u16; AUDIO_STATS_LEN],
    pwrbuf: [u32; AUDIO_STATS_LEN],
    index: i16,
}

/// C-compatible function table exported through the descriptor symbol.
#[repr(C)]
pub struct RadioDescriptor {
    struct_size: u32,
    abi_version: u32,
    capability_name: *const c_char,
    radio_create: extern "C" fn(*const RadioConfig, *mut *mut Radio) -> c_int,
    radio_tick: extern "C" fn(*mut Radio, *const f32, *mut f32, u32) -> c_int,
    radio_repeat_f32: extern "C" fn(*const f32, *mut f32, u32, f32, u32) -> c_int,
    radio_destroy: extern "C" fn(*mut Radio),
    radio_render_transmit_f32: extern "C" fn(
        *const Radio,
        *const f32,
        *const f32,
        *const f32,
        u32,
        *const TransmitRenderConfig,
        *mut i16,
        *mut i16,
        *mut u64,
    ) -> c_int,
    radio_ctcss_frequency_supported: extern "C" fn(f32) -> u32,
    radio_ctcss_legacy_frequency: extern "C" fn(f64) -> f64,
    radio_ctcss_legacy_peak: extern "C" fn(f64, u32) -> f64,
    radio_ctcss_legacy_scaled_peak: extern "C" fn(f64, u32, i32, i32) -> f64,
    radio_ctcss_legacy_scaled_levels:
        extern "C" fn(f64, u32, i32, i32, *mut f64, *mut f64) -> c_int,
    radio_ctcss_generate_f32: extern "C" fn(*mut Radio, *mut f32, u32, f64, f32, u32, f64) -> c_int,
    radio_ctcss_generate_tail_f32: extern "C" fn(*mut Radio, *mut f32, u32, f64, f32, u32) -> c_int,
    radio_ctcss_phase_radians: extern "C" fn(*const Radio, *mut f64) -> c_int,
    radio_dcs_code_supported: extern "C" fn(i32) -> u32,
    radio_dcs_configure_transmit: extern "C" fn(*mut Radio, i32, u32) -> c_int,
    radio_dcs_generate_f32: extern "C" fn(*mut Radio, *mut f32, u32, f32, u32, u32) -> c_int,
    radio_dcs_tail_phase_radians: extern "C" fn(*const Radio, *mut f64) -> c_int,
    radio_extract_receive_f32: extern "C" fn(
        *const Radio,
        *const f32,
        *mut f32,
        u32,
        *mut f32,
        u32,
        *mut u32,
        *mut ReceiveExtractStats,
    ) -> c_int,
    radio_render_calibrated_test_tone_f32: extern "C" fn(*mut Radio, *mut f32, u32, u32) -> c_int,
    radio_calibrated_test_tone_phase_radians: extern "C" fn(*const Radio, *mut f64) -> c_int,
    radio_native_parrot_bind_f32: extern "C" fn(*mut Radio, *mut f32, u32) -> c_int,
    radio_native_parrot_reset: extern "C" fn(*mut Radio) -> c_int,
    radio_native_parrot_rx_transition: extern "C" fn(*mut Radio, u32, u32, *mut u32) -> c_int,
    radio_native_parrot_record_f32:
        extern "C" fn(*mut Radio, *const f32, u32, u32, *mut u32) -> c_int,
    radio_native_parrot_play_f32: extern "C" fn(*mut Radio, *mut f32, u32, *mut u32) -> c_int,
    radio_native_parrot_status: extern "C" fn(*const Radio, *mut NativeParrotStatus) -> c_int,
    radio_parse_rx_audio_mode: extern "C" fn(*const c_char, *mut u32) -> c_int,
    radio_parse_carrier_source: extern "C" fn(*const c_char, *mut u32) -> c_int,
    radio_parse_ctcss_source: extern "C" fn(*const c_char, *mut u32) -> c_int,
    radio_parse_tone_off_mode: extern "C" fn(*const c_char, *mut u32) -> c_int,
    radio_dcs_configure_receive: extern "C" fn(*mut Radio, i32, u32) -> c_int,
    radio_dcs_process_receive_f32: extern "C" fn(*mut Radio, *const f32, u32, *mut u32) -> c_int,
    radio_ctcss_configure_receive: extern "C" fn(*mut Radio, *const CtcssReceiveConfig) -> c_int,
    radio_ctcss_process_receive_f32:
        extern "C" fn(*mut Radio, *const f32, u32, u32, *mut i32) -> c_int,
    radio_measure_raw_pcm_f32:
        extern "C" fn(*const f32, u32, u32, *mut AudioStatistics, *mut u32) -> c_int,
    radio_micor_squelch_update:
        extern "C" fn(*mut micor_squelch::State, u32, f64, u32, u32, *mut u32) -> c_int,
    radio_measure_envelope_f32: extern "C" fn(
        *const f32,
        *mut f32,
        u32,
        i32,
        i16,
        *mut envelope_meter::State,
        *mut u32,
    ) -> c_int,
    radio_delay_line_f32: extern "C" fn(
        *const f32,
        *mut f32,
        u32,
        *mut f32,
        u32,
        u32,
        *mut delay_line::State,
        u32,
        u32,
    ) -> c_int,
    radio_center_slicer_f32: extern "C" fn(
        *const f32,
        *mut f32,
        *mut f32,
        u32,
        i32,
        i16,
        i32,
        *mut center_slicer::State,
    ) -> c_int,
    radio_deemphasis_integrator_f32: extern "C" fn(
        *const f32,
        *mut f32,
        u32,
        i16,
        i16,
        i32,
        *mut deemphasis_integrator::State,
    ) -> c_int,
    radio_fir_mono_f32:
        extern "C" fn(*const f32, *mut f32, u32, *mut i16, u32, *const i16, i32, i32, i32) -> c_int,
    radio_receive_frontend_f32: extern "C" fn(
        *const f32,
        *mut f32,
        u32,
        *mut u8,
        u32,
        u32,
        *mut i16,
        u32,
        *const i16,
        i32,
        i32,
        *const i16,
        u32,
        i32,
        u32,
        u32,
        u32,
        u32,
        *mut receive_frontend::State,
        *mut u32,
        *mut u32,
    ) -> c_int,
    radio_elapsed_ms: extern "C" fn(*mut u32, u32, *mut i32) -> c_int,
    radio_timer_consume: extern "C" fn(*mut i32, i32, *mut i32) -> c_int,
    radio_signal_mode_advance: extern "C" fn(
        *const SignalModeConfig,
        *const SignalModeInput,
        *mut SignalModeState,
    ) -> c_int,
    radio_ctcss_render_state_advance: extern "C" fn(
        *const CtcssRenderStateConfig,
        *const CtcssRenderStateInput,
        *mut CtcssRenderState,
    ) -> c_int,
    radio_tx_finish_advance: extern "C" fn(*const TxFinishInput, *mut TxFinishState) -> c_int,
    radio_tx_finish_continue: extern "C" fn(*const TxFinishInput, *mut TxFinishState) -> c_int,
    radio_tx_complete: extern "C" fn(*const TxCompleteConfig, *mut TxCompleteState) -> c_int,
    radio_rx_blanking_advance: extern "C" fn(*const RxBlankingInput, *mut RxBlankingState) -> c_int,
    radio_vox_carrier_advance: extern "C" fn(*const VoxCarrierInput, *mut VoxCarrierState) -> c_int,
    radio_tx_cpu_saver_advance:
        extern "C" fn(*const TxCpuSaverInput, *mut TxCpuSaverState) -> c_int,
    radio_rx_cpu_saver_advance:
        extern "C" fn(*const RxCpuSaverInput, *mut RxCpuSaverState) -> c_int,
    radio_dcs_turnoff_advance: extern "C" fn(
        *const DcsTurnoffConfig,
        *const DcsTurnoffInput,
        *mut DcsTurnoffState,
    ) -> c_int,
}

// SAFETY: the descriptor is immutable, and its function pointers and static
// capability-name address remain valid for the loaded shared object's life.
unsafe impl Sync for RadioDescriptor {}

fn validate_config(config: &RadioConfig) -> Result<(), c_int> {
    if config.struct_size < size_of::<RadioConfig>() as u32 || config.abi_version != ABI_VERSION {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    if config.native_sample_rate_hz == 0 || config.maximum_frame_count == 0 {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    if config.native_sample_rate_hz != timer::NATIVE_SAMPLE_RATE_HZ
        || config.interleaved_channels != CANONICAL_CHANNELS
    {
        return Err(RADIO_UNSUPPORTED);
    }
    Ok(())
}

extern "C" fn radio_create(config: *const RadioConfig, radio: *mut *mut Radio) -> c_int {
    if config.is_null() || radio.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    // A failed create never leaves a stale object pointer in caller storage.
    unsafe { *radio = ptr::null_mut() };
    let config = unsafe { &*config };
    if let Err(result) = validate_config(config) {
        return result;
    }
    let radio_object = Box::new(Radio {
        maximum_frame_count: config.maximum_frame_count,
        interleaved_channels: config.interleaved_channels,
        ctcss_phase_radians: 0.0,
        ctcss_receive: ctcss_receive::ReceiveState::default(),
        dcs_transmit: dcs_transmit::TransmitState::default(),
        dcs_receive: dcs_receive::ReceiveState::default(),
        calibrated_test_tone: calibrated_test_tone::State::default(),
        native_parrot: parrot::State::default(),
    });
    unsafe { *radio = Box::into_raw(radio_object) };
    RADIO_OK
}

extern "C" fn radio_tick(
    radio: *mut Radio,
    input: *const f32,
    output: *mut f32,
    frame_count: u32,
) -> c_int {
    let Some(radio) = NonNull::new(radio) else {
        return RADIO_INVALID_ARGUMENT;
    };
    if output.is_null() || frame_count == 0 {
        return RADIO_INVALID_ARGUMENT;
    }
    let radio = unsafe { radio.as_ref() };
    if frame_count > radio.maximum_frame_count {
        return RADIO_FRAME_COUNT_EXCEEDED;
    }
    // Supported project targets are 64-bit. `u32 * canonical_channels` is
    // therefore representable after configuration fixed the channel layout.
    let sample_count = frame_count as usize * radio.interleaved_channels as usize;
    // A valid output block always receives safe canonical silence, including
    // when the input pointer is invalid.  The caller can therefore recover
    // from a bad input without forwarding stale uninitialized PCM.
    unsafe { ptr::write_bytes(output, 0, sample_count) };
    if input.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    RADIO_OK
}

extern "C" fn radio_repeat_f32(
    input: *const f32,
    output: *mut f32,
    sample_count: u32,
    gain: f32,
    muted: u32,
) -> c_int {
    if muted > 1 || (sample_count != 0 && output.is_null()) {
        return RADIO_INVALID_ARGUMENT;
    }
    if sample_count == 0 {
        return RADIO_OK;
    }
    let sample_count = sample_count as usize;
    if muted != 0 {
        // IEEE-754 positive zero is the canonical silent f32 value.
        unsafe { ptr::write_bytes(output, 0, sample_count) };
        return RADIO_OK;
    }
    if input.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    // The contract permits exact input/output aliasing for in-place gain. A
    // raw forward loop preserves that case without allocating a scratch block.
    for index in 0..sample_count {
        unsafe { output.add(index).write(input.add(index).read() * gain) };
    }
    RADIO_OK
}

extern "C" fn radio_destroy(radio: *mut Radio) {
    if let Some(radio) = NonNull::new(radio) {
        unsafe { drop(Box::from_raw(radio.as_ptr())) };
    }
}

fn parse_transmit_render_config(
    config: *const TransmitRenderConfig,
) -> Result<transmit_renderer::RenderConfig, c_int> {
    let Some(config) = NonNull::new(config.cast_mut()) else {
        return Err(RADIO_INVALID_ARGUMENT);
    };
    let config = unsafe { config.as_ref() };
    if config.struct_size < size_of::<TransmitRenderConfig>() as u32 {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(transmit_renderer::RenderConfig {
        output_a: transmit_renderer::OutputRoute::from_ffi(
            config.output_a_route,
            RADIO_INVALID_ARGUMENT,
        )?,
        output_b: transmit_renderer::OutputRoute::from_ffi(
            config.output_b_route,
            RADIO_INVALID_ARGUMENT,
        )?,
        ctcss_peak_a: config.ctcss_peak_a,
        ctcss_bias_a: config.ctcss_bias_a,
        ctcss_peak_b: config.ctcss_peak_b,
        ctcss_bias_b: config.ctcss_bias_b,
    })
}

extern "C" fn radio_render_transmit_f32(
    radio: *const Radio,
    program: *const f32,
    ctcss: *const f32,
    dcs: *const f32,
    frame_count: u32,
    config: *const TransmitRenderConfig,
    stereo: *mut i16,
    meter_stereo: *mut i16,
    program_rail_samples: *mut u64,
) -> c_int {
    let Some(radio) = NonNull::new(radio.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    if program_rail_samples.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe { *program_rail_samples = 0 };
    let config = match parse_transmit_render_config(config) {
        Ok(config) => config,
        Err(result) => return result,
    };
    let radio = unsafe { radio.as_ref() };
    if frame_count > radio.maximum_frame_count {
        return RADIO_FRAME_COUNT_EXCEEDED;
    }
    if frame_count == 0 {
        return RADIO_OK;
    }
    if program.is_null() || ctcss.is_null() || dcs.is_null() || stereo.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    let rails = unsafe {
        transmit_renderer::render(
            program,
            ctcss,
            dcs,
            frame_count,
            &config,
            stereo,
            meter_stereo,
        )
    };
    unsafe { *program_rail_samples = rails };
    RADIO_OK
}

fn validate_ctcss_radio(
    radio: *mut Radio,
    output: *mut f32,
    frame_count: u32,
) -> Result<NonNull<Radio>, c_int> {
    let Some(radio) = NonNull::new(radio) else {
        return Err(RADIO_INVALID_ARGUMENT);
    };
    let radio_ref = unsafe { radio.as_ref() };
    if frame_count > radio_ref.maximum_frame_count {
        return Err(RADIO_FRAME_COUNT_EXCEEDED);
    }
    if frame_count != 0 && output.is_null() {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(radio)
}

extern "C" fn radio_ctcss_frequency_supported(frequency_hz: f32) -> u32 {
    u32::from(ctcss_transmit::frequency_supported(frequency_hz))
}

extern "C" fn radio_ctcss_legacy_frequency(frequency_hz: f64) -> f64 {
    ctcss_transmit::legacy_frequency(frequency_hz)
}

extern "C" fn radio_ctcss_legacy_peak(frequency_hz: f64, filter_250: u32) -> f64 {
    ctcss_transmit::legacy_peak(frequency_hz, filter_250 != 0)
}

extern "C" fn radio_ctcss_legacy_scaled_peak(
    frequency_hz: f64,
    filter_250: u32,
    tone_gain_q8: i32,
    output_gain_q8: i32,
) -> f64 {
    ctcss_transmit::legacy_scaled_peak(frequency_hz, filter_250 != 0, tone_gain_q8, output_gain_q8)
}

extern "C" fn radio_ctcss_legacy_scaled_levels(
    frequency_hz: f64,
    filter_250: u32,
    tone_gain_q8: i32,
    output_gain_q8: i32,
    amplitude_pcm_codes: *mut f64,
    bias_pcm_codes: *mut f64,
) -> c_int {
    if amplitude_pcm_codes.is_null() || bias_pcm_codes.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    let (amplitude, bias) = ctcss_transmit::legacy_scaled_levels(
        frequency_hz,
        filter_250 != 0,
        tone_gain_q8,
        output_gain_q8,
    );
    unsafe {
        amplitude_pcm_codes.write(amplitude);
        bias_pcm_codes.write(bias);
    }
    RADIO_OK
}

extern "C" fn radio_ctcss_generate_f32(
    radio: *mut Radio,
    output: *mut f32,
    frame_count: u32,
    frequency_hz: f64,
    peak: f32,
    enabled: u32,
    phase_shift_degrees: f64,
) -> c_int {
    let mut radio = match validate_ctcss_radio(radio, output, frame_count) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    let radio = unsafe { radio.as_mut() };
    unsafe {
        ctcss_transmit::generate(
            &mut radio.ctcss_phase_radians,
            output,
            frame_count,
            ctcss_transmit::legacy_frequency(frequency_hz),
            peak,
            enabled,
            phase_shift_degrees,
        );
    }
    RADIO_OK
}

extern "C" fn radio_ctcss_generate_tail_f32(
    radio: *mut Radio,
    output: *mut f32,
    frame_count: u32,
    frequency_hz: f64,
    peak: f32,
    enabled: u32,
) -> c_int {
    let mut radio = match validate_ctcss_radio(radio, output, frame_count) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    let radio = unsafe { radio.as_mut() };
    unsafe {
        ctcss_transmit::generate(
            &mut radio.ctcss_phase_radians,
            output,
            frame_count,
            frequency_hz,
            peak,
            enabled,
            0.0,
        );
    }
    RADIO_OK
}

extern "C" fn radio_ctcss_phase_radians(radio: *const Radio, phase_radians: *mut f64) -> c_int {
    let Some(radio) = NonNull::new(radio.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    if phase_radians.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe { phase_radians.write(radio.as_ref().ctcss_phase_radians) };
    RADIO_OK
}

fn validate_dcs_radio(
    radio: *mut Radio,
    output: *mut f32,
    frame_count: u32,
) -> Result<NonNull<Radio>, c_int> {
    let Some(radio) = NonNull::new(radio) else {
        return Err(RADIO_INVALID_ARGUMENT);
    };
    if frame_count > unsafe { radio.as_ref().maximum_frame_count } {
        return Err(RADIO_FRAME_COUNT_EXCEEDED);
    }
    if frame_count != 0 && output.is_null() {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(radio)
}

extern "C" fn radio_dcs_code_supported(code: i32) -> u32 {
    u32::from(dcs_transmit::code_supported(code))
}

extern "C" fn radio_dcs_configure_transmit(radio: *mut Radio, code: i32, inverted: u32) -> c_int {
    let Some(mut radio) = NonNull::new(radio) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let radio = unsafe { radio.as_mut() };
    // The legacy configuration reset starts a new fractional symbol interval,
    // while preserving the repeated-word and turn-off oscillator phases.
    radio.dcs_transmit.configure(code, inverted);
    RADIO_OK
}

extern "C" fn radio_dcs_generate_f32(
    radio: *mut Radio,
    output: *mut f32,
    frame_count: u32,
    peak: f32,
    enabled: u32,
    turnoff: u32,
) -> c_int {
    let mut radio = match validate_dcs_radio(radio, output, frame_count) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    let radio = unsafe { radio.as_mut() };
    unsafe {
        dcs_transmit::generate(
            &mut radio.dcs_transmit,
            output,
            frame_count,
            dcs_transmit::GenerateConfig {
                peak,
                enabled,
                turnoff,
            },
        );
    }
    RADIO_OK
}

extern "C" fn radio_dcs_tail_phase_radians(radio: *const Radio, phase_radians: *mut f64) -> c_int {
    let Some(radio) = NonNull::new(radio.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    if phase_radians.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe { phase_radians.write(radio.as_ref().dcs_transmit.tail_phase_radians()) };
    RADIO_OK
}

extern "C" fn radio_extract_receive_f32(
    radio: *const Radio,
    stereo: *const f32,
    mono: *mut f32,
    frame_count: u32,
    delay: *mut f32,
    delay_frame_count: u32,
    delay_index: *mut u32,
    stats: *mut ReceiveExtractStats,
) -> c_int {
    const POSITIVE_I16_RAIL: f32 = i16::MAX as f32 / 32_768.0;

    let Some(radio) = NonNull::new(radio.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(stats) = NonNull::new(stats) else {
        return RADIO_INVALID_ARGUMENT;
    };
    // Every call supplies a coherent measurement result, including an invalid
    // PCM buffer.  This prevents control-plane readers from retaining a stale
    // receive peak after a compatibility-adapter recovery path.
    unsafe {
        stats.as_ptr().write(ReceiveExtractStats {
            peak: 0.0,
            rail_samples: 0,
        });
    }
    let radio = unsafe { radio.as_ref() };
    if frame_count > radio.maximum_frame_count {
        return RADIO_FRAME_COUNT_EXCEEDED;
    }
    if frame_count == 0 {
        return RADIO_OK;
    }
    if stereo.is_null() || mono.is_null() || delay_index.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    if delay_frame_count != 0 && delay.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }

    let mut cursor = unsafe { delay_index.read() };
    if delay_frame_count != 0 && cursor >= delay_frame_count {
        cursor = 0;
    }
    let mut peak = 0.0_f32;
    let mut rail_samples = 0_u64;
    for index in 0..frame_count as usize {
        let sample = unsafe { stereo.add(index * CANONICAL_CHANNELS as usize).read() };
        peak = peak.max(sample.abs());
        if sample >= POSITIVE_I16_RAIL || sample <= -1.0 {
            rail_samples += 1;
        }
        let delayed = if delay_frame_count == 0 {
            sample
        } else {
            let delayed = unsafe { delay.add(cursor as usize).read() };
            unsafe { delay.add(cursor as usize).write(sample) };
            cursor += 1;
            if cursor == delay_frame_count {
                cursor = 0;
            }
            delayed
        };
        unsafe { mono.add(index).write(delayed) };
    }
    unsafe {
        delay_index.write(cursor);
        stats
            .as_ptr()
            .write(ReceiveExtractStats { peak, rail_samples });
    }
    RADIO_OK
}

fn validate_calibrated_test_tone_radio(
    radio: *mut Radio,
    program: *mut f32,
    frame_count: u32,
) -> Result<NonNull<Radio>, c_int> {
    let Some(radio) = NonNull::new(radio) else {
        return Err(RADIO_INVALID_ARGUMENT);
    };
    let radio_ref = unsafe { radio.as_ref() };
    if frame_count > radio_ref.maximum_frame_count {
        return Err(RADIO_FRAME_COUNT_EXCEEDED);
    }
    if frame_count != 0 && program.is_null() {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok(radio)
}

extern "C" fn radio_render_calibrated_test_tone_f32(
    radio: *mut Radio,
    program: *mut f32,
    frame_count: u32,
    enabled: u32,
) -> c_int {
    let mut radio = match validate_calibrated_test_tone_radio(radio, program, frame_count) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    let radio = unsafe { radio.as_mut() };
    unsafe {
        radio
            .calibrated_test_tone
            .render(program, frame_count, enabled);
    }
    RADIO_OK
}

extern "C" fn radio_calibrated_test_tone_phase_radians(
    radio: *const Radio,
    phase_radians: *mut f64,
) -> c_int {
    let Some(radio) = NonNull::new(radio.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    if phase_radians.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe {
        phase_radians.write(radio.as_ref().calibrated_test_tone.phase_radians());
    }
    RADIO_OK
}

fn validate_native_parrot_radio(
    radio: *mut Radio,
    frame_count: u32,
) -> Result<NonNull<Radio>, c_int> {
    let Some(radio) = NonNull::new(radio) else {
        return Err(RADIO_INVALID_ARGUMENT);
    };
    if frame_count > unsafe { radio.as_ref() }.maximum_frame_count {
        return Err(RADIO_FRAME_COUNT_EXCEEDED);
    }
    Ok(radio)
}

extern "C" fn radio_native_parrot_bind_f32(
    radio: *mut Radio,
    storage: *mut f32,
    storage_capacity: u32,
) -> c_int {
    let mut radio = match validate_native_parrot_radio(radio, 0) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    let radio = unsafe { radio.as_mut() };
    match radio.native_parrot.bind(storage, storage_capacity) {
        Ok(()) => RADIO_OK,
        Err(result) => result,
    }
}

extern "C" fn radio_native_parrot_reset(radio: *mut Radio) -> c_int {
    let mut radio = match validate_native_parrot_radio(radio, 0) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    unsafe { radio.as_mut() }.native_parrot.reset();
    RADIO_OK
}

extern "C" fn radio_native_parrot_rx_transition(
    radio: *mut Radio,
    was_keyed: u32,
    is_keyed: u32,
    playback_started: *mut u32,
) -> c_int {
    if playback_started.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe { playback_started.write(0) };
    let mut radio = match validate_native_parrot_radio(radio, 0) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    let started = unsafe { radio.as_mut() }
        .native_parrot
        .rx_transition(was_keyed, is_keyed);
    unsafe { playback_started.write(started.into()) };
    RADIO_OK
}

extern "C" fn radio_native_parrot_record_f32(
    radio: *mut Radio,
    input: *const f32,
    frame_count: u32,
    recording_limit: u32,
    recorded: *mut u32,
) -> c_int {
    if recorded.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe { recorded.write(0) };
    let mut radio = match validate_native_parrot_radio(radio, frame_count) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    match unsafe {
        radio
            .as_mut()
            .native_parrot
            .record(input, frame_count, recording_limit)
    } {
        Ok(sample_count) => {
            unsafe { recorded.write(sample_count) };
            RADIO_OK
        }
        Err(result) => result,
    }
}

extern "C" fn radio_native_parrot_play_f32(
    radio: *mut Radio,
    output: *mut f32,
    frame_count: u32,
    played: *mut u32,
) -> c_int {
    if played.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe { played.write(0) };
    let mut radio = match validate_native_parrot_radio(radio, frame_count) {
        Ok(radio) => radio,
        Err(result) => return result,
    };
    match unsafe { radio.as_mut().native_parrot.play(output, frame_count) } {
        Ok(sample_count) => {
            unsafe { played.write(sample_count) };
            RADIO_OK
        }
        Err(result) => result,
    }
}

extern "C" fn radio_native_parrot_status(
    radio: *const Radio,
    status: *mut NativeParrotStatus,
) -> c_int {
    let Some(radio) = NonNull::new(radio.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(status) = NonNull::new(status) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let native_status = unsafe { radio.as_ref() }.native_parrot.status();
    unsafe {
        status.as_ptr().write(NativeParrotStatus {
            recorded_samples: native_status.recorded_samples,
            playback_offset: native_status.playback_offset,
            playing: native_status.playing,
            truncated: native_status.truncated,
        });
    }
    RADIO_OK
}

extern "C" fn radio_dcs_configure_receive(radio: *mut Radio, code: i32, inverted: u32) -> c_int {
    let Some(mut radio) = NonNull::new(radio) else {
        return RADIO_INVALID_ARGUMENT;
    };
    unsafe { radio.as_mut() }
        .dcs_receive
        .configure(code, inverted);
    RADIO_OK
}

extern "C" fn radio_dcs_process_receive_f32(
    radio: *mut Radio,
    stereo: *const f32,
    frame_count: u32,
    valid: *mut u32,
) -> c_int {
    let Some(valid) = NonNull::new(valid) else {
        return RADIO_INVALID_ARGUMENT;
    };
    unsafe { valid.write(0) };
    let Some(mut radio) = NonNull::new(radio) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let radio = unsafe { radio.as_mut() };
    if frame_count > radio.maximum_frame_count {
        return RADIO_FRAME_COUNT_EXCEEDED;
    }
    if !radio.dcs_receive.enabled() {
        return RADIO_OK;
    }
    if frame_count != 0 && stereo.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    let decoded = if frame_count == 0 {
        radio.dcs_receive.valid()
    } else {
        unsafe { radio.dcs_receive.process(stereo, frame_count) }
    };
    unsafe { valid.write(decoded.into()) };
    RADIO_OK
}

extern "C" fn radio_ctcss_configure_receive(
    radio: *mut Radio,
    config: *const CtcssReceiveConfig,
) -> c_int {
    let Some(mut radio) = NonNull::new(radio) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(config) = NonNull::new(config.cast_mut()) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let config = unsafe { config.as_ref() };
    if config.struct_size < size_of::<CtcssReceiveConfig>() as u32 || config.relax > 1 {
        return RADIO_INVALID_ARGUMENT;
    }
    unsafe { radio.as_mut() }
        .ctcss_receive
        .configure(config.tone_mask, config.relax != 0);
    RADIO_OK
}

extern "C" fn radio_ctcss_process_receive_f32(
    radio: *mut Radio,
    samples: *const f32,
    sample_count: u32,
    carrier_detect: u32,
    decoded: *mut i32,
) -> c_int {
    let Some(decoded) = NonNull::new(decoded) else {
        return RADIO_INVALID_ARGUMENT;
    };
    unsafe { decoded.write(-1) };
    let Some(mut radio) = NonNull::new(radio) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let radio = unsafe { radio.as_mut() };
    if carrier_detect > 1 {
        return RADIO_INVALID_ARGUMENT;
    }
    if sample_count > radio.maximum_frame_count {
        return RADIO_FRAME_COUNT_EXCEEDED;
    }
    if !radio.ctcss_receive.enabled() {
        return RADIO_OK;
    }
    if sample_count != 0 && samples.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    let result = if sample_count == 0 {
        radio.ctcss_receive.decoded()
    } else {
        unsafe {
            radio
                .ctcss_receive
                .process(samples, sample_count, carrier_detect != 0)
        }
    };
    unsafe { decoded.write(i32::from(result)) };
    RADIO_OK
}

extern "C" fn radio_measure_raw_pcm_f32(
    samples: *const f32,
    sample_count: u32,
    channels: u32,
    statistics: *mut AudioStatistics,
    clipping: *mut u32,
) -> c_int {
    let Some(mut statistics) = NonNull::new(statistics) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(clipping) = NonNull::new(clipping) else {
        return RADIO_INVALID_ARGUMENT;
    };
    // A valid status destination is always reset before argument validation,
    // matching the other descriptor wrappers' stale-state protection.
    unsafe { clipping.write(0) };
    let statistics = unsafe { statistics.as_mut() };
    match audio_meter::measure(samples, sample_count, channels, statistics) {
        Ok(detected) => {
            unsafe { clipping.write(u32::from(detected)) };
            RADIO_OK
        }
        Err(result) => result,
    }
}

extern "C" fn radio_micor_squelch_update(
    state: *mut micor_squelch::State,
    squelched: u32,
    sample_power: f64,
    open_level: u32,
    hysteresis: u32,
    closed: *mut u32,
) -> c_int {
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(closed) = NonNull::new(closed) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let result = micor_squelch::update(
        unsafe { state.as_mut() },
        squelched != 0,
        sample_power,
        open_level,
        hysteresis,
    );
    unsafe { closed.write(u32::from(result)) };
    RADIO_OK
}

extern "C" fn radio_measure_envelope_f32(
    input: *const f32,
    output: *mut f32,
    sample_count: u32,
    decay_factor: i32,
    threshold: i16,
    state: *mut envelope_meter::State,
    comparator: *mut u32,
) -> c_int {
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(comparator) = NonNull::new(comparator) else {
        return RADIO_INVALID_ARGUMENT;
    };
    // A valid comparator destination never retains a stale VOX or tuning
    // decision when an invalid F32 span is rejected.
    unsafe { comparator.write(0) };
    match unsafe {
        envelope_meter::measure(
            input,
            output,
            sample_count,
            decay_factor,
            threshold,
            state.as_mut(),
        )
    } {
        Ok(result) => {
            unsafe { comparator.write(u32::from(result)) };
            RADIO_OK
        }
        Err(result) => result,
    }
}

extern "C" fn radio_delay_line_f32(
    input: *const f32,
    output: *mut f32,
    sample_count: u32,
    storage: *mut f32,
    storage_capacity: u32,
    lead: u32,
    state: *mut delay_line::State,
    enabled: u32,
    outzero: u32,
) -> c_int {
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };
    if enabled > 1 || outzero > 1 {
        return RADIO_INVALID_ARGUMENT;
    }
    match unsafe {
        delay_line::process(delay_line::Request {
            input,
            output,
            sample_count,
            storage,
            storage_capacity,
            lead,
            state: state.as_mut(),
            enabled: enabled != 0,
            outzero: outzero != 0,
        })
    } {
        Ok(()) => RADIO_OK,
        Err(result) => result,
    }
}

extern "C" fn radio_center_slicer_f32(
    input: *const f32,
    centered_output: *mut f32,
    limited_output: *mut f32,
    sample_count: u32,
    limit: i32,
    setpoint: i16,
    decay_factor: i32,
    state: *mut center_slicer::State,
) -> c_int {
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };
    match unsafe {
        center_slicer::process(center_slicer::Request {
            input,
            centered_output,
            limited_output,
            sample_count,
            limit,
            setpoint,
            decay_factor,
            state: state.as_mut(),
        })
    } {
        Ok(()) => RADIO_OK,
        Err(result) => result,
    }
}

extern "C" fn radio_deemphasis_integrator_f32(
    input: *const f32,
    output: *mut f32,
    sample_count: u32,
    output_coefficient: i16,
    feedback_coefficient: i16,
    output_gain: i32,
    state: *mut deemphasis_integrator::State,
) -> c_int {
    let Some(mut state) = NonNull::new(state) else {
        return RADIO_INVALID_ARGUMENT;
    };
    match unsafe {
        deemphasis_integrator::process(deemphasis_integrator::Request {
            input,
            output,
            sample_count,
            output_coefficient,
            feedback_coefficient,
            output_gain,
            state: state.as_mut(),
        })
    } {
        Ok(()) => RADIO_OK,
        Err(result) => result,
    }
}

extern "C" fn radio_fir_mono_f32(
    input: *const f32,
    output: *mut f32,
    sample_count: u32,
    history: *mut i16,
    history_count: u32,
    coefficients: *const i16,
    input_gain: i32,
    output_gain: i32,
    calc_adjust: i32,
) -> c_int {
    match unsafe {
        fir::process(fir::Request {
            input,
            output,
            sample_count,
            history,
            history_count,
            coefficients,
            input_gain,
            output_gain,
            calc_adjust,
        })
    } {
        Ok(()) => RADIO_OK,
        Err(result) => result,
    }
}

extern "C" fn radio_receive_frontend_f32(
    input: *const f32,
    baseband_output: *mut f32,
    baseband_output_capacity: u32,
    carrier_gate: *mut u8,
    carrier_gate_capacity: u32,
    native_frame_count: u32,
    history: *mut i16,
    history_count: u32,
    baseband_coefficients: *const i16,
    baseband_calc_adjust: i32,
    baseband_output_gain: i32,
    noise_coefficients: *const i16,
    noise_coefficient_count: u32,
    noise_divisor: i32,
    decimate: u32,
    calibration_window: u32,
    open_level: u32,
    hysteresis: u32,
    state: *mut receive_frontend::State,
    baseband_output_count: *mut u32,
    rssi_updated: *mut u32,
) -> c_int {
    let Some(baseband_output_count) = NonNull::new(baseband_output_count) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(rssi_updated) = NonNull::new(rssi_updated) else {
        return RADIO_INVALID_ARGUMENT;
    };
    // Never leave a caller with a stale completion result after a rejected
    // request. The mutable radio state remains untouched on any error.
    unsafe {
        baseband_output_count.write(0);
        rssi_updated.write(0);
    }
    match unsafe {
        receive_frontend::process(receive_frontend::Request {
            input,
            baseband_output,
            baseband_output_capacity,
            carrier_gate,
            carrier_gate_capacity,
            native_frame_count,
            history,
            history_count,
            baseband_coefficients,
            baseband_calc_adjust,
            baseband_output_gain,
            noise_coefficients,
            noise_coefficient_count,
            noise_divisor,
            noise_squelch: true,
            decimate,
            calibration_window,
            open_level,
            hysteresis,
            state,
        })
    } {
        Ok(result) => {
            unsafe {
                baseband_output_count.write(result.baseband_output_count);
                rssi_updated.write(u32::from(result.rssi_updated));
            }
            RADIO_OK
        }
        Err(result) => result,
    }
}

extern "C" fn radio_elapsed_ms(
    remainder: *mut u32,
    native_frame_count: u32,
    milliseconds: *mut i32,
) -> c_int {
    let Some(remainder) = NonNull::new(remainder) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(milliseconds) = NonNull::new(milliseconds) else {
        return RADIO_INVALID_ARGUMENT;
    };

    let elapsed = timer::elapsed_ms(unsafe { &mut *remainder.as_ptr() }, native_frame_count);
    unsafe { milliseconds.write(elapsed) };
    RADIO_OK
}

extern "C" fn radio_timer_consume(
    timer_value: *mut i32,
    milliseconds: i32,
    remaining: *mut i32,
) -> c_int {
    let Some(timer_value) = NonNull::new(timer_value) else {
        return RADIO_INVALID_ARGUMENT;
    };
    let Some(remaining) = NonNull::new(remaining) else {
        return RADIO_INVALID_ARGUMENT;
    };

    let residual = timer::consume(unsafe { &mut *timer_value.as_ptr() }, milliseconds);
    unsafe { remaining.write(residual) };
    RADIO_OK
}

static DESCRIPTOR: RadioDescriptor = RadioDescriptor {
    struct_size: size_of::<RadioDescriptor>() as u32,
    abi_version: ABI_VERSION,
    capability_name: CAPABILITY_NAME.as_ptr().cast::<c_char>(),
    radio_create,
    radio_tick,
    radio_repeat_f32,
    radio_destroy,
    radio_render_transmit_f32,
    radio_ctcss_frequency_supported,
    radio_ctcss_legacy_frequency,
    radio_ctcss_legacy_peak,
    radio_ctcss_legacy_scaled_peak,
    radio_ctcss_legacy_scaled_levels,
    radio_ctcss_generate_f32,
    radio_ctcss_generate_tail_f32,
    radio_ctcss_phase_radians,
    radio_dcs_code_supported,
    radio_dcs_configure_transmit,
    radio_dcs_generate_f32,
    radio_dcs_tail_phase_radians,
    radio_extract_receive_f32,
    radio_render_calibrated_test_tone_f32,
    radio_calibrated_test_tone_phase_radians,
    radio_native_parrot_bind_f32,
    radio_native_parrot_reset,
    radio_native_parrot_rx_transition,
    radio_native_parrot_record_f32,
    radio_native_parrot_play_f32,
    radio_native_parrot_status,
    radio_parse_rx_audio_mode: config_parse::radio_parse_rx_audio_mode,
    radio_parse_carrier_source: config_parse::radio_parse_carrier_source,
    radio_parse_ctcss_source: config_parse::radio_parse_ctcss_source,
    radio_parse_tone_off_mode: config_parse::radio_parse_tone_off_mode,
    radio_dcs_configure_receive,
    radio_dcs_process_receive_f32,
    radio_ctcss_configure_receive,
    radio_ctcss_process_receive_f32,
    radio_measure_raw_pcm_f32,
    radio_micor_squelch_update,
    radio_measure_envelope_f32,
    radio_delay_line_f32,
    radio_center_slicer_f32,
    radio_deemphasis_integrator_f32,
    radio_fir_mono_f32,
    radio_receive_frontend_f32,
    radio_elapsed_ms,
    radio_timer_consume,
    radio_signal_mode_advance: signal_mode::radio_signal_mode_advance,
    radio_ctcss_render_state_advance: ctcss_render_state::radio_ctcss_render_state_advance,
    radio_tx_finish_advance: tx_finish::radio_tx_finish_advance,
    radio_tx_finish_continue: tx_finish::radio_tx_finish_continue,
    radio_tx_complete: tx_complete::radio_tx_complete,
    radio_rx_blanking_advance: rx_blanking::radio_rx_blanking_advance,
    radio_vox_carrier_advance: vox_carrier::radio_vox_carrier_advance,
    radio_tx_cpu_saver_advance: tx_cpu_saver::radio_tx_cpu_saver_advance,
    radio_rx_cpu_saver_advance: rx_cpu_saver::radio_rx_cpu_saver_advance,
    radio_dcs_turnoff_advance: dcs_turnoff::radio_dcs_turnoff_advance,
};

/// Return the immutable function table for native radio-core ABI version one.
#[unsafe(no_mangle)]
pub extern "C" fn rptadv_radio_descriptor() -> *const RadioDescriptor {
    &DESCRIPTOR
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests;
