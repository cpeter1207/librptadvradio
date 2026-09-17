//! ABI 4 whole-session radio engine.
//!
//! One prepared session owns independent receive and transmit workers.  The
//! workers may run concurrently, but each has exactly one serial callback
//! owner.  Their only shared mutable state is exchanged through atomics.

use std::cell::UnsafeCell;
use std::ffi::{c_int, c_void};
use std::mem::size_of;
use std::ptr;
use std::slice;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, Ordering};

use crate::{
    CtcssRenderStateConfig, DcsTurnoffConfig, SignalModeConfig, TxCompleteConfig,
    calibrated_test_tone, ctcss_transmit, dcs_transmit, receive_path, receive_profiles,
    receive_qualification, rx_cpu_saver,
    transmit_signaling::{self, TransmitSignaling},
};

pub(crate) const NATIVE_SAMPLE_RATE_HZ: u32 = 48_000;
pub(crate) const CANONICAL_CHANNELS: u32 = 2;
pub(crate) const ABI_VERSION: u32 = 4;
pub(crate) const OK: c_int = 0;
pub(crate) const INVALID_ARGUMENT: c_int = -1;
pub(crate) const PROVIDER_FAILED: c_int = -2;
pub(crate) const FRAME_COUNT_EXCEEDED: c_int = -3;
pub(crate) const UNSUPPORTED: c_int = -4;
pub(crate) const NOT_READY: c_int = -5;
pub(crate) const BUSY: c_int = -6;

const EVENT_CAPACITY: usize = 64;
const MAX_SIGNED_PCM_LEVEL: u32 = i16::MAX as u32;
const VALID_CTCSS_TONE_MASK: u64 = (1_u64 << crate::ctcss_receive::TONE_COUNT) - 1;

/// External processing function prepared by an adapter.
pub(crate) type ProcessFn = unsafe extern "C" fn(
    context: *mut c_void,
    input: *const f32,
    output: *mut f32,
    frame_count: u32,
) -> c_int;

/// Setup-time warm-up function for one external processing object.
pub(crate) type WarmFn = unsafe extern "C" fn(context: *mut c_void, frame_count: u32) -> c_int;

/// Advance a bypassed prepared processor without producing output.
pub(crate) type BypassFn = unsafe extern "C" fn(context: *mut c_void, frame_count: u32) -> c_int;

/// Exact-frame program-ring render function.
pub(crate) type RingRenderFn = unsafe extern "C" fn(
    context: *mut c_void,
    output: *mut f32,
    frame_count: u32,
    result: *mut ProgramRingResult,
) -> c_int;

/// Prepared, borrowed mono processor port.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ProcessorPort {
    pub(crate) context: *mut c_void,
    pub(crate) process_f32: Option<ProcessFn>,
    pub(crate) bypass: Option<BypassFn>,
    pub(crate) warm: Option<WarmFn>,
}

impl Default for ProcessorPort {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            process_f32: None,
            bypass: None,
            warm: None,
        }
    }
}

/// Prepared, borrowed rate-adjusting program-ring consumer port.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ProgramRingPort {
    pub(crate) context: *mut c_void,
    pub(crate) render_f32: Option<RingRenderFn>,
    pub(crate) warm: Option<WarmFn>,
}

impl Default for ProgramRingPort {
    fn default() -> Self {
        Self {
            context: ptr::null_mut(),
            render_f32: None,
            warm: None,
        }
    }
}

/// One program-ring observation returned by its provider.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct RingObservation {
    pub(crate) occupancy_frames: u32,
    pub(crate) reserve_frames: u32,
    pub(crate) target_frames: u32,
    pub(crate) capacity_frames: u32,
    pub(crate) ratio: f64,
    pub(crate) underrun_samples: u64,
    pub(crate) overrun_samples: u64,
    pub(crate) concealment_samples: u64,
}

/// Sample-associated local-receiver state returned with rendered program PCM.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct ProgramRingResult {
    pub(crate) observation: RingObservation,
    pub(crate) receiver_keyed: u32,
    pub(crate) ctcss_decoded_index: i32,
    pub(crate) dcs_valid: u32,
}

impl Default for ProgramRingResult {
    fn default() -> Self {
        Self {
            observation: RingObservation::default(),
            receiver_keyed: 0,
            ctcss_decoded_index: -1,
            dcs_valid: 0,
        }
    }
}

/// Immutable native receive-detector configuration.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ReceiveConfig {
    pub(crate) noise_filter_profile: u32,
    pub(crate) squelch_open_level: u32,
    pub(crate) squelch_hysteresis: u32,
    pub(crate) ctcss_decoder_gain: f32,
    pub(crate) vox_threshold: i32,
    pub(crate) vox_hang_ms: i32,
    pub(crate) ctcss_enabled: u32,
    pub(crate) ctcss_tone_mask: u64,
    pub(crate) ctcss_relax: u32,
    pub(crate) dcs_enabled: u32,
    pub(crate) dcs_code: i32,
    pub(crate) dcs_inverted: u32,
    pub(crate) cpu_saver_enabled: u32,
    pub(crate) native_squelch_delay_frames: u32,
}

/// Immutable receiver source and qualification policy.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct QualificationConfig {
    pub(crate) carrier_source: u32,
    pub(crate) subaudible_source: u32,
    pub(crate) subaudible_override: u32,
    pub(crate) advanced_transport: u32,
    pub(crate) radio_duplex: u32,
    pub(crate) rx_on_delay_blocks: u32,
    pub(crate) tx_off_delay_blocks: u32,
}

/// Immutable transmitter signaling and renderer configuration.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct TransmitConfig {
    pub(crate) ctcss_transmit_enabled: u32,
    pub(crate) default_ctcss_frequency_tenths_hz: i32,
    pub(crate) mapped_ctcss_frequency_tenths_hz: [i32; crate::ctcss_receive::TONE_COUNT],
    pub(crate) tone_off_mode: u32,
    pub(crate) ctcss_turnoff_duration_ms: i32,
    pub(crate) ctcss_turnoff_phase_shift_degrees: f64,
    pub(crate) ctcss_turnoff_tail_tone_hz: f64,
    pub(crate) dcs_transmit_enabled: u32,
    pub(crate) dcs_turnoff_enabled: u32,
    pub(crate) dcs_turnoff_duration_ms: i32,
    pub(crate) receiver_blanking_ms: i32,
    pub(crate) tx_settle_time_ms: i32,
    pub(crate) cpu_saver_enabled: u32,
    pub(crate) dcs_code: i32,
    pub(crate) dcs_inverted: u32,
    pub(crate) dcs_peak: f32,
    pub(crate) ctcss_peak: f32,
    pub(crate) output_a_route: u32,
    pub(crate) output_b_route: u32,
    pub(crate) output_a_tone_gain: f32,
    pub(crate) output_a_tone_bias: f32,
    pub(crate) output_b_tone_gain: f32,
    pub(crate) output_b_tone_bias: f32,
}

impl Default for TransmitConfig {
    fn default() -> Self {
        Self {
            ctcss_transmit_enabled: 0,
            default_ctcss_frequency_tenths_hz: 0,
            mapped_ctcss_frequency_tenths_hz: [0; crate::ctcss_receive::TONE_COUNT],
            tone_off_mode: 0,
            ctcss_turnoff_duration_ms: 0,
            ctcss_turnoff_phase_shift_degrees: 0.0,
            ctcss_turnoff_tail_tone_hz: 0.0,
            dcs_transmit_enabled: 0,
            dcs_turnoff_enabled: 0,
            dcs_turnoff_duration_ms: 0,
            receiver_blanking_ms: 0,
            tx_settle_time_ms: 0,
            cpu_saver_enabled: 0,
            dcs_code: 0,
            dcs_inverted: 0,
            dcs_peak: 0.0,
            ctcss_peak: 0.0,
            output_a_route: 0,
            output_b_route: 0,
            output_a_tone_gain: 0.0,
            output_a_tone_bias: 0.0,
            output_b_tone_gain: 0.0,
            output_b_tone_bias: 0.0,
        }
    }
}

/// Immutable setup for one complete runtime generation.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct SessionConfig {
    pub(crate) struct_size: u32,
    pub(crate) abi_version: u32,
    pub(crate) generation_id: u64,
    pub(crate) native_sample_rate_hz: u32,
    pub(crate) interleaved_channels: u32,
    pub(crate) maximum_receive_frame_count: u32,
    pub(crate) maximum_transmit_frame_count: u32,
    pub(crate) publication_interval_ms: u32,
    pub(crate) receive_channel: u32,
    pub(crate) receive_input_gain: f32,
    pub(crate) receive: ReceiveConfig,
    pub(crate) qualification: QualificationConfig,
    pub(crate) transmit: TransmitConfig,
}

/// Borrowed prepared processing and ring objects for one session.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct SessionPorts {
    pub(crate) struct_size: u32,
    pub(crate) receive_deemphasis: ProcessorPort,
    pub(crate) receive_filter: ProcessorPort,
    pub(crate) receive_ctcss_notch: [ProcessorPort; crate::ctcss_receive::TONE_COUNT],
    pub(crate) receive_noise_reduction: ProcessorPort,
    pub(crate) receive_dynamics: ProcessorPort,
    pub(crate) transmit_program: ProcessorPort,
    pub(crate) transmit_dcs_normal_filter: ProcessorPort,
    pub(crate) transmit_dcs_turnoff_filter: ProcessorPort,
    pub(crate) program_ring: ProgramRingPort,
    /// Prepared 55 Hz CTCSS tail notch, selected until carrier loss.
    pub(crate) receive_ctcss_tail_notch: ProcessorPort,
}

impl Default for SessionPorts {
    fn default() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            receive_deemphasis: ProcessorPort::default(),
            receive_filter: ProcessorPort::default(),
            receive_ctcss_notch: [ProcessorPort::default(); crate::ctcss_receive::TONE_COUNT],
            receive_noise_reduction: ProcessorPort::default(),
            receive_dynamics: ProcessorPort::default(),
            transmit_program: ProcessorPort::default(),
            transmit_dcs_normal_filter: ProcessorPort::default(),
            transmit_dcs_turnoff_filter: ProcessorPort::default(),
            program_ring: ProgramRingPort::default(),
            receive_ctcss_tail_notch: ProcessorPort::default(),
        }
    }
}

/// Hardware/control snapshots consumed by the receive worker.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct ReceiveInput {
    pub(crate) hardware_carrier: u32,
    pub(crate) parallel_carrier: u32,
    pub(crate) hardware_subaudible: u32,
    pub(crate) parallel_subaudible: u32,
    pub(crate) subaudible_override: u32,
}

/// Hardware/control snapshots consumed by the transmit worker.
#[repr(C)]
#[derive(Clone, Copy, Default)]
pub(crate) struct TransmitInput {
    pub(crate) external_ptt_request: u32,
    pub(crate) physical_ptt_applied: u32,
    pub(crate) render_admitted: u32,
    pub(crate) ctcss_inhibit: u32,
    pub(crate) calibrated_test_tone: u32,
    pub(crate) forced_ctcss_tenths_hz: i32,
}

/// Immediate result of one receive callback.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct ReceiveResult {
    pub(crate) generation_id: u64,
    pub(crate) first_sample_index: u64,
    pub(crate) frame_count: u32,
    pub(crate) carrier_active: u32,
    pub(crate) subaudible_active: u32,
    pub(crate) receiver_keyed: u32,
    pub(crate) ctcss_decoded_index: i32,
    pub(crate) dcs_valid: u32,
    pub(crate) rssi_peak: i16,
    pub(crate) rssi_updated: u32,
    pub(crate) ctcss_decoder_peak: f32,
    pub(crate) input_peak: f32,
    pub(crate) input_rms: f32,
    pub(crate) input_rail_samples: u64,
    pub(crate) output_peak: f32,
    pub(crate) output_rms: f32,
    pub(crate) output_rail_samples: u64,
    pub(crate) periodic_status_due: u32,
}

/// Immediate result of one transmit callback.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct TransmitResult {
    pub(crate) generation_id: u64,
    pub(crate) first_sample_index: u64,
    pub(crate) frame_count: u32,
    pub(crate) logical_ptt: u32,
    pub(crate) transmitter_state: i32,
    pub(crate) selected_ctcss_tenths_hz: i32,
    pub(crate) program_peak: f32,
    pub(crate) program_rms: f32,
    pub(crate) program_rail_samples: u64,
    pub(crate) output_peak: f32,
    pub(crate) output_rms: f32,
    pub(crate) output_rail_samples: u64,
    pub(crate) periodic_status_due: u32,
    pub(crate) program_ring: RingObservation,
}

/// Event class emitted by one serial callback owner.
#[repr(u32)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum EventKind {
    #[default]
    None = 0,
    Carrier = 1,
    Subaudible = 2,
    ReceiverKeyed = 3,
    CtcssDecode = 4,
    DcsDecode = 5,
    Ptt = 6,
    CtcssTransmit = 7,
    ReceiverBlanking = 8,
    ProviderFailure = 9,
}

/// One generation-tagged observable edge.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct Event {
    pub(crate) generation_id: u64,
    pub(crate) sample_index: u64,
    pub(crate) kind: u32,
    pub(crate) value: i32,
}

/// Lock-free aggregate snapshot suitable for meters and diagnostics.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Snapshot {
    pub(crate) generation_id: u64,
    pub(crate) receive_frames: u64,
    pub(crate) transmit_frames: u64,
    pub(crate) receive_input_peak: f32,
    pub(crate) receive_input_rms: f32,
    pub(crate) receive_ctcss_decoder_peak: f32,
    pub(crate) receive_output_peak: f32,
    pub(crate) receive_output_rms: f32,
    pub(crate) transmit_program_peak: f32,
    pub(crate) transmit_program_rms: f32,
    pub(crate) transmit_output_peak: f32,
    pub(crate) transmit_output_rms: f32,
    pub(crate) receive_input_rail_samples: u64,
    pub(crate) receive_output_rail_samples: u64,
    pub(crate) transmit_program_rail_samples: u64,
    pub(crate) transmit_output_rail_samples: u64,
    pub(crate) provider_failures: u64,
    pub(crate) receive_event_drops: u64,
    pub(crate) transmit_event_drops: u64,
    pub(crate) carrier_active: u32,
    pub(crate) subaudible_active: u32,
    pub(crate) receiver_keyed: u32,
    pub(crate) logical_ptt: u32,
    pub(crate) ctcss_decoded_index: i32,
    pub(crate) dcs_valid: u32,
    pub(crate) program_ring: RingObservation,
}

#[derive(Clone, Copy)]
struct Ports {
    receive_deemphasis: ProcessorPort,
    receive_filter: ProcessorPort,
    receive_ctcss_notch: [ProcessorPort; crate::ctcss_receive::TONE_COUNT],
    receive_noise_reduction: ProcessorPort,
    receive_dynamics: ProcessorPort,
    transmit_program: ProcessorPort,
    transmit_dcs_normal_filter: ProcessorPort,
    transmit_dcs_turnoff_filter: ProcessorPort,
    program_ring: ProgramRingPort,
    receive_ctcss_tail_notch: ProcessorPort,
}

struct ReceiveWorker {
    detector: receive_path::ReceivePath,
    qualification: receive_qualification::Config,
    qualification_state: receive_qualification::State,
    cpu_saver_enabled: bool,
    cpu_saver_state: crate::RxCpuSaverState,
    per_sample_noise_gate: bool,
    ctcss_enabled: bool,
    dcs_enabled: bool,
    receive_channel: usize,
    input_gain: f32,
    maximum_frames: usize,
    publication_interval_frames: u64,
    next_publication_frame: u64,
    sample_index: u64,
    previous_carrier: bool,
    previous_subaudible: bool,
    previous_keyed: bool,
    previous_ctcss: i32,
    previous_dcs: bool,
    input_mono: Vec<f32>,
    delay: Vec<f32>,
    delay_index: usize,
    work_a: Vec<f32>,
    work_b: Vec<f32>,
    work_c: Vec<f32>,
}

struct TransmitWorker {
    signaling: TransmitSignaling,
    maximum_frames: usize,
    publication_interval_frames: u64,
    next_publication_frame: u64,
    sample_index: u64,
    ctcss_peak: f32,
    dcs_peak: f32,
    dcs_enabled: bool,
    output_a_route: OutputRoute,
    output_b_route: OutputRoute,
    output_a_tone_gain: f32,
    output_a_tone_bias: f32,
    output_b_tone_gain: f32,
    output_b_tone_bias: f32,
    ctcss_phase: f64,
    dcs: dcs_transmit::TransmitState,
    test_tone: calibrated_test_tone::State,
    program: Vec<f32>,
    processed: Vec<f32>,
    ctcss: Vec<f32>,
    dcs_normal_raw: Vec<f32>,
    dcs_normal_filtered: Vec<f32>,
    dcs_turnoff_raw: Vec<f32>,
    dcs_turnoff_filtered: Vec<f32>,
    active: Vec<u8>,
}

#[derive(Clone, Copy)]
enum OutputRoute {
    Disabled,
    Voice,
    Tone,
    Composite,
    AuxiliaryVoice,
}

impl OutputRoute {
    fn parse(value: u32) -> Result<Self, c_int> {
        match value {
            0 => Ok(Self::Disabled),
            1 => Ok(Self::Voice),
            2 => Ok(Self::Tone),
            3 => Ok(Self::Composite),
            4 => Ok(Self::AuxiliaryVoice),
            _ => Err(INVALID_ARGUMENT),
        }
    }

    fn voice(self) -> bool {
        matches!(self, Self::Voice | Self::Composite | Self::AuxiliaryVoice)
    }

    fn tone(self) -> bool {
        matches!(self, Self::Tone | Self::Composite)
    }
}

struct SharedState {
    logical_ptt: AtomicBool,
    receiver_blanking_ms: AtomicI32,
}

struct AtomicSnapshot {
    receive_frames: AtomicU64,
    transmit_frames: AtomicU64,
    receive_input_peak: AtomicU32,
    receive_input_rms: AtomicU32,
    receive_ctcss_decoder_peak: AtomicU32,
    receive_output_peak: AtomicU32,
    receive_output_rms: AtomicU32,
    transmit_program_peak: AtomicU32,
    transmit_program_rms: AtomicU32,
    transmit_output_peak: AtomicU32,
    transmit_output_rms: AtomicU32,
    receive_input_rails: AtomicU64,
    receive_output_rails: AtomicU64,
    transmit_program_rails: AtomicU64,
    transmit_output_rails: AtomicU64,
    provider_failures: AtomicU64,
    carrier: AtomicBool,
    subaudible: AtomicBool,
    receiver_keyed: AtomicBool,
    logical_ptt: AtomicBool,
    decoded_ctcss: AtomicI32,
    dcs_valid: AtomicBool,
    ring_occupancy: AtomicU32,
    ring_reserve: AtomicU32,
    ring_target: AtomicU32,
    ring_capacity: AtomicU32,
    ring_ratio: AtomicU64,
    ring_underruns: AtomicU64,
    ring_overruns: AtomicU64,
    ring_concealment: AtomicU64,
}

impl Default for AtomicSnapshot {
    fn default() -> Self {
        Self {
            receive_frames: AtomicU64::new(0),
            transmit_frames: AtomicU64::new(0),
            receive_input_peak: AtomicU32::new(0),
            receive_input_rms: AtomicU32::new(0),
            receive_ctcss_decoder_peak: AtomicU32::new(0),
            receive_output_peak: AtomicU32::new(0),
            receive_output_rms: AtomicU32::new(0),
            transmit_program_peak: AtomicU32::new(0),
            transmit_program_rms: AtomicU32::new(0),
            transmit_output_peak: AtomicU32::new(0),
            transmit_output_rms: AtomicU32::new(0),
            receive_input_rails: AtomicU64::new(0),
            receive_output_rails: AtomicU64::new(0),
            transmit_program_rails: AtomicU64::new(0),
            transmit_output_rails: AtomicU64::new(0),
            provider_failures: AtomicU64::new(0),
            carrier: AtomicBool::new(false),
            subaudible: AtomicBool::new(false),
            receiver_keyed: AtomicBool::new(false),
            logical_ptt: AtomicBool::new(false),
            decoded_ctcss: AtomicI32::new(-1),
            dcs_valid: AtomicBool::new(false),
            ring_occupancy: AtomicU32::new(0),
            ring_reserve: AtomicU32::new(0),
            ring_target: AtomicU32::new(0),
            ring_capacity: AtomicU32::new(0),
            ring_ratio: AtomicU64::new(1.0_f64.to_bits()),
            ring_underruns: AtomicU64::new(0),
            ring_overruns: AtomicU64::new(0),
            ring_concealment: AtomicU64::new(0),
        }
    }
}

struct EventQueue {
    storage: Box<[UnsafeCell<Event>; EVENT_CAPACITY]>,
    write: AtomicU64,
    read: AtomicU64,
    drops: AtomicU64,
}

// SAFETY: the single producer writes only the unpublished write slot; the
// single consumer reads only the published read slot.  Release/acquire index
// publication transfers ownership of each slot without a lock.
unsafe impl Sync for EventQueue {}

impl EventQueue {
    fn new() -> Self {
        Self {
            storage: Box::new(std::array::from_fn(|_| UnsafeCell::new(Event::default()))),
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            drops: AtomicU64::new(0),
        }
    }

    fn push(&self, event: Event) {
        let write = self.write.load(Ordering::Relaxed);
        let read = self.read.load(Ordering::Acquire);
        if write.wrapping_sub(read) >= EVENT_CAPACITY as u64 {
            self.drops.fetch_add(1, Ordering::Relaxed);
            return;
        }
        let slot = write as usize % EVENT_CAPACITY;
        // SAFETY: only this queue's producer owns the unpublished write slot.
        unsafe { *self.storage[slot].get() = event };
        self.write.store(write.wrapping_add(1), Ordering::Release);
    }

    fn pop(&self, output: &mut Event) -> bool {
        let read = self.read.load(Ordering::Relaxed);
        let write = self.write.load(Ordering::Acquire);
        if read == write {
            return false;
        }
        let slot = read as usize % EVENT_CAPACITY;
        // SAFETY: acquire observed publication and only the consumer owns this slot.
        *output = unsafe { *self.storage[slot].get() };
        self.read.store(read.wrapping_add(1), Ordering::Release);
        true
    }
}

/// Opaque whole-engine runtime generation.
#[repr(C)]
pub(crate) struct Session {
    generation_id: u64,
    ports: Ports,
    receive: UnsafeCell<ReceiveWorker>,
    transmit: UnsafeCell<TransmitWorker>,
    receive_busy: AtomicBool,
    transmit_busy: AtomicBool,
    warmed: AtomicBool,
    shared: SharedState,
    snapshot: AtomicSnapshot,
    receive_events: EventQueue,
    transmit_events: EventQueue,
}

// SAFETY: receive and transmit mutable state have independent single-owner
// guards.  Cross-owner state is atomic, and borrowed providers have the same
// ownership split documented by SessionPorts.
unsafe impl Sync for Session {}

struct BusyGuard<'a>(&'a AtomicBool);

impl Drop for BusyGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

fn enter(flag: &AtomicBool) -> Result<BusyGuard<'_>, c_int> {
    flag.compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .map(|_| BusyGuard(flag))
        .map_err(|_| BUSY)
}

fn bool32(value: u32) -> Result<bool, c_int> {
    match value {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(INVALID_ARGUMENT),
    }
}

fn q8_gain(value: f32) -> Result<i32, c_int> {
    if !value.is_finite() || value < 0.0 || value > i32::MAX as f32 / 256.0 {
        return Err(INVALID_ARGUMENT);
    }
    Ok((value * 256.0).round() as i32)
}

fn carrier_source(value: u32) -> Result<receive_qualification::CarrierSource, c_int> {
    use receive_qualification::CarrierSource;
    match value {
        0 => Ok(CarrierSource::Ignore),
        1 => Ok(CarrierSource::DspNoise),
        2 => Ok(CarrierSource::DspVox),
        3 => Ok(CarrierSource::Hardware),
        4 => Ok(CarrierSource::HardwareInverted),
        5 => Ok(CarrierSource::Parallel),
        6 => Ok(CarrierSource::ParallelInverted),
        _ => Err(INVALID_ARGUMENT),
    }
}

fn subaudible_source(value: u32) -> Result<receive_qualification::SubaudibleSource, c_int> {
    use receive_qualification::SubaudibleSource;
    match value {
        0 => Ok(SubaudibleSource::Ignore),
        1 => Ok(SubaudibleSource::Hardware),
        2 => Ok(SubaudibleSource::HardwareInverted),
        3 => Ok(SubaudibleSource::Dsp),
        4 => Ok(SubaudibleSource::Parallel),
        5 => Ok(SubaudibleSource::ParallelInverted),
        _ => Err(INVALID_ARGUMENT),
    }
}

fn tone_off_mode(value: u32) -> Result<transmit_signaling::ToneOffMode, c_int> {
    use transmit_signaling::ToneOffMode;
    match value {
        0 => Ok(ToneOffMode::None),
        1 => Ok(ToneOffMode::PhaseShift),
        2 => Ok(ToneOffMode::ToneRemove),
        3 => Ok(ToneOffMode::TailTone),
        _ => Err(INVALID_ARGUMENT),
    }
}

fn receive_config(
    config: &SessionConfig,
) -> Result<
    (
        receive_path::Config<'static>,
        receive_qualification::Config,
        bool,
        bool,
        bool,
    ),
    c_int,
> {
    let rx = config.receive;
    let noise = receive_profiles::noise(rx.noise_filter_profile).ok_or(INVALID_ARGUMENT)?;
    let ctcss = receive_profiles::ctcss(rx.ctcss_tone_mask);
    let ctcss_enabled = bool32(rx.ctcss_enabled)?;
    let dcs_enabled = bool32(rx.dcs_enabled)?;
    let cpu_saver_enabled = bool32(rx.cpu_saver_enabled)?;
    let ctcss_decoder_gain = q8_gain(rx.ctcss_decoder_gain)?;
    let vox_threshold = i16::try_from(rx.vox_threshold).map_err(|_| INVALID_ARGUMENT)?;
    let selected_carrier = carrier_source(config.qualification.carrier_source)?;
    if rx.squelch_open_level > MAX_SIGNED_PCM_LEVEL
        || rx.squelch_hysteresis > MAX_SIGNED_PCM_LEVEL
        || rx.vox_threshold < 0
        || !(0..=i32::from(i16::MAX)).contains(&rx.vox_hang_ms)
        || rx.ctcss_tone_mask & !VALID_CTCSS_TONE_MASK != 0
        || (ctcss_enabled && dcs_enabled)
        || (dcs_enabled && !crate::dcs_receive::code_supported(rx.dcs_code))
    {
        return Err(INVALID_ARGUMENT);
    }
    let qualification = receive_qualification::Config {
        carrier_source: selected_carrier,
        subaudible_source: subaudible_source(config.qualification.subaudible_source)?,
        subaudible_override: bool32(config.qualification.subaudible_override)?,
        advanced_transport: bool32(config.qualification.advanced_transport)?,
        radio_duplex: bool32(config.qualification.radio_duplex)?,
        rx_on_delay_frames: config.qualification.rx_on_delay_blocks,
        tx_off_delay_frames: config.qualification.tx_off_delay_blocks,
    };
    Ok((
        receive_path::Config {
            native_sample_rate_hz: config.native_sample_rate_hz,
            maximum_native_frames: config.maximum_receive_frame_count,
            frontend: receive_path::FrontendConfig {
                baseband_coefficients: receive_profiles::BASEBAND.coefficients,
                baseband_calc_adjust: receive_profiles::BASEBAND.divisor,
                baseband_output_gain: 256,
                noise_coefficients: noise.coefficients,
                noise_divisor: noise.divisor,
                decimate: 6,
                calibration_window: 960,
                open_level: rx.squelch_open_level,
                hysteresis: rx.squelch_hysteresis,
                noise_squelch: selected_carrier == receive_qualification::CarrierSource::DspNoise,
            },
            lsd_filter: receive_path::FilterConfig {
                coefficients: ctcss.coefficients,
                input_gain: 256,
                output_gain: ctcss_decoder_gain,
                calc_adjust: ctcss.divisor,
            },
            hpf_filter: None,
            center_slicer: ctcss_enabled.then_some(receive_path::CenterSlicerConfig {
                limit: 625,
                setpoint: 4_900,
                decay_factor: 5,
                trace: false,
            }),
            deemphasis: None,
            delay: None,
            vox: (selected_carrier == receive_qualification::CarrierSource::DspVox).then_some(
                receive_path::VoxConfig {
                    envelope: receive_path::EnvelopeConfig {
                        decay_factor: 3,
                        threshold: vox_threshold,
                    },
                    hang_time_ms: rx.vox_hang_ms,
                },
            ),
            measurement: None,
            ctcss: ctcss_enabled.then_some(receive_path::CtcssConfig {
                tone_mask: rx.ctcss_tone_mask,
                relax: bool32(rx.ctcss_relax)?,
            }),
            dcs: bool32(rx.dcs_enabled)?.then_some(receive_path::DcsConfig {
                code: rx.dcs_code,
                inverted: bool32(rx.dcs_inverted)?,
            }),
        },
        qualification,
        cpu_saver_enabled,
        ctcss_enabled,
        dcs_enabled,
    ))
}

fn transmit_config(
    config: TransmitConfig,
) -> Result<(transmit_signaling::Config, bool, OutputRoute, OutputRoute), c_int> {
    let ctcss_enabled = bool32(config.ctcss_transmit_enabled)?;
    let dcs_enabled = bool32(config.dcs_transmit_enabled)?;
    let _ = bool32(config.dcs_inverted)?;
    if dcs_enabled && (ctcss_enabled || !dcs_transmit::code_supported(config.dcs_code))
        || !config.ctcss_peak.is_finite()
        || !(0.0..=1.0).contains(&config.ctcss_peak)
        || !config.dcs_peak.is_finite()
        || !(0.0..=1.0).contains(&config.dcs_peak)
    {
        return Err(INVALID_ARGUMENT);
    }
    let signaling = transmit_signaling::Config {
        signal_mode: SignalModeConfig {
            hold_ms: 2_500,
            ctcss_tx_enabled: config.ctcss_transmit_enabled,
            default_tx_ctcss_frequency_tenths_hz: config.default_ctcss_frequency_tenths_hz,
            mapped_tx_ctcss_frequency_tenths_hz: config.mapped_ctcss_frequency_tenths_hz,
        },
        dcs_turnoff: DcsTurnoffConfig {
            turnoff_duration_ms: config.dcs_turnoff_duration_ms,
        },
        ctcss_render: CtcssRenderStateConfig {
            turnoff_duration_ms: config.ctcss_turnoff_duration_ms,
            turnoff_phase_shift_degrees: config.ctcss_turnoff_phase_shift_degrees,
            turnoff_tail_tone_hz: config.ctcss_turnoff_tail_tone_hz,
        },
        tx_complete: TxCompleteConfig {
            txrx_blanking_time_ms: config.receiver_blanking_ms,
        },
        tone_off_mode: tone_off_mode(config.tone_off_mode)?,
        dcs_transmit_enabled: dcs_enabled,
        dcs_turnoff_enabled: bool32(config.dcs_turnoff_enabled)?,
        tx_settle_time_ms: config.tx_settle_time_ms,
        tx_cpu_saver_enabled: bool32(config.cpu_saver_enabled)?,
    };
    Ok((
        signaling,
        dcs_enabled,
        OutputRoute::parse(config.output_a_route)?,
        OutputRoute::parse(config.output_b_route)?,
    ))
}

/// Create and preallocate one immutable runtime generation.
pub(crate) unsafe extern "C" fn create(
    config: *const SessionConfig,
    ports: *const SessionPorts,
    output: *mut *mut Session,
) -> c_int {
    if config.is_null() || ports.is_null() || output.is_null() {
        return INVALID_ARGUMENT;
    }
    unsafe { output.write(ptr::null_mut()) };
    let config = unsafe { &*config };
    let ports = unsafe { &*ports };
    if config.struct_size < size_of::<SessionConfig>() as u32
        || ports.struct_size < size_of::<SessionPorts>() as u32
        || config.abi_version != ABI_VERSION
        || config.maximum_receive_frame_count == 0
        || config.maximum_transmit_frame_count == 0
        || config.publication_interval_ms == 0
        || config.receive_channel >= CANONICAL_CHANNELS
        || !config.receive_input_gain.is_finite()
        || config.receive_input_gain < 0.0
        || config.qualification.rx_on_delay_blocks > 3_000
        || config.qualification.tx_off_delay_blocks > 3_000
        || !config.transmit.output_a_tone_gain.is_finite()
        || !config.transmit.output_a_tone_bias.is_finite()
        || !config.transmit.output_b_tone_gain.is_finite()
        || !config.transmit.output_b_tone_bias.is_finite()
    {
        return INVALID_ARGUMENT;
    }
    if config.native_sample_rate_hz != NATIVE_SAMPLE_RATE_HZ
        || config.interleaved_channels != CANONICAL_CHANNELS
    {
        return UNSUPPORTED;
    }
    let (
        receive_config,
        qualification,
        receive_cpu_saver,
        receive_ctcss_enabled,
        receive_dcs_enabled,
    ) = match receive_config(config) {
        Ok(value) => value,
        Err(error) => return error,
    };
    // Setup above validates the only variable fields; profiles are built-in.
    let detector = receive_path::ReceivePath::new(receive_config)
        .expect("validated session receiver configuration");
    let (signaling_config, transmit_dcs_enabled, output_a_route, output_b_route) =
        match transmit_config(config.transmit) {
            Ok(value) => value,
            Err(error) => return error,
        };
    let signaling = match TransmitSignaling::new(signaling_config) {
        Ok(value) => value,
        Err(_) => return INVALID_ARGUMENT,
    };
    let publication_frames = u64::from(config.publication_interval_ms) * 48;
    let rx_max = config.maximum_receive_frame_count as usize;
    let tx_max = config.maximum_transmit_frame_count as usize;
    let mut dcs = dcs_transmit::TransmitState::default();
    dcs.configure(config.transmit.dcs_code, config.transmit.dcs_inverted);
    let session = Box::new(Session {
        generation_id: config.generation_id,
        ports: Ports {
            receive_deemphasis: ports.receive_deemphasis,
            receive_filter: ports.receive_filter,
            receive_ctcss_notch: ports.receive_ctcss_notch,
            receive_noise_reduction: ports.receive_noise_reduction,
            receive_dynamics: ports.receive_dynamics,
            transmit_program: ports.transmit_program,
            transmit_dcs_normal_filter: ports.transmit_dcs_normal_filter,
            transmit_dcs_turnoff_filter: ports.transmit_dcs_turnoff_filter,
            program_ring: ports.program_ring,
            receive_ctcss_tail_notch: ports.receive_ctcss_tail_notch,
        },
        receive: UnsafeCell::new(ReceiveWorker {
            detector,
            qualification,
            qualification_state: receive_qualification::State::default(),
            cpu_saver_enabled: receive_cpu_saver,
            cpu_saver_state: crate::RxCpuSaverState::default(),
            per_sample_noise_gate: matches!(
                qualification.carrier_source,
                receive_qualification::CarrierSource::DspNoise
            ),
            ctcss_enabled: receive_ctcss_enabled,
            dcs_enabled: receive_dcs_enabled,
            receive_channel: config.receive_channel as usize,
            input_gain: config.receive_input_gain,
            maximum_frames: rx_max,
            publication_interval_frames: publication_frames,
            next_publication_frame: publication_frames,
            sample_index: 0,
            previous_carrier: false,
            previous_subaudible: false,
            previous_keyed: false,
            previous_ctcss: -1,
            previous_dcs: false,
            input_mono: vec![0.0; rx_max],
            delay: vec![0.0; config.receive.native_squelch_delay_frames as usize],
            delay_index: 0,
            work_a: vec![0.0; rx_max],
            work_b: vec![0.0; rx_max],
            work_c: vec![0.0; rx_max],
        }),
        transmit: UnsafeCell::new(TransmitWorker {
            signaling,
            maximum_frames: tx_max,
            publication_interval_frames: publication_frames,
            next_publication_frame: publication_frames,
            sample_index: 0,
            ctcss_peak: config.transmit.ctcss_peak,
            dcs_peak: config.transmit.dcs_peak,
            dcs_enabled: transmit_dcs_enabled,
            output_a_route,
            output_b_route,
            output_a_tone_gain: config.transmit.output_a_tone_gain,
            output_a_tone_bias: config.transmit.output_a_tone_bias,
            output_b_tone_gain: config.transmit.output_b_tone_gain,
            output_b_tone_bias: config.transmit.output_b_tone_bias,
            ctcss_phase: 0.0,
            dcs,
            test_tone: calibrated_test_tone::State::default(),
            program: vec![0.0; tx_max],
            processed: vec![0.0; tx_max],
            ctcss: vec![0.0; tx_max],
            dcs_normal_raw: vec![0.0; tx_max],
            dcs_normal_filtered: vec![0.0; tx_max],
            dcs_turnoff_raw: vec![0.0; tx_max],
            dcs_turnoff_filtered: vec![0.0; tx_max],
            active: vec![0; tx_max],
        }),
        receive_busy: AtomicBool::new(false),
        transmit_busy: AtomicBool::new(false),
        warmed: AtomicBool::new(false),
        shared: SharedState {
            logical_ptt: AtomicBool::new(false),
            receiver_blanking_ms: AtomicI32::new(0),
        },
        snapshot: AtomicSnapshot::default(),
        receive_events: EventQueue::new(),
        transmit_events: EventQueue::new(),
    });
    unsafe { output.write(Box::into_raw(session)) };
    OK
}

unsafe fn run_processor(
    port: ProcessorPort,
    input: &[f32],
    output: &mut [f32],
) -> Result<(), c_int> {
    if let Some(process) = port.process_f32 {
        let result = unsafe {
            process(
                port.context,
                input.as_ptr(),
                output.as_mut_ptr(),
                input.len() as u32,
            )
        };
        if result != 0 {
            output.fill(0.0);
            return Err(PROVIDER_FAILED);
        }
    } else {
        output.copy_from_slice(input);
    }
    if output.iter().any(|sample| !sample.is_finite()) {
        output.fill(0.0);
        return Err(PROVIDER_FAILED);
    }
    Ok(())
}

unsafe fn warm_port(port: ProcessorPort, frames: u32) -> Result<(), c_int> {
    if let Some(warm) = port.warm {
        let result = unsafe { warm(port.context, frames) };
        if result != 0 {
            return Err(PROVIDER_FAILED);
        }
    }
    Ok(())
}

unsafe fn bypass_port(port: ProcessorPort, frames: u32) -> Result<(), c_int> {
    if let Some(bypass) = port.bypass {
        if unsafe { bypass(port.context, frames) } != 0 {
            return Err(PROVIDER_FAILED);
        }
    }
    Ok(())
}

unsafe fn run_optional_processor(
    port: ProcessorPort,
    input: &[f32],
    output: &mut [f32],
) -> Result<(), c_int> {
    if port.process_f32.is_some() {
        return unsafe { run_processor(port, input, output) };
    }
    unsafe { bypass_port(port, input.len() as u32) }?;
    output.copy_from_slice(input);
    Ok(())
}

unsafe fn run_receive_filters(
    ports: &Ports,
    decoded_ctcss: i16,
    tail_tone_active: bool,
    input: &[f32],
    filtered: &mut [f32],
    notched: &mut [f32],
) -> Result<bool, c_int> {
    unsafe { run_processor(ports.receive_filter, input, filtered) }?;
    let notch = tail_tone_active
        .then_some(ports.receive_ctcss_tail_notch)
        .or_else(|| {
            usize::try_from(decoded_ctcss)
                .ok()
                .and_then(|index| ports.receive_ctcss_notch.get(index))
                .copied()
        })
        .filter(|port| port.process_f32.is_some());
    let Some(port) = notch else {
        return Ok(false);
    };
    unsafe { run_processor(port, filtered, notched) }?;
    Ok(true)
}

/// Exercise each prepared external processor before audio starts.
pub(crate) unsafe extern "C" fn warm(session: *mut Session) -> c_int {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return INVALID_ARGUMENT;
    };
    if session.warmed.load(Ordering::Acquire) {
        return OK;
    }
    let _rx = match enter(&session.receive_busy) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let _tx = match enter(&session.transmit_busy) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let rx_frames = unsafe { (*session.receive.get()).maximum_frames as u32 };
    let tx_frames = unsafe { (*session.transmit.get()).maximum_frames as u32 };
    let receive = unsafe { &mut *session.receive.get() };
    receive.input_mono.fill(0.0);
    for port in [
        session.ports.receive_deemphasis,
        session.ports.receive_filter,
        session.ports.receive_noise_reduction,
        session.ports.receive_dynamics,
    ] {
        if unsafe { warm_port(port, rx_frames) }.is_err()
            || unsafe {
                run_processor(
                    port,
                    &receive.input_mono[..rx_frames as usize],
                    &mut receive.work_a[..rx_frames as usize],
                )
            }
            .is_err()
        {
            return PROVIDER_FAILED;
        }
        receive
            .input_mono
            .copy_from_slice(&receive.work_a[..rx_frames as usize]);
    }
    receive.input_mono.fill(0.0);
    for port in session
        .ports
        .receive_ctcss_notch
        .into_iter()
        .chain([session.ports.receive_ctcss_tail_notch])
    {
        if port.process_f32.is_some()
            && (unsafe { warm_port(port, rx_frames) }.is_err()
                || unsafe {
                    run_processor(
                        port,
                        &receive.input_mono[..rx_frames as usize],
                        &mut receive.work_a[..rx_frames as usize],
                    )
                }
                .is_err())
        {
            return PROVIDER_FAILED;
        }
    }
    let transmit = unsafe { &mut *session.transmit.get() };
    transmit.program.fill(0.0);
    for port in [
        session.ports.transmit_program,
        session.ports.transmit_dcs_normal_filter,
        session.ports.transmit_dcs_turnoff_filter,
    ] {
        if unsafe { warm_port(port, tx_frames) }.is_err()
            || unsafe {
                run_processor(
                    port,
                    &transmit.program[..tx_frames as usize],
                    &mut transmit.processed[..tx_frames as usize],
                )
            }
            .is_err()
        {
            return PROVIDER_FAILED;
        }
        transmit
            .program
            .copy_from_slice(&transmit.processed[..tx_frames as usize]);
    }
    if let Some(warm_ring) = session.ports.program_ring.warm {
        if unsafe { warm_ring(session.ports.program_ring.context, tx_frames) } != 0 {
            return PROVIDER_FAILED;
        }
    }
    session.warmed.store(true, Ordering::Release);
    OK
}

fn measure(samples: &[f32]) -> (f32, f32, u64) {
    let mut peak = 0.0_f32;
    let mut power = 0.0_f64;
    let mut rails = 0_u64;
    for &sample in samples {
        let magnitude = sample.abs();
        peak = peak.max(magnitude);
        power += f64::from(sample) * f64::from(sample);
        rails += u64::from(sample >= i16::MAX as f32 / 32_768.0 || sample <= -1.0);
    }
    let rms = if samples.is_empty() {
        0.0
    } else {
        (power / samples.len() as f64).sqrt() as f32
    };
    (peak, rms, rails)
}

fn measure_channel(samples: &[f32], channel: usize) -> (f32, f32, u64) {
    let mut peak = 0.0_f32;
    let mut power = 0.0_f64;
    let mut rails = 0_u64;
    let mut count = 0_u64;
    for sample in samples
        .iter()
        .skip(channel)
        .step_by(CANONICAL_CHANNELS as usize)
    {
        peak = peak.max(sample.abs());
        power += f64::from(*sample) * f64::from(*sample);
        rails += u64::from(*sample >= i16::MAX as f32 / 32_768.0 || *sample <= -1.0);
        count += 1;
    }
    let rms = if count == 0 {
        0.0
    } else {
        (power / count as f64).sqrt() as f32
    };
    (peak, rms, rails)
}

fn publication_due(sample_index: u64, next: &mut u64, interval: u64) -> bool {
    if sample_index < *next {
        return false;
    }
    while *next <= sample_index {
        *next = next.saturating_add(interval);
    }
    true
}

fn push_edge(
    queue: &EventQueue,
    generation_id: u64,
    sample_index: u64,
    kind: EventKind,
    value: i32,
) {
    queue.push(Event {
        generation_id,
        sample_index,
        kind: kind as u32,
        value,
    });
}

fn provider_failure(session: &Session, receive: bool, sample_index: u64) {
    session
        .snapshot
        .provider_failures
        .fetch_add(1, Ordering::Relaxed);
    let queue = if receive {
        &session.receive_events
    } else {
        &session.transmit_events
    };
    push_edge(
        queue,
        session.generation_id,
        sample_index,
        EventKind::ProviderFailure,
        1,
    );
}

/// Consume exactly one variable-size native capture span.
pub(crate) unsafe extern "C" fn receive(
    session: *mut Session,
    input: *const f32,
    output: *mut f32,
    frame_count: u32,
    controls: *const ReceiveInput,
    result: *mut ReceiveResult,
) -> c_int {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return INVALID_ARGUMENT;
    };
    if output.is_null() || result.is_null() || frame_count == 0 {
        return INVALID_ARGUMENT;
    }
    unsafe { ptr::write_bytes(output, 0, frame_count as usize) };
    if input.is_null() || controls.is_null() {
        return INVALID_ARGUMENT;
    }
    if !session.warmed.load(Ordering::Acquire) {
        return NOT_READY;
    }
    let _busy = match enter(&session.receive_busy) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let worker = unsafe { &mut *session.receive.get() };
    let count = frame_count as usize;
    if count > worker.maximum_frames {
        return FRAME_COUNT_EXCEEDED;
    }
    let input = unsafe { slice::from_raw_parts(input, count * 2) };
    if input.iter().any(|sample| !sample.is_finite()) {
        return INVALID_ARGUMENT;
    }
    let (input_peak, input_rms, input_rails) = measure_channel(input, worker.receive_channel);
    let controls = unsafe { *controls };
    let duration = session
        .shared
        .receiver_blanking_ms
        .swap(0, Ordering::AcqRel);
    if duration > 0 {
        worker.detector.arm_receive_blanking(duration);
    }
    // Count and finite PCM were checked above; detector state is session-owned.
    let detected = worker
        .detector
        .process_with_carrier(
            input,
            receive_qualification::external_carrier(
                worker.qualification.carrier_source,
                controls.hardware_carrier != 0,
                controls.parallel_carrier != 0,
            ),
        )
        .expect("validated session receiver span");
    let signal_mode = if worker.dcs_enabled {
        receive_qualification::SignalMode::Dcs
    } else if worker.ctcss_enabled {
        receive_qualification::SignalMode::Ctcss
    } else {
        receive_qualification::SignalMode::None
    };
    let tx_ptt = session.shared.logical_ptt.load(Ordering::Acquire);
    let mut qualification = worker.qualification;
    qualification.subaudible_override |= controls.subaudible_override != 0;
    let qualified = receive_qualification::advance(
        &qualification,
        &receive_qualification::Inputs {
            hardware_carrier: controls.hardware_carrier != 0,
            parallel_carrier: controls.parallel_carrier != 0,
            dsp_carrier: detected.carrier_detect,
            hardware_subaudible: controls.hardware_subaudible != 0,
            parallel_subaudible: controls.parallel_subaudible != 0,
            dcs_receive_enabled: worker.dcs_enabled,
            dcs_valid: detected.dcs_valid,
            ctcss_receive_enabled: worker.ctcss_enabled,
            ctcss_decoder_available: true,
            ctcss_decoded: detected.ctcss_decoded >= 0,
            signal_mode,
            tx_ptt_out: tx_ptt,
            txrx_blanking_active: detected.blanked_native_frames != 0,
        },
        frame_count,
        &mut worker.qualification_state,
    );
    rx_cpu_saver::advance(
        &crate::RxCpuSaverInput {
            enabled: u32::from(worker.cpu_saver_enabled),
            carrier_detect: u32::from(qualified.carrier_active),
            signal_mode_null: u32::from(signal_mode == receive_qualification::SignalMode::None),
            tx_ptt_in: 0,
            tx_ptt_out: u32::from(tx_ptt),
        },
        &mut worker.cpu_saver_state,
    )
    .expect("session CPU-saver inputs and state are boolean");
    worker
        .detector
        .set_voice_processing_active(worker.cpu_saver_state.halted == 0);

    for (index, sample) in worker.input_mono[..count].iter_mut().enumerate() {
        let captured = input[index * 2 + worker.receive_channel];
        if worker.delay.is_empty() {
            *sample = captured;
        } else {
            *sample = worker.delay[worker.delay_index];
            worker.delay[worker.delay_index] = captured;
            worker.delay_index += 1;
            if worker.delay_index == worker.delay.len() {
                worker.delay_index = 0;
            }
        }
    }
    if unsafe {
        run_processor(
            session.ports.receive_deemphasis,
            &worker.input_mono[..count],
            &mut worker.work_a[..count],
        )
    }
    .is_err()
    {
        provider_failure(session, true, worker.sample_index);
        return PROVIDER_FAILED;
    }
    if worker.per_sample_noise_gate {
        for (sample, gate) in worker.work_a[..count]
            .iter_mut()
            .zip(worker.detector.carrier_gates())
        {
            if *gate == 0 {
                *sample = 0.0;
            }
        }
    } else if !qualified.carrier_active {
        worker.work_a[..count].fill(0.0);
    }
    for sample in &mut worker.work_a[..count] {
        *sample *= worker.input_gain;
    }
    let notched = match unsafe {
        run_receive_filters(
            &session.ports,
            detected.ctcss_decoded,
            detected.ctcss_tail_tone_active,
            &worker.work_a[..count],
            &mut worker.work_b[..count],
            &mut worker.work_c[..count],
        )
    } {
        Ok(value) => value,
        Err(_) => {
            provider_failure(session, true, worker.sample_index);
            return PROVIDER_FAILED;
        }
    };
    if worker.cpu_saver_enabled && !qualified.rx_keyed {
        if unsafe { bypass_port(session.ports.receive_noise_reduction, frame_count) }.is_err() {
            provider_failure(session, true, worker.sample_index);
            return PROVIDER_FAILED;
        }
        if notched {
            worker.work_b[..count].copy_from_slice(&worker.work_c[..count]);
        }
    } else {
        let noise_result = if notched {
            unsafe {
                run_optional_processor(
                    session.ports.receive_noise_reduction,
                    &worker.work_c[..count],
                    &mut worker.work_a[..count],
                )
            }
        } else {
            unsafe {
                run_optional_processor(
                    session.ports.receive_noise_reduction,
                    &worker.work_b[..count],
                    &mut worker.work_a[..count],
                )
            }
        };
        if noise_result.is_err() {
            provider_failure(session, true, worker.sample_index);
            return PROVIDER_FAILED;
        }
        if unsafe {
            run_processor(
                session.ports.receive_dynamics,
                &worker.work_a[..count],
                &mut worker.work_b[..count],
            )
        }
        .is_err()
        {
            provider_failure(session, true, worker.sample_index);
            return PROVIDER_FAILED;
        }
    }
    unsafe { ptr::copy_nonoverlapping(worker.work_b.as_ptr(), output, count) };

    let first_sample = worker.sample_index;
    worker.sample_index = worker.sample_index.saturating_add(u64::from(frame_count));
    let due = publication_due(
        worker.sample_index,
        &mut worker.next_publication_frame,
        worker.publication_interval_frames,
    );
    let (output_peak, output_rms, output_rails) = measure(&worker.work_b[..count]);
    let decoded = i32::from(detected.ctcss_decoded);
    for (changed, kind, value) in [
        (
            worker.previous_carrier != qualified.carrier_active,
            EventKind::Carrier,
            i32::from(qualified.carrier_active),
        ),
        (
            worker.previous_subaudible != qualified.subaudible_active,
            EventKind::Subaudible,
            i32::from(qualified.subaudible_active),
        ),
        (
            worker.previous_keyed != qualified.rx_keyed,
            EventKind::ReceiverKeyed,
            i32::from(qualified.rx_keyed),
        ),
        (
            worker.previous_ctcss != decoded,
            EventKind::CtcssDecode,
            decoded,
        ),
        (
            worker.previous_dcs != detected.dcs_valid,
            EventKind::DcsDecode,
            i32::from(detected.dcs_valid),
        ),
    ] {
        if changed {
            push_edge(
                &session.receive_events,
                session.generation_id,
                worker.sample_index,
                kind,
                value,
            );
        }
    }
    worker.previous_carrier = qualified.carrier_active;
    worker.previous_subaudible = qualified.subaudible_active;
    worker.previous_keyed = qualified.rx_keyed;
    worker.previous_ctcss = decoded;
    worker.previous_dcs = detected.dcs_valid;
    session
        .snapshot
        .receive_frames
        .store(worker.sample_index, Ordering::Relaxed);
    session
        .snapshot
        .receive_input_peak
        .store(input_peak.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .receive_input_rms
        .store(input_rms.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .receive_ctcss_decoder_peak
        .store(detected.ctcss_decoder_peak.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .receive_output_peak
        .store(output_peak.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .receive_output_rms
        .store(output_rms.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .receive_input_rails
        .fetch_add(input_rails, Ordering::Relaxed);
    session
        .snapshot
        .receive_output_rails
        .fetch_add(output_rails, Ordering::Relaxed);
    session
        .snapshot
        .carrier
        .store(qualified.carrier_active, Ordering::Relaxed);
    session
        .snapshot
        .subaudible
        .store(qualified.subaudible_active, Ordering::Relaxed);
    session
        .snapshot
        .receiver_keyed
        .store(qualified.rx_keyed, Ordering::Relaxed);
    session
        .snapshot
        .decoded_ctcss
        .store(decoded, Ordering::Relaxed);
    session
        .snapshot
        .dcs_valid
        .store(detected.dcs_valid, Ordering::Release);
    unsafe {
        result.write(ReceiveResult {
            generation_id: session.generation_id,
            first_sample_index: first_sample,
            frame_count,
            carrier_active: u32::from(qualified.carrier_active),
            subaudible_active: u32::from(qualified.subaudible_active),
            receiver_keyed: u32::from(qualified.rx_keyed),
            ctcss_decoded_index: decoded,
            dcs_valid: u32::from(detected.dcs_valid),
            rssi_peak: detected.rssi_peak,
            rssi_updated: u32::from(detected.rssi_updated),
            ctcss_decoder_peak: detected.ctcss_decoder_peak,
            input_peak,
            input_rms,
            input_rail_samples: input_rails,
            output_peak,
            output_rms,
            output_rail_samples: output_rails,
            periodic_status_due: u32::from(due),
        })
    };
    OK
}

unsafe fn render_ring(
    port: ProgramRingPort,
    output: &mut [f32],
    result: &mut ProgramRingResult,
) -> Result<(), c_int> {
    output.fill(0.0);
    if let Some(render) = port.render_f32 {
        let status = unsafe {
            render(
                port.context,
                output.as_mut_ptr(),
                output.len() as u32,
                result,
            )
        };
        if status != 0
            || output.iter().any(|sample| !sample.is_finite())
            || !(0..=1).contains(&result.receiver_keyed)
            || !(0..=1).contains(&result.dcs_valid)
            || result.ctcss_decoded_index < -1
            || result.ctcss_decoded_index >= crate::ctcss_receive::TONE_COUNT as i32
        {
            output.fill(0.0);
            return Err(PROVIDER_FAILED);
        }
    }
    Ok(())
}

fn render_ctcss(worker: &mut TransmitWorker, output: &transmit_signaling::Output, count: usize) {
    worker.ctcss[..count].fill(0.0);
    for span in &output.render_spans[..output.render_span_count] {
        let start = span.frame_offset as usize;
        let frames = span.frame_count as usize;
        let destination = worker.ctcss[start..start + frames].as_mut_ptr();
        let tail_frequency = span.ctcss_render.tail_tone_hz;
        let frequency = if tail_frequency > 0.0 {
            tail_frequency
        } else {
            ctcss_transmit::legacy_frequency(f64::from(span.ctcss_frequency_tenths_hz) / 10.0)
        };
        unsafe {
            ctcss_transmit::generate(
                &mut worker.ctcss_phase,
                destination,
                span.frame_count,
                frequency,
                worker.ctcss_peak,
                u32::from(span.logical_ptt && span.ctcss_render.enabled != 0),
                span.ctcss_render.phase_shift_degrees,
            )
        };
    }
}

fn render_dcs(worker: &mut TransmitWorker, output: &transmit_signaling::Output, count: usize) {
    worker.dcs_normal_raw[..count].fill(0.0);
    worker.dcs_turnoff_raw[..count].fill(0.0);
    for span in &output.render_spans[..output.render_span_count] {
        let start = span.frame_offset as usize;
        let destination = if span.dcs_turnoff_active {
            &mut worker.dcs_turnoff_raw
        } else {
            &mut worker.dcs_normal_raw
        };
        unsafe {
            dcs_transmit::generate(
                &mut worker.dcs,
                destination[start..].as_mut_ptr(),
                span.frame_count,
                dcs_transmit::GenerateConfig {
                    peak: worker.dcs_peak,
                    enabled: u32::from(span.logical_ptt && worker.dcs_enabled),
                    turnoff: u32::from(span.dcs_turnoff_active),
                },
            )
        };
    }
}

fn route_sample(
    route: OutputRoute,
    voice: f32,
    ctcss: f32,
    dcs: f32,
    tone_gain: f32,
    bias: f32,
) -> f32 {
    let mut value = 0.0_f32;
    if route.voice() {
        value += voice;
    }
    if route.tone() {
        value += ctcss * tone_gain + bias + dcs;
    }
    value.clamp(-1.0, 1.0)
}

/// Produce exactly one variable-size native playback span.
pub(crate) unsafe extern "C" fn transmit(
    session: *mut Session,
    output: *mut f32,
    frame_count: u32,
    controls: *const TransmitInput,
    result: *mut TransmitResult,
) -> c_int {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return INVALID_ARGUMENT;
    };
    if output.is_null() || result.is_null() || controls.is_null() || frame_count == 0 {
        return INVALID_ARGUMENT;
    }
    unsafe { ptr::write_bytes(output, 0, frame_count as usize * 2) };
    if !session.warmed.load(Ordering::Acquire) {
        return NOT_READY;
    }
    let _busy = match enter(&session.transmit_busy) {
        Ok(value) => value,
        Err(error) => return error,
    };
    let worker = unsafe { &mut *session.transmit.get() };
    let count = frame_count as usize;
    if count > worker.maximum_frames {
        return FRAME_COUNT_EXCEEDED;
    }
    let controls = unsafe { *controls };
    let mut ring = ProgramRingResult::default();
    if unsafe {
        render_ring(
            session.ports.program_ring,
            &mut worker.program[..count],
            &mut ring,
        )
    }
    .is_err()
    {
        provider_failure(session, false, worker.sample_index);
        return PROVIDER_FAILED;
    }
    if unsafe {
        run_processor(
            session.ports.transmit_program,
            &worker.program[..count],
            &mut worker.processed[..count],
        )
    }
    .is_err()
    {
        provider_failure(session, false, worker.sample_index);
        return PROVIDER_FAILED;
    }
    unsafe {
        worker.test_tone.render(
            worker.processed.as_mut_ptr(),
            frame_count,
            controls.calibrated_test_tone,
        )
    };
    let (program_peak, program_rms, program_rails) = measure(&worker.processed[..count]);
    let signaling = match worker.signaling.tick(transmit_signaling::Input {
        native_frame_count: frame_count,
        tx_render_admitted: controls.render_admitted != 0,
        external_ptt_request: controls.external_ptt_request != 0,
        physical_ptt_applied: controls.physical_ptt_applied != 0,
        decoded_ctcss: ring.ctcss_decoded_index,
        dcs_valid: ring.dcs_valid != 0,
        ctcss_inhibit: controls.ctcss_inhibit != 0,
        forced_ctcss_tenths_hz: controls.forced_ctcss_tenths_hz,
    }) {
        Ok(value) => value,
        Err(_) => return INVALID_ARGUMENT,
    };
    render_ctcss(worker, &signaling, count);
    render_dcs(worker, &signaling, count);
    if unsafe {
        run_processor(
            session.ports.transmit_dcs_normal_filter,
            &worker.dcs_normal_raw[..count],
            &mut worker.dcs_normal_filtered[..count],
        )
    }
    .is_err()
    {
        provider_failure(session, false, worker.sample_index);
        return PROVIDER_FAILED;
    }
    if unsafe {
        run_processor(
            session.ports.transmit_dcs_turnoff_filter,
            &worker.dcs_turnoff_raw[..count],
            &mut worker.dcs_turnoff_filtered[..count],
        )
    }
    .is_err()
    {
        provider_failure(session, false, worker.sample_index);
        return PROVIDER_FAILED;
    }
    worker.active[..count].fill(0);
    for span in &signaling.render_spans[..signaling.render_span_count] {
        if span.logical_ptt {
            let start = span.frame_offset as usize;
            worker.active[start..start + span.frame_count as usize].fill(1);
        }
    }
    let output_slice = unsafe { slice::from_raw_parts_mut(output, count * 2) };
    for index in 0..count {
        if worker.active[index] == 0 {
            continue;
        }
        output_slice[index * 2] = route_sample(
            worker.output_a_route,
            worker.processed[index],
            worker.ctcss[index],
            worker.dcs_normal_filtered[index] + worker.dcs_turnoff_filtered[index],
            worker.output_a_tone_gain,
            worker.output_a_tone_bias,
        );
        output_slice[index * 2 + 1] = route_sample(
            worker.output_b_route,
            worker.processed[index],
            worker.ctcss[index],
            worker.dcs_normal_filtered[index] + worker.dcs_turnoff_filtered[index],
            worker.output_b_tone_gain,
            worker.output_b_tone_bias,
        );
    }
    let first_sample = worker.sample_index;
    worker.sample_index = worker.sample_index.saturating_add(u64::from(frame_count));
    let due = publication_due(
        worker.sample_index,
        &mut worker.next_publication_frame,
        worker.publication_interval_frames,
    );
    let (output_peak, output_rms, output_rails) = measure(output_slice);
    if signaling.logical_ptt_raised || signaling.logical_ptt_released {
        push_edge(
            &session.transmit_events,
            session.generation_id,
            worker.sample_index,
            EventKind::Ptt,
            i32::from(signaling.logical_ptt),
        );
    }
    if signaling.ctcss_status_event {
        push_edge(
            &session.transmit_events,
            session.generation_id,
            worker.sample_index,
            EventKind::CtcssTransmit,
            signaling.signal_mode.tx_ctcss_frequency_tenths_hz,
        );
    }
    if let Some(blanking) = signaling.receiver_blanking_arm {
        session
            .shared
            .receiver_blanking_ms
            .store(blanking.duration_ms, Ordering::Release);
        push_edge(
            &session.transmit_events,
            session.generation_id,
            worker.sample_index,
            EventKind::ReceiverBlanking,
            blanking.duration_ms,
        );
    }
    session
        .shared
        .logical_ptt
        .store(signaling.logical_ptt, Ordering::Release);
    session
        .snapshot
        .transmit_frames
        .store(worker.sample_index, Ordering::Relaxed);
    session
        .snapshot
        .transmit_program_peak
        .store(program_peak.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .transmit_program_rms
        .store(program_rms.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .transmit_output_peak
        .store(output_peak.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .transmit_output_rms
        .store(output_rms.to_bits(), Ordering::Relaxed);
    session
        .snapshot
        .transmit_program_rails
        .fetch_add(program_rails, Ordering::Relaxed);
    session
        .snapshot
        .transmit_output_rails
        .fetch_add(output_rails, Ordering::Relaxed);
    session
        .snapshot
        .logical_ptt
        .store(signaling.logical_ptt, Ordering::Release);
    store_ring(&session.snapshot, ring.observation);
    unsafe {
        result.write(TransmitResult {
            generation_id: session.generation_id,
            first_sample_index: first_sample,
            frame_count,
            logical_ptt: u32::from(signaling.logical_ptt),
            transmitter_state: signaling.transmitter_state,
            selected_ctcss_tenths_hz: signaling.signal_mode.tx_ctcss_frequency_tenths_hz,
            program_peak,
            program_rms,
            program_rail_samples: program_rails,
            output_peak,
            output_rms,
            output_rail_samples: output_rails,
            periodic_status_due: u32::from(due),
            program_ring: ring.observation,
        })
    };
    OK
}

fn store_ring(snapshot: &AtomicSnapshot, ring: RingObservation) {
    snapshot
        .ring_occupancy
        .store(ring.occupancy_frames, Ordering::Relaxed);
    snapshot
        .ring_reserve
        .store(ring.reserve_frames, Ordering::Relaxed);
    snapshot
        .ring_target
        .store(ring.target_frames, Ordering::Relaxed);
    snapshot
        .ring_capacity
        .store(ring.capacity_frames, Ordering::Relaxed);
    snapshot
        .ring_ratio
        .store(ring.ratio.to_bits(), Ordering::Relaxed);
    snapshot
        .ring_underruns
        .store(ring.underrun_samples, Ordering::Relaxed);
    snapshot
        .ring_overruns
        .store(ring.overrun_samples, Ordering::Relaxed);
    snapshot
        .ring_concealment
        .store(ring.concealment_samples, Ordering::Release);
}

fn load_ring(snapshot: &AtomicSnapshot) -> RingObservation {
    RingObservation {
        occupancy_frames: snapshot.ring_occupancy.load(Ordering::Relaxed),
        reserve_frames: snapshot.ring_reserve.load(Ordering::Relaxed),
        target_frames: snapshot.ring_target.load(Ordering::Relaxed),
        capacity_frames: snapshot.ring_capacity.load(Ordering::Relaxed),
        ratio: f64::from_bits(snapshot.ring_ratio.load(Ordering::Relaxed)),
        underrun_samples: snapshot.ring_underruns.load(Ordering::Relaxed),
        overrun_samples: snapshot.ring_overruns.load(Ordering::Relaxed),
        concealment_samples: snapshot.ring_concealment.load(Ordering::Acquire),
    }
}

/// Read the latest independently published RX/TX observations without locking.
pub(crate) unsafe extern "C" fn snapshot(session: *const Session, output: *mut Snapshot) -> c_int {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return INVALID_ARGUMENT;
    };
    if output.is_null() {
        return INVALID_ARGUMENT;
    }
    unsafe {
        output.write(Snapshot {
            generation_id: session.generation_id,
            receive_frames: session.snapshot.receive_frames.load(Ordering::Acquire),
            transmit_frames: session.snapshot.transmit_frames.load(Ordering::Acquire),
            receive_input_peak: f32::from_bits(
                session.snapshot.receive_input_peak.load(Ordering::Relaxed),
            ),
            receive_input_rms: f32::from_bits(
                session.snapshot.receive_input_rms.load(Ordering::Relaxed),
            ),
            receive_ctcss_decoder_peak: f32::from_bits(
                session
                    .snapshot
                    .receive_ctcss_decoder_peak
                    .load(Ordering::Relaxed),
            ),
            receive_output_peak: f32::from_bits(
                session.snapshot.receive_output_peak.load(Ordering::Relaxed),
            ),
            receive_output_rms: f32::from_bits(
                session.snapshot.receive_output_rms.load(Ordering::Relaxed),
            ),
            transmit_program_peak: f32::from_bits(
                session
                    .snapshot
                    .transmit_program_peak
                    .load(Ordering::Relaxed),
            ),
            transmit_program_rms: f32::from_bits(
                session
                    .snapshot
                    .transmit_program_rms
                    .load(Ordering::Relaxed),
            ),
            transmit_output_peak: f32::from_bits(
                session
                    .snapshot
                    .transmit_output_peak
                    .load(Ordering::Relaxed),
            ),
            transmit_output_rms: f32::from_bits(
                session.snapshot.transmit_output_rms.load(Ordering::Relaxed),
            ),
            receive_input_rail_samples: session
                .snapshot
                .receive_input_rails
                .load(Ordering::Relaxed),
            receive_output_rail_samples: session
                .snapshot
                .receive_output_rails
                .load(Ordering::Relaxed),
            transmit_program_rail_samples: session
                .snapshot
                .transmit_program_rails
                .load(Ordering::Relaxed),
            transmit_output_rail_samples: session
                .snapshot
                .transmit_output_rails
                .load(Ordering::Relaxed),
            provider_failures: session.snapshot.provider_failures.load(Ordering::Relaxed),
            receive_event_drops: session.receive_events.drops.load(Ordering::Relaxed),
            transmit_event_drops: session.transmit_events.drops.load(Ordering::Relaxed),
            carrier_active: u32::from(session.snapshot.carrier.load(Ordering::Relaxed)),
            subaudible_active: u32::from(session.snapshot.subaudible.load(Ordering::Relaxed)),
            receiver_keyed: u32::from(session.snapshot.receiver_keyed.load(Ordering::Relaxed)),
            logical_ptt: u32::from(session.snapshot.logical_ptt.load(Ordering::Relaxed)),
            ctcss_decoded_index: session.snapshot.decoded_ctcss.load(Ordering::Relaxed),
            dcs_valid: u32::from(session.snapshot.dcs_valid.load(Ordering::Acquire)),
            program_ring: load_ring(&session.snapshot),
        })
    };
    OK
}

/// Pop one receive-owner event; return zero when no event is available.
pub(crate) unsafe extern "C" fn pop_receive_event(
    session: *const Session,
    output: *mut Event,
) -> u32 {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return 0;
    };
    let Some(output) = (unsafe { output.as_mut() }) else {
        return 0;
    };
    u32::from(session.receive_events.pop(output))
}

/// Pop one transmit-owner event; return zero when no event is available.
pub(crate) unsafe extern "C" fn pop_transmit_event(
    session: *const Session,
    output: *mut Event,
) -> u32 {
    let Some(session) = (unsafe { session.as_ref() }) else {
        return 0;
    };
    let Some(output) = (unsafe { output.as_mut() }) else {
        return 0;
    };
    u32::from(session.transmit_events.pop(output))
}

/// Destroy a stopped session; passing null is harmless.
pub(crate) unsafe extern "C" fn destroy(session: *mut Session) {
    if !session.is_null() {
        unsafe { drop(Box::from_raw(session)) };
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    struct TestProcessor {
        bias: f32,
        gain: f32,
        calls: u32,
        nonzero_samples: u32,
        fail: bool,
    }

    impl Default for TestProcessor {
        fn default() -> Self {
            Self {
                bias: 0.0,
                gain: 1.0,
                calls: 0,
                nonzero_samples: 0,
                fail: false,
            }
        }
    }

    unsafe extern "C" fn test_process(
        context: *mut c_void,
        input: *const f32,
        output: *mut f32,
        frame_count: u32,
    ) -> c_int {
        let context = unsafe { &mut *context.cast::<TestProcessor>() };
        context.calls += 1;
        if context.fail {
            return -1;
        }
        for index in 0..frame_count as usize {
            let sample = unsafe { *input.add(index) };
            context.nonzero_samples += u32::from(sample != 0.0);
            unsafe {
                output
                    .add(index)
                    .write(sample * context.gain + context.bias)
            };
        }
        0
    }

    fn test_port(context: &mut TestProcessor) -> ProcessorPort {
        ProcessorPort {
            context: ptr::from_mut(context).cast::<c_void>(),
            process_f32: Some(test_process),
            ..ProcessorPort::default()
        }
    }

    fn valid_config() -> SessionConfig {
        SessionConfig {
            struct_size: size_of::<SessionConfig>() as u32,
            abi_version: ABI_VERSION,
            generation_id: 42,
            native_sample_rate_hz: NATIVE_SAMPLE_RATE_HZ,
            interleaved_channels: CANONICAL_CHANNELS,
            maximum_receive_frame_count: 16,
            maximum_transmit_frame_count: 16,
            publication_interval_ms: 50,
            receive_input_gain: 1.0,
            receive: ReceiveConfig {
                ctcss_decoder_gain: 1.0,
                ..ReceiveConfig::default()
            },
            qualification: QualificationConfig {
                carrier_source: 3,
                radio_duplex: 1,
                ..QualificationConfig::default()
            },
            transmit: TransmitConfig {
                output_a_route: 1,
                output_b_route: 1,
                ..TransmitConfig::default()
            },
            ..SessionConfig::default()
        }
    }

    fn create_result(config: &SessionConfig) -> c_int {
        let ports = SessionPorts::default();
        let mut session = ptr::null_mut();
        let result = unsafe { create(config, &ports, &mut session) };
        if result == OK {
            assert!(!session.is_null());
            unsafe { destroy(session) };
        } else {
            assert!(session.is_null());
        }
        result
    }

    #[test]
    fn event_queue_is_bounded_fifo() {
        let queue = EventQueue::new();
        for value in 0..EVENT_CAPACITY as i32 {
            queue.push(Event {
                value,
                ..Event::default()
            });
        }
        queue.push(Event {
            value: 99,
            ..Event::default()
        });
        assert_eq!(queue.drops.load(Ordering::Relaxed), 1);
        let mut event = Event::default();
        for value in 0..EVENT_CAPACITY as i32 {
            assert!(queue.pop(&mut event));
            assert_eq!(event.value, value);
        }
        assert!(!queue.pop(&mut event));
    }

    #[test]
    fn publication_deadline_has_no_partition_drift() {
        let mut next = 2_400;
        assert!(!publication_due(2_399, &mut next, 2_400));
        assert!(publication_due(2_401, &mut next, 2_400));
        assert_eq!(next, 4_800);
        assert!(publication_due(9_601, &mut next, 2_400));
        assert_eq!(next, 12_000);
    }

    #[test]
    fn output_routes_and_meter_are_normalized() {
        assert!(OutputRoute::parse(5).is_err());
        assert_eq!(
            route_sample(OutputRoute::Voice, 2.0, 0.0, 0.0, 1.0, 0.0),
            1.0
        );
        assert_eq!(
            route_sample(OutputRoute::Tone, 0.0, -2.0, 0.0, 1.0, 0.0),
            -1.0
        );
        assert_eq!(
            route_sample(OutputRoute::Tone, 0.0, 0.1, 0.2, 2.0, 0.3),
            0.7
        );
        assert_eq!(
            route_sample(OutputRoute::Disabled, 1.0, 1.0, 1.0, 1.0, 1.0),
            0.0
        );
        let (peak, rms, rails) = measure(&[1.0, -1.0]);
        assert_eq!((peak, rms, rails), (1.0, 1.0, 2));
    }

    #[test]
    fn callback_subaudible_override_admits_an_active_hardware_carrier() {
        let mut config = valid_config();
        config.qualification.carrier_source = 3;
        config.qualification.subaudible_source = 1;
        let ports = SessionPorts::default();
        let mut session = ptr::null_mut();
        assert_eq!(unsafe { create(&config, &ports, &mut session) }, OK);
        assert_eq!(unsafe { warm(session) }, OK);

        let input = [0.0; 8];
        let mut output = [0.0; 4];
        let mut result = ReceiveResult::default();
        let mut controls = ReceiveInput {
            hardware_carrier: 1,
            ..ReceiveInput::default()
        };
        assert_eq!(
            unsafe {
                receive(
                    session,
                    input.as_ptr(),
                    output.as_mut_ptr(),
                    4,
                    &controls,
                    &mut result,
                )
            },
            OK
        );
        assert_eq!(result.receiver_keyed, 0);

        controls.subaudible_override = 1;
        assert_eq!(
            unsafe {
                receive(
                    session,
                    input.as_ptr(),
                    output.as_mut_ptr(),
                    4,
                    &controls,
                    &mut result,
                )
            },
            OK
        );
        assert_eq!(result.receiver_keyed, 1);
        unsafe { destroy(session) };
    }

    #[test]
    fn receive_filter_selects_decoded_or_tail_prepared_notch() {
        let mut fixed = TestProcessor {
            bias: 1.0,
            ..TestProcessor::default()
        };
        let mut selected = TestProcessor {
            bias: 2.0,
            ..TestProcessor::default()
        };
        let mut other = TestProcessor {
            bias: 4.0,
            ..TestProcessor::default()
        };
        let mut tail = TestProcessor {
            bias: 8.0,
            ..TestProcessor::default()
        };
        let mut ports = Ports {
            receive_deemphasis: ProcessorPort::default(),
            receive_filter: test_port(&mut fixed),
            receive_ctcss_notch: [ProcessorPort::default(); crate::ctcss_receive::TONE_COUNT],
            receive_noise_reduction: ProcessorPort::default(),
            receive_dynamics: ProcessorPort::default(),
            transmit_program: ProcessorPort::default(),
            transmit_dcs_normal_filter: ProcessorPort::default(),
            transmit_dcs_turnoff_filter: ProcessorPort::default(),
            program_ring: ProgramRingPort::default(),
            receive_ctcss_tail_notch: test_port(&mut tail),
        };
        ports.receive_ctcss_notch[5] = test_port(&mut selected);
        ports.receive_ctcss_notch[6] = test_port(&mut other);
        let input = [0.25, -0.25];
        let mut filtered = [0.0; 2];
        let mut notched = [0.0; 2];

        assert!(!unsafe {
            run_receive_filters(&ports, -1, false, &input, &mut filtered, &mut notched).unwrap()
        });
        assert_eq!(filtered, [1.25, 0.75]);
        assert_eq!(selected.calls, 0);
        assert!(unsafe {
            run_receive_filters(&ports, 5, false, &input, &mut filtered, &mut notched).unwrap()
        });
        assert_eq!(notched, [3.25, 2.75]);
        assert_eq!(selected.calls, 1);
        assert_eq!(other.calls, 0);

        assert!(unsafe {
            run_receive_filters(&ports, -1, true, &input, &mut filtered, &mut notched).unwrap()
        });
        assert_eq!(notched, [9.25, 8.75]);
        assert_eq!(tail.calls, 1);
        assert_eq!(selected.calls, 1);

        selected.fail = true;
        assert_eq!(
            unsafe { run_receive_filters(&ports, 5, false, &input, &mut filtered, &mut notched) },
            Err(PROVIDER_FAILED)
        );
        assert_eq!(notched, [0.0; 2]);
    }

    #[test]
    fn dcs_normal_and_turnoff_sources_use_distinct_prepared_shapers() {
        let mut config = valid_config();
        config.transmit.dcs_transmit_enabled = 1;
        config.transmit.dcs_turnoff_enabled = 1;
        config.transmit.dcs_turnoff_duration_ms = 20;
        config.transmit.dcs_code = 0o023;
        config.transmit.dcs_peak = 0.1;
        config.transmit.output_a_route = 2;
        config.transmit.output_b_route = 2;

        let mut normal = TestProcessor {
            gain: 2.0,
            ..TestProcessor::default()
        };
        let mut turnoff = TestProcessor {
            gain: 3.0,
            ..TestProcessor::default()
        };
        let ports = SessionPorts {
            transmit_dcs_normal_filter: test_port(&mut normal),
            transmit_dcs_turnoff_filter: test_port(&mut turnoff),
            ..SessionPorts::default()
        };
        let mut session = ptr::null_mut();
        assert_eq!(unsafe { create(&config, &ports, &mut session) }, OK);
        assert_eq!(unsafe { warm(session) }, OK);

        let mut pcm = [0.0; 8];
        let mut result = TransmitResult::default();
        let keyed = TransmitInput {
            external_ptt_request: 1,
            physical_ptt_applied: 1,
            render_admitted: 1,
            ..TransmitInput::default()
        };
        assert_eq!(
            unsafe { transmit(session, pcm.as_mut_ptr(), 4, &keyed, &mut result) },
            OK
        );
        assert_eq!(normal.nonzero_samples, 4);
        assert_eq!(turnoff.nonzero_samples, 0);
        assert!(pcm.iter().all(|sample| sample.abs() == 0.2));

        let released = TransmitInput {
            physical_ptt_applied: 1,
            render_admitted: 1,
            ..TransmitInput::default()
        };
        assert_eq!(
            unsafe { transmit(session, pcm.as_mut_ptr(), 4, &released, &mut result) },
            OK
        );
        assert_eq!(normal.nonzero_samples, 4);
        assert!(turnoff.nonzero_samples > 0);
        assert!(pcm.iter().any(|sample| *sample != 0.0));
        unsafe { destroy(session) };
    }

    #[test]
    fn session_creation_rejects_nonsemantic_configuration_values() {
        assert_eq!(create_result(&valid_config()), OK);

        let mut invalid = Vec::new();
        let mut candidate = valid_config();
        candidate.receive.noise_filter_profile = 2;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive.squelch_open_level = MAX_SIGNED_PCM_LEVEL + 1;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive.squelch_hysteresis = MAX_SIGNED_PCM_LEVEL + 1;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive.vox_threshold = -1;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive.vox_hang_ms = i32::from(i16::MAX) + 1;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive.ctcss_tone_mask = 1_u64 << crate::ctcss_receive::TONE_COUNT;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive.ctcss_enabled = 1;
        candidate.receive.dcs_enabled = 1;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive.dcs_enabled = 1;
        candidate.receive.dcs_code = 0o1000;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.receive_input_gain = -1.0;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.qualification.rx_on_delay_blocks = 3_001;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.qualification.tx_off_delay_blocks = 3_001;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.transmit.ctcss_transmit_enabled = 1;
        candidate.transmit.dcs_transmit_enabled = 1;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.transmit.dcs_transmit_enabled = 1;
        candidate.transmit.dcs_code = 0o1000;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.transmit.dcs_inverted = 2;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.transmit.ctcss_peak = 1.01;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.transmit.dcs_peak = f32::NAN;
        invalid.push(candidate);
        candidate = valid_config();
        candidate.transmit.output_a_route = 5;
        invalid.push(candidate);

        for config in invalid {
            assert_eq!(create_result(&config), INVALID_ARGUMENT);
        }
    }

    /// Own a prepared engine so every failing assertion still releases its buffers.
    struct TestSession(*mut Session);

    impl TestSession {
        fn new(config: &SessionConfig, ports: &SessionPorts) -> Self {
            let mut session = ptr::null_mut();
            assert_eq!(unsafe { create(config, ports, &mut session) }, OK);
            Self(session)
        }

        fn prepared(config: &SessionConfig, ports: &SessionPorts) -> Self {
            let session = Self::new(config, ports);
            assert_eq!(unsafe { warm(session.0) }, OK);
            session
        }

        fn receive(&self, keyed: bool) -> (c_int, [f32; 4], ReceiveResult) {
            let input = [0.25; 8];
            let mut output = [9.0; 4];
            let mut result = ReceiveResult::default();
            let controls = ReceiveInput {
                hardware_carrier: u32::from(keyed),
                ..ReceiveInput::default()
            };
            let status = unsafe {
                receive(
                    self.0,
                    input.as_ptr(),
                    output.as_mut_ptr(),
                    4,
                    &controls,
                    &mut result,
                )
            };
            (status, output, result)
        }

        fn transmit(&self, keyed: bool) -> (c_int, [f32; 8], TransmitResult) {
            let mut output = [9.0; 8];
            let mut result = TransmitResult::default();
            let controls = TransmitInput {
                external_ptt_request: u32::from(keyed),
                physical_ptt_applied: u32::from(keyed),
                render_admitted: 1,
                ..TransmitInput::default()
            };
            let status =
                unsafe { transmit(self.0, output.as_mut_ptr(), 4, &controls, &mut result) };
            (status, output, result)
        }

        fn snapshot(&self) -> Snapshot {
            let mut result = Snapshot::default();
            assert_eq!(unsafe { snapshot(self.0, &mut result) }, OK);
            result
        }
    }

    impl Drop for TestSession {
        fn drop(&mut self) {
            unsafe { destroy(self.0) };
        }
    }

    unsafe extern "C" fn test_warm(context: *mut c_void, _frames: u32) -> c_int {
        let context = unsafe { &mut *context.cast::<TestProcessor>() };
        context.calls += 1;
        -i32::from(context.fail)
    }

    #[derive(Default)]
    struct TestRing {
        sample: f32,
        result: ProgramRingResult,
        fail: bool,
    }

    unsafe extern "C" fn test_ring(
        context: *mut c_void,
        output: *mut f32,
        frames: u32,
        result: *mut ProgramRingResult,
    ) -> c_int {
        let ring = unsafe { &*context.cast::<TestRing>() };
        unsafe { slice::from_raw_parts_mut(output, frames as usize) }.fill(ring.sample);
        unsafe { result.write(ring.result) };
        -i32::from(ring.fail)
    }

    fn ring_port(ring: &mut TestRing) -> ProgramRingPort {
        ProgramRingPort {
            context: ptr::from_mut(ring).cast(),
            render_f32: Some(test_ring),
            warm: None,
        }
    }

    #[test]
    fn create_validates_every_public_setup_boundary() {
        let edits: &[fn(&mut SessionConfig)] = &[
            |c| c.struct_size = 0,
            |c| c.abi_version = ABI_VERSION - 1,
            |c| c.maximum_receive_frame_count = 0,
            |c| c.maximum_transmit_frame_count = 0,
            |c| c.publication_interval_ms = 0,
            |c| c.receive_channel = 2,
            |c| c.receive_input_gain = f32::NAN,
            |c| c.transmit.output_a_tone_gain = f32::INFINITY,
            |c| c.transmit.output_a_tone_bias = f32::NAN,
            |c| c.transmit.output_b_tone_gain = f32::INFINITY,
            |c| c.transmit.output_b_tone_bias = f32::NAN,
            |c| c.receive.ctcss_enabled = 2,
            |c| c.receive.dcs_enabled = 2,
            |c| c.receive.cpu_saver_enabled = 2,
            |c| c.receive.ctcss_decoder_gain = f32::NAN,
            |c| c.receive.ctcss_decoder_gain = -1.0,
            |c| c.receive.ctcss_decoder_gain = i32::MAX as f32,
            |c| c.receive.vox_threshold = i32::MAX,
            |c| c.qualification.carrier_source = 7,
            |c| c.qualification.subaudible_source = 6,
            |c| c.qualification.subaudible_override = 2,
            |c| c.qualification.advanced_transport = 2,
            |c| c.qualification.radio_duplex = 2,
            |c| c.receive.ctcss_relax = 2,
            |c| c.receive.dcs_inverted = 2,
            |c| c.transmit.ctcss_transmit_enabled = 2,
            |c| c.transmit.dcs_transmit_enabled = 2,
            |c| c.transmit.dcs_peak = 1.01,
            |c| c.transmit.ctcss_peak = f32::NAN,
            |c| c.transmit.tone_off_mode = 4,
            |c| c.transmit.dcs_turnoff_enabled = 2,
            |c| c.transmit.cpu_saver_enabled = 2,
            |c| c.transmit.output_b_route = 5,
            |c| c.transmit.tx_settle_time_ms = -1,
        ];
        for edit in edits {
            let mut config = valid_config();
            edit(&mut config);
            assert_eq!(create_result(&config), INVALID_ARGUMENT);
        }
        for edit in [
            (|c: &mut SessionConfig| c.native_sample_rate_hz = 8_000) as fn(&mut SessionConfig),
            |c| c.interleaved_channels = 1,
        ] {
            let mut config = valid_config();
            edit(&mut config);
            assert_eq!(create_result(&config), UNSUPPORTED);
        }
        let config = valid_config();
        let ports = SessionPorts::default();
        let mut output = ptr::null_mut();
        unsafe {
            assert_eq!(create(ptr::null(), &ports, &mut output), INVALID_ARGUMENT);
            assert_eq!(create(&config, ptr::null(), &mut output), INVALID_ARGUMENT);
            assert_eq!(create(&config, &ports, ptr::null_mut()), INVALID_ARGUMENT);
            let short_ports = SessionPorts {
                struct_size: 0,
                ..ports
            };
            assert_eq!(create(&config, &short_ports, &mut output), INVALID_ARGUMENT);
            destroy(ptr::null_mut());
        }
        for carrier in 0..=6 {
            for subaudible in 0..=5 {
                let mut config = valid_config();
                config.qualification.carrier_source = carrier;
                config.qualification.subaudible_source = subaudible;
                config.receive.vox_hang_ms = 50;
                assert_eq!(create_result(&config), OK);
            }
        }
        for mode in 0..=3 {
            let mut config = valid_config();
            config.transmit.tone_off_mode = mode;
            assert_eq!(create_result(&config), OK);
        }
        for enabled in [false, true] {
            let mut config = valid_config();
            config.receive.ctcss_enabled = u32::from(enabled);
            config.receive.dcs_enabled = u32::from(!enabled);
            config.receive.dcs_code = 0o023;
            assert_eq!(create_result(&config), OK);
        }
    }

    #[test]
    fn public_callbacks_reject_invalid_spans_and_reentrant_calls() {
        let session = TestSession::new(&valid_config(), &SessionPorts::default());
        let input = [0.25; 34];
        let mut rx = [9.0; 17];
        let mut tx = [9.0; 34];
        let mut rx_result = ReceiveResult::default();
        let mut tx_result = TransmitResult::default();
        let rx_controls = ReceiveInput::default();
        let tx_controls = TransmitInput::default();
        unsafe {
            assert_eq!(
                receive(
                    ptr::null_mut(),
                    input.as_ptr(),
                    rx.as_mut_ptr(),
                    1,
                    &rx_controls,
                    &mut rx_result
                ),
                INVALID_ARGUMENT
            );
            assert_eq!(
                transmit(
                    ptr::null_mut(),
                    tx.as_mut_ptr(),
                    1,
                    &tx_controls,
                    &mut tx_result
                ),
                INVALID_ARGUMENT
            );
            for (output, result, count) in [
                (ptr::null_mut(), &mut rx_result as *mut _, 1),
                (rx.as_mut_ptr(), ptr::null_mut(), 1),
                (rx.as_mut_ptr(), &mut rx_result as *mut _, 0),
            ] {
                assert_eq!(
                    receive(
                        session.0,
                        input.as_ptr(),
                        output,
                        count,
                        &rx_controls,
                        result
                    ),
                    INVALID_ARGUMENT
                );
            }
            assert_eq!(
                receive(
                    session.0,
                    ptr::null(),
                    rx.as_mut_ptr(),
                    1,
                    &rx_controls,
                    &mut rx_result
                ),
                INVALID_ARGUMENT
            );
            assert_eq!(
                receive(
                    session.0,
                    input.as_ptr(),
                    rx.as_mut_ptr(),
                    1,
                    ptr::null(),
                    &mut rx_result
                ),
                INVALID_ARGUMENT
            );
            for (output, controls, result, count) in [
                (
                    ptr::null_mut(),
                    &tx_controls as *const _,
                    &mut tx_result as *mut _,
                    1,
                ),
                (tx.as_mut_ptr(), &tx_controls, ptr::null_mut(), 1),
                (tx.as_mut_ptr(), ptr::null(), &mut tx_result, 1),
                (tx.as_mut_ptr(), &tx_controls, &mut tx_result, 0),
            ] {
                assert_eq!(
                    transmit(session.0, output, count, controls, result),
                    INVALID_ARGUMENT
                );
            }
            assert_eq!(session.receive(false).0, NOT_READY);
            assert_eq!(session.transmit(false).0, NOT_READY);
            assert_eq!(warm(ptr::null_mut()), INVALID_ARGUMENT);
            (*session.0).receive_busy.store(true, Ordering::Relaxed);
            assert_eq!(warm(session.0), BUSY);
            (*session.0).receive_busy.store(false, Ordering::Relaxed);
            (*session.0).transmit_busy.store(true, Ordering::Relaxed);
            assert_eq!(warm(session.0), BUSY);
            assert!(!(*session.0).receive_busy.load(Ordering::Relaxed));
            (*session.0).transmit_busy.store(false, Ordering::Relaxed);
            assert_eq!(warm(session.0), OK);
            assert_eq!(warm(session.0), OK);
            assert_eq!(
                receive(
                    session.0,
                    input.as_ptr(),
                    rx.as_mut_ptr(),
                    17,
                    &rx_controls,
                    &mut rx_result
                ),
                FRAME_COUNT_EXCEEDED
            );
            assert_eq!(
                transmit(session.0, tx.as_mut_ptr(), 17, &tx_controls, &mut tx_result),
                FRAME_COUNT_EXCEEDED
            );
            let bad_input = [f32::NAN; 2];
            assert_eq!(
                receive(
                    session.0,
                    bad_input.as_ptr(),
                    rx.as_mut_ptr(),
                    1,
                    &rx_controls,
                    &mut rx_result
                ),
                INVALID_ARGUMENT
            );
            (*session.0).receive_busy.store(true, Ordering::Relaxed);
            assert_eq!(session.receive(false).0, BUSY);
            (*session.0).receive_busy.store(false, Ordering::Relaxed);
            (*session.0).transmit_busy.store(true, Ordering::Relaxed);
            assert_eq!(session.transmit(false).0, BUSY);
            (*session.0).transmit_busy.store(false, Ordering::Relaxed);
            let invalid_controls = TransmitInput {
                forced_ctcss_tenths_hz: 1,
                ..tx_controls
            };
            assert_eq!(
                transmit(
                    session.0,
                    tx.as_mut_ptr(),
                    1,
                    &invalid_controls,
                    &mut tx_result
                ),
                INVALID_ARGUMENT
            );
        }
        assert_eq!(session.receive(true).0, OK);
        assert_eq!(session.transmit(true).0, OK);
    }

    #[test]
    fn warm_failures_are_retryable_for_every_processor_group() {
        for index in 0..9 {
            for warm_failure in [false, true] {
                let mut processor = TestProcessor {
                    fail: true,
                    ..TestProcessor::default()
                };
                let mut ports = SessionPorts::default();
                let port = ProcessorPort {
                    warm: warm_failure.then_some(test_warm),
                    ..test_port(&mut processor)
                };
                match index {
                    0 => ports.receive_deemphasis = port,
                    1 => ports.receive_filter = port,
                    2 => ports.receive_noise_reduction = port,
                    3 => ports.receive_dynamics = port,
                    4 => ports.receive_ctcss_notch[0] = port,
                    5 => ports.transmit_program = port,
                    6 => ports.transmit_dcs_normal_filter = port,
                    7 => ports.transmit_dcs_turnoff_filter = port,
                    8 => ports.receive_ctcss_tail_notch = port,
                    _ => unreachable!(),
                }
                let session = TestSession::new(&valid_config(), &ports);
                assert_eq!(unsafe { warm(session.0) }, PROVIDER_FAILED);
                processor.fail = false;
                assert_eq!(unsafe { warm(session.0) }, OK);
            }
        }
        let mut processor = TestProcessor {
            fail: true,
            ..TestProcessor::default()
        };
        let ports = SessionPorts {
            program_ring: ProgramRingPort {
                context: ptr::from_mut(&mut processor).cast(),
                warm: Some(test_warm),
                ..ProgramRingPort::default()
            },
            ..SessionPorts::default()
        };
        let session = TestSession::new(&valid_config(), &ports);
        assert_eq!(unsafe { warm(session.0) }, PROVIDER_FAILED);
        processor.fail = false;
        assert_eq!(unsafe { warm(session.0) }, OK);
        assert_eq!(processor.calls, 2);
    }

    #[test]
    fn each_callback_provider_failure_is_silent_observable_and_retryable() {
        for index in 0..7 {
            let mut processor = TestProcessor::default();
            let mut ports = SessionPorts::default();
            let port = test_port(&mut processor);
            match index {
                0 => ports.receive_deemphasis = port,
                1 => ports.receive_filter = port,
                2 => ports.receive_noise_reduction = port,
                3 => ports.receive_dynamics = port,
                4 => ports.transmit_program = port,
                5 => ports.transmit_dcs_normal_filter = port,
                6 => ports.transmit_dcs_turnoff_filter = port,
                _ => unreachable!(),
            }
            let session = TestSession::prepared(&valid_config(), &ports);
            processor.fail = true;
            if index < 4 {
                let (status, output, _) = session.receive(true);
                assert_eq!((status, output), (PROVIDER_FAILED, [0.0; 4]));
            } else {
                let (status, output, _) = session.transmit(true);
                assert_eq!((status, output), (PROVIDER_FAILED, [0.0; 8]));
            }
            assert_eq!(session.snapshot().provider_failures, 1);
            let mut event = Event::default();
            let pop = if index < 4 {
                pop_receive_event
            } else {
                pop_transmit_event
            };
            assert_eq!(unsafe { pop(session.0, &mut event) }, 1);
            assert_eq!(event.kind, EventKind::ProviderFailure as u32);
            processor.fail = false;
            assert_eq!(
                if index < 4 {
                    session.receive(true).0
                } else {
                    session.transmit(true).0
                },
                OK
            );
        }
        let mut processor = TestProcessor {
            bias: f32::NAN,
            ..TestProcessor::default()
        };
        let mut output = [0.0; 2];
        assert_eq!(
            unsafe { run_processor(test_port(&mut processor), &[0.0; 2], &mut output) },
            Err(PROVIDER_FAILED)
        );
        assert_eq!(output, [0.0; 2]);
    }

    #[test]
    fn ring_validation_and_public_observers_preserve_actual_values() {
        let mut ring = TestRing {
            sample: 0.25,
            ..TestRing::default()
        };
        ring.result.observation = RingObservation {
            occupancy_frames: 120,
            reserve_frames: 20,
            target_frames: 60,
            capacity_frames: 240,
            ratio: 1.00001,
            underrun_samples: 3,
            overrun_samples: 4,
            concealment_samples: 5,
        };
        let ports = SessionPorts {
            program_ring: ring_port(&mut ring),
            ..SessionPorts::default()
        };
        let session = TestSession::prepared(&valid_config(), &ports);
        let (status, output, result) = session.transmit(true);
        assert_eq!(status, OK);
        assert_eq!(output, [0.25; 8]);
        assert_eq!(result.program_ring, ring.result.observation);
        assert_eq!(session.snapshot().program_ring, ring.result.observation);
        assert_eq!(session.receive(true).0, OK);
        let mut event = Event::default();
        unsafe {
            assert_eq!(
                snapshot(ptr::null(), &mut Snapshot::default()),
                INVALID_ARGUMENT
            );
            assert_eq!(snapshot(session.0, ptr::null_mut()), INVALID_ARGUMENT);
            for pop in [pop_receive_event, pop_transmit_event] {
                assert_eq!(pop(ptr::null(), &mut event), 0);
                assert_eq!(pop(session.0, ptr::null_mut()), 0);
                assert_eq!(pop(session.0, &mut event), 1);
                while pop(session.0, &mut event) != 0 {}
            }
        }
        for invalid in 0..6 {
            ring.fail = invalid == 0;
            ring.sample = if invalid == 1 { f32::NAN } else { 0.25 };
            ring.result.receiver_keyed = if invalid == 2 { 2 } else { 0 };
            ring.result.dcs_valid = if invalid == 3 { 2 } else { 0 };
            ring.result.ctcss_decoded_index = match invalid {
                4 => -2,
                5 => crate::ctcss_receive::TONE_COUNT as i32,
                _ => -1,
            };
            let (status, output, _) = session.transmit(true);
            assert_eq!((status, output), (PROVIDER_FAILED, [0.0; 8]));
        }
        assert_eq!(session.snapshot().provider_failures, 6);
    }

    #[test]
    fn meters_routes_delay_and_idle_noise_bypass_use_native_pcm() {
        assert_eq!(measure(&[]), (0.0, 0.0, 0));
        assert_eq!(measure_channel(&[], 0), (0.0, 0.0, 0));
        assert_eq!(measure_channel(&[1.0, 0.0, -1.0, 0.0], 0), (1.0, 1.0, 2));
        assert_eq!(
            route_sample(OutputRoute::Composite, 0.2, 0.1, 0.2, 1.0, 0.0),
            0.5
        );
        assert_eq!(
            route_sample(OutputRoute::AuxiliaryVoice, 0.2, 0.1, 0.2, 1.0, 0.0),
            0.2
        );
        let mut config = valid_config();
        config.receive.native_squelch_delay_frames = 2;
        config.receive.cpu_saver_enabled = 1;
        let mut processor = TestProcessor::default();
        let ports = SessionPorts {
            receive_noise_reduction: ProcessorPort {
                context: ptr::from_mut(&mut processor).cast(),
                bypass: Some(test_warm),
                ..ProcessorPort::default()
            },
            ..SessionPorts::default()
        };
        let session = TestSession::prepared(&config, &ports);
        assert_eq!(session.receive(true).1, [0.0, 0.0, 0.25, 0.25]);
        assert_eq!(session.receive(false).1, [0.0; 4]);
        assert_eq!(processor.calls, 2);
        processor.fail = true;
        assert_eq!(session.receive(false).0, PROVIDER_FAILED);
        assert_eq!(session.receive(true).0, PROVIDER_FAILED);
        assert_eq!(session.snapshot().provider_failures, 2);
    }

    #[test]
    fn native_decode_selects_the_notch_before_dynamics_and_while_idle() {
        let mut config = valid_config();
        config.maximum_receive_frame_count = 960;
        config.receive.ctcss_enabled = 1;
        config.receive.ctcss_tone_mask = 1 << 11; // 100.0 Hz decoder.
        config.receive.cpu_saver_enabled = 1;
        config.qualification.subaudible_source = 1;
        let mut notch = TestProcessor::default();
        let mut ports = SessionPorts::default();
        ports.receive_ctcss_notch[11] = test_port(&mut notch);
        let session = TestSession::prepared(&config, &ports);
        let input: Vec<f32> = (0..960)
            .flat_map(|index| {
                let sample = (std::f32::consts::TAU * 100.0 * index as f32 / 48_000.0).sin() * 0.2;
                [sample, sample]
            })
            .collect();
        let mut output = [0.0; 960];
        let mut result = ReceiveResult::default();
        let mut controls = ReceiveInput {
            hardware_carrier: 1,
            hardware_subaudible: 1,
            ..ReceiveInput::default()
        };
        for _ in 0..100 {
            assert_eq!(
                unsafe {
                    receive(
                        session.0,
                        input.as_ptr(),
                        output.as_mut_ptr(),
                        960,
                        &controls,
                        &mut result,
                    )
                },
                OK
            );
            if result.ctcss_decoded_index == 11 {
                break;
            }
        }
        assert_eq!(result.ctcss_decoded_index, 11);
        assert!(notch.calls > 1);
        let calls = notch.calls;
        controls.hardware_subaudible = 0;
        assert_eq!(
            unsafe {
                receive(
                    session.0,
                    input.as_ptr(),
                    output.as_mut_ptr(),
                    960,
                    &controls,
                    &mut result,
                )
            },
            OK
        );
        assert_eq!(result.receiver_keyed, 0);
        assert_eq!(result.ctcss_decoded_index, 11);
        assert_eq!(notch.calls, calls + 1);
        controls.hardware_carrier = 0;
        assert_eq!(
            unsafe {
                receive(
                    session.0,
                    input.as_ptr(),
                    output.as_mut_ptr(),
                    960,
                    &controls,
                    &mut result,
                )
            },
            OK
        );
        assert_eq!(result.ctcss_decoded_index, -1);
        assert!(output.iter().all(|sample| *sample == 0.0));
    }

    #[test]
    fn dsp_noise_gate_and_dcs_mode_process_native_spans() {
        let mut config = valid_config();
        config.maximum_receive_frame_count = 960;
        config.qualification.carrier_source = 1;
        config.receive.squelch_open_level = 2_000;
        let session = TestSession::prepared(&config, &SessionPorts::default());
        let mut input = [0.0; 1920];
        let mut output = [0.0; 960];
        let mut result = ReceiveResult::default();
        let controls = ReceiveInput::default();
        let mut saw_open = false;
        let mut saw_closed = false;
        for block in 0..100 {
            for index in 0..960 {
                let sample = if block < 50 {
                    0.0
                } else {
                    (std::f32::consts::TAU * 8_000.0 * index as f32 / 48_000.0).sin() * 0.8
                };
                input[index * 2] = sample;
                input[index * 2 + 1] = sample;
            }
            assert_eq!(
                unsafe {
                    receive(
                        session.0,
                        input.as_ptr(),
                        output.as_mut_ptr(),
                        960,
                        &controls,
                        &mut result,
                    )
                },
                OK
            );
            let gates = unsafe { (*(*session.0).receive.get()).detector.carrier_gates() };
            saw_open |= gates.contains(&1);
            saw_closed |= gates.contains(&0);
        }
        assert!(saw_open && saw_closed);

        config.receive.dcs_enabled = 1;
        config.receive.dcs_code = 0o023;
        let dcs = TestSession::prepared(&config, &SessionPorts::default());
        assert_eq!(dcs.receive(false).0, OK);
    }

    #[test]
    fn tone_tail_finishes_then_arms_receiver_blanking_and_unkeys() {
        let mut config = valid_config();
        config.maximum_transmit_frame_count = 960;
        config.transmit.ctcss_transmit_enabled = 1;
        config.transmit.default_ctcss_frequency_tenths_hz = 1_000;
        config.transmit.ctcss_peak = 0.1;
        config.transmit.output_a_route = 3;
        config.transmit.output_b_route = 4;
        config.transmit.output_a_tone_gain = 1.0;
        config.transmit.tone_off_mode = 3;
        config.transmit.ctcss_turnoff_duration_ms = 20;
        config.transmit.ctcss_turnoff_tail_tone_hz = 55.0;
        config.transmit.receiver_blanking_ms = 20;
        let session = TestSession::prepared(&config, &SessionPorts::default());
        assert_eq!(session.transmit(false).1, [0.0; 8]);
        let mut output = [0.0; 1920];
        let mut result = TransmitResult::default();
        let mut controls = TransmitInput {
            external_ptt_request: 1,
            physical_ptt_applied: 1,
            render_admitted: 1,
            ..TransmitInput::default()
        };
        assert_eq!(
            unsafe { transmit(session.0, output.as_mut_ptr(), 960, &controls, &mut result) },
            OK
        );
        assert_eq!(result.logical_ptt, 1);
        assert_eq!(result.selected_ctcss_tenths_hz, 1_000);
        assert!(output.iter().step_by(2).any(|sample| *sample != 0.0));
        controls.external_ptt_request = 0;
        for _ in 0..20 {
            assert_eq!(
                unsafe { transmit(session.0, output.as_mut_ptr(), 960, &controls, &mut result) },
                OK
            );
            if result.logical_ptt == 0 {
                break;
            }
        }
        assert_eq!(result.logical_ptt, 0);
        let mut event = Event::default();
        let mut saw_blanking = false;
        while unsafe { pop_transmit_event(session.0, &mut event) } != 0 {
            saw_blanking |= event.kind == EventKind::ReceiverBlanking as u32;
        }
        assert!(saw_blanking);
        assert_eq!(session.receive(true).0, OK);
        assert_eq!(
            unsafe {
                (*session.0)
                    .shared
                    .receiver_blanking_ms
                    .load(Ordering::Relaxed)
            },
            0
        );
        assert!(matches!(OutputRoute::parse(0), Ok(OutputRoute::Disabled)));
    }
}
