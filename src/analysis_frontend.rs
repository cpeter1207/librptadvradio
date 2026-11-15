//! Prepared analysis-only preprocessing through released neutral adapters.
//!
//! A front end owns one continuous source generation. Carrier admission is a
//! separate classifier policy: closed spans still advance this state. Creation
//! and destruction belong to the control owner; no full graph reset is implied.

use crate::samplerate_client::Client;
use crate::session::{PROVIDER_FAILED, UNSUPPORTED};
use crate::vad::PreprocessSettings;
use std::ffi::{CStr, CString, c_char, c_void};
use std::mem::size_of;
use std::ptr::{self, NonNull};

const FRAME: usize = 480;
const MAP_CAPACITY: usize = 4096;
const BILLION: i128 = 1_000_000_000;
const RN_DELAY: i128 = 960;

#[repr(C)]
struct Prefix {
    size: u32,
    version: u32,
    name: *const c_char,
}
/// Validate the common neutral descriptor prefix before reading its tail.
unsafe fn check_descriptor<T>(pointer: *const T, name: &[u8]) -> Result<(), i32> {
    // SAFETY: A nonnull provider descriptor promises its readable common prefix.
    let prefix = unsafe { pointer.cast::<Prefix>().as_ref() }.ok_or(UNSUPPORTED)?;
    if prefix.size < size_of::<T>() as u32
        || prefix.version != 1
        || prefix.name.is_null()
        // SAFETY: A nonnull capability name is a provider-owned terminated string.
        || unsafe { CStr::from_ptr(prefix.name) }.to_bytes() != name
    {
        return Err(UNSUPPORTED);
    }
    Ok(())
}

type Destroy = unsafe extern "C" fn(*mut c_void);
type RnProcess = unsafe extern "C" fn(*mut c_void, *const f32, u32, *mut f32, *mut f32) -> i32;
#[repr(C)]
struct RnDescriptor {
    _prefix: Prefix,
    create: Option<unsafe extern "C" fn(u32, u32, u32, *mut *mut c_void) -> i32>,
    process: Option<RnProcess>,
    destroy: Option<Destroy>,
}
#[link(
    name = "librptadv_rnnoise_adapter.so.1",
    kind = "dylib",
    modifiers = "+verbatim"
)]
unsafe extern "C" {
    fn rptadv_rnnoise_adapter_descriptor() -> *const RnDescriptor;
}
struct Denoiser {
    handle: NonNull<c_void>,
    process: RnProcess,
    destroy: Destroy,
}
impl Denoiser {
    fn new() -> Result<Self, i32> {
        // SAFETY: The released adapter owns this immutable process-lifetime descriptor.
        let pointer = unsafe { rptadv_rnnoise_adapter_descriptor() };
        // SAFETY: The linked provider supplies the descriptor prefix.
        unsafe { check_descriptor(pointer, b"rptadv.rnnoise") }?;
        // SAFETY: The size and capability check covers this complete descriptor.
        let api = unsafe { &*pointer };
        let (Some(create), Some(process), Some(destroy)) = (api.create, api.process, api.destroy)
        else {
            return Err(UNSUPPORTED);
        };
        let mut raw = ptr::null_mut();
        // SAFETY: The output is writable and RNNoise's exact fixed format is requested.
        let result = unsafe { create(48000, 1, FRAME as u32, &mut raw) };
        if result != 0 {
            if !raw.is_null() {
                // SAFETY: A failed setup cannot transfer this unexpected owned handle.
                unsafe { destroy(raw) };
            }
            return Err(PROVIDER_FAILED);
        }
        Ok(Self {
            handle: NonNull::new(raw).ok_or(PROVIDER_FAILED)?,
            process,
            destroy,
        })
    }
    fn process(&mut self, input: &[f32; FRAME], output: &mut [f32; FRAME]) -> Result<(), i32> {
        let mut probability = 0.0;
        // SAFETY: A serial owner supplies disjoint fixed-size normalized mono frames.
        let result = unsafe {
            (self.process)(
                self.handle.as_ptr(),
                input.as_ptr(),
                FRAME as u32,
                output.as_mut_ptr(),
                &mut probability,
            )
        };
        if result != 0
            || !probability.is_finite()
            || !(0.0..=1.0).contains(&probability)
            || output.iter().any(|sample| !sample.is_finite())
        {
            return Err(PROVIDER_FAILED);
        }
        Ok(())
    }
}
impl Drop for Denoiser {
    fn drop(&mut self) {
        // SAFETY: The stopped source generation returns its unique handle on control.
        unsafe { (self.destroy)(self.handle.as_ptr()) };
    }
}

#[repr(C)]
struct GraphConfig {
    size: u32,
    version: u32,
    rate: u32,
    maximum: u32,
    description: *const c_char,
}
#[repr(C)]
struct GraphCounters {
    size: u32,
    version: u32,
    flags: u32,
    reserved: u32,
    accepted: u64,
    delivered: u64,
    queued: u64,
    prefix: u32,
    suffix: u32,
}
type GraphStream =
    unsafe extern "C" fn(*mut c_void, *const f32, u32, *mut f32, u32, *mut u32, *mut u32) -> i32;
type GraphBlock = unsafe extern "C" fn(*mut c_void, *const f32, u32, *mut f32) -> i32;
type GraphCounts = unsafe extern "C" fn(*const c_void, *mut GraphCounters) -> i32;
#[repr(C)]
struct GraphDescriptor {
    _prefix: Prefix,
    create: Option<unsafe extern "C" fn(*const GraphConfig, *mut *mut c_void) -> i32>,
    _stream: Option<GraphStream>,
    destroy: Option<Destroy>,
    block: Option<GraphBlock>,
    counters: Option<GraphCounts>,
}
#[link(
    name = "librptadv_ffmpeg_adapter.so.1",
    kind = "dylib",
    modifiers = "+verbatim"
)]
unsafe extern "C" {
    fn rptadv_ffmpeg_adapter_descriptor() -> *const GraphDescriptor;
}
struct Bandpass {
    handle: NonNull<c_void>,
    process: GraphBlock,
    counts: GraphCounts,
    destroy: Destroy,
}
impl Bandpass {
    fn new(settings: PreprocessSettings) -> Result<Self, i32> {
        // SAFETY: The released adapter owns this immutable process-lifetime descriptor.
        let pointer = unsafe { rptadv_ffmpeg_adapter_descriptor() };
        // SAFETY: The linked provider supplies the descriptor prefix.
        unsafe { check_descriptor(pointer, b"rptadv.ffmpeg") }?;
        // SAFETY: The validated descriptor span includes exact-block accounting.
        let api = unsafe { &*pointer };
        let (Some(create), Some(process), Some(counts), Some(destroy)) =
            (api.create, api.block, api.counters, api.destroy)
        else {
            return Err(UNSUPPORTED);
        };
        // Reuse the released local receive recipe, including its causal phase response.
        let description = CString::new(format!(
            "[in]acrossover=split={:.9}:order=20th[bplo][bppass];[bplo]anullsink;\
             [bppass]acrossover=split={:.9}:order=20th[out][bphi];[bphi]anullsink",
            settings.low_hz, settings.high_hz
        ))
        .map_err(|_| PROVIDER_FAILED)?;
        let config = GraphConfig {
            size: size_of::<GraphConfig>() as u32,
            version: 1,
            rate: 48000,
            maximum: FRAME as u32,
            description: description.as_ptr(),
        };
        let mut raw = ptr::null_mut();
        // SAFETY: Setup records and copied graph description outlive the create call.
        let result = unsafe { create(&config, &mut raw) };
        if result != 0 {
            if !raw.is_null() {
                // SAFETY: A failed setup cannot transfer this unexpected owned handle.
                unsafe { destroy(raw) };
            }
            return Err(PROVIDER_FAILED);
        }
        let mut filter = Self {
            handle: NonNull::new(raw).ok_or(PROVIDER_FAILED)?,
            process,
            counts,
            destroy,
        };
        for _ in 0..2 {
            filter.process(&[0.0; FRAME], &mut [0.0; FRAME])?;
        }
        Ok(filter)
    }
    fn process(&mut self, input: &[f32; FRAME], output: &mut [f32; FRAME]) -> Result<(), i32> {
        // SAFETY: One prepared exact block is processed serially into disjoint storage.
        let result = unsafe {
            (self.process)(
                self.handle.as_ptr(),
                input.as_ptr(),
                FRAME as u32,
                output.as_mut_ptr(),
            )
        };
        if result != 0 || output.iter().any(|sample| !sample.is_finite()) {
            return Err(PROVIDER_FAILED);
        }
        let mut counts = GraphCounters {
            size: size_of::<GraphCounters>() as u32,
            version: 1,
            flags: 0,
            reserved: 0,
            accepted: 0,
            delivered: 0,
            queued: 0,
            prefix: 0,
            suffix: 0,
        };
        // SAFETY: The immutable accounting snapshot has complete writable storage.
        if unsafe { (self.counts)(self.handle.as_ptr(), &mut counts) } != 0
            || counts.flags & 3 != 3
            || counts.prefix != 0
            || counts.suffix != 0
            || counts.queued != 0
            || counts.accepted != counts.delivered
        {
            return Err(PROVIDER_FAILED);
        }
        Ok(())
    }
}
impl Drop for Bandpass {
    fn drop(&mut self) {
        // SAFETY: The stopped source generation returns its unique handle on control.
        unsafe { (self.destroy)(self.handle.as_ptr()) };
    }
}

/// Tagged source coordinates; never infer a constant delay for the causal bandpass.
pub(crate) struct SourceMap {
    positions: Box<[i128]>,
    tags: Box<[u64]>,
    rn_delay: i128,
}
impl SourceMap {
    /// Map a 48 kHz analysis coordinate after removing only nominal RNNoise latency.
    /// Initial output preceding this generation is unqualified, not a source sample.
    pub(crate) fn source_position(&self, position: i128) -> Result<Option<u64>, i32> {
        let position = position - self.rn_delay;
        if position < 0 {
            return Ok(None);
        }
        let index = u64::try_from(position).map_err(|_| PROVIDER_FAILED)?;
        let slot = index as usize % MAP_CAPACITY;
        if self.tags[slot] != index {
            return Err(PROVIDER_FAILED);
        }
        let source = self.positions[slot].div_euclid(BILLION);
        if source < 0 {
            return Ok(None);
        }
        Ok(Some(u64::try_from(source).map_err(|_| PROVIDER_FAILED)?))
    }
}

/// Prepared source-rate conversion, causal bandpass and RNNoise framing.
pub(crate) struct FrontEnd {
    up: Option<Client>,
    bandpass: Option<Bandpass>,
    rn: Option<Denoiser>,
    frame: [f32; FRAME],
    filtered: [f32; FRAME],
    output: [f32; FRAME],
    used: usize,
    produced: u64,
    source_origin: Option<u64>,
    /// Actual accepted input frames, including closed-carrier spans.
    pub(crate) accepted_source: u64,
    map: SourceMap,
}
impl FrontEnd {
    /// Allocate, validate and warm all selected providers before source adoption.
    pub(crate) fn new(
        source_rate: u32,
        maximum_frames: usize,
        settings: PreprocessSettings,
    ) -> Result<Self, i32> {
        settings.validate(source_rate)?;
        Ok(Self {
            up: if source_rate == 48000 {
                None
            } else {
                Some(Client::to_48k(source_rate, maximum_frames)?)
            },
            bandpass: if settings.bandpass_enabled {
                Some(Bandpass::new(settings)?)
            } else {
                None
            },
            rn: if settings.rnnoise_enabled {
                Some(Denoiser::new()?)
            } else {
                None
            },
            frame: [0.0; FRAME],
            filtered: [0.0; FRAME],
            output: [0.0; FRAME],
            used: 0,
            produced: 0,
            source_origin: None,
            accepted_source: 0,
            map: SourceMap {
                positions: vec![0; MAP_CAPACITY].into_boxed_slice(),
                tags: vec![u64::MAX; MAP_CAPACITY].into_boxed_slice(),
                rn_delay: if settings.rnnoise_enabled {
                    RN_DELAY
                } else {
                    0
                },
            },
        })
    }
    /// Consume one contiguous source span; emit complete 48 kHz analysis blocks.
    pub(crate) fn process(
        &mut self,
        input: &[f32],
        first_source: u64,
        mut output: impl FnMut(&[f32], u64, &SourceMap) -> Result<(), i32>,
    ) -> Result<(), i32> {
        let origin = *self.source_origin.get_or_insert(first_source);
        if origin.checked_add(self.accepted_source) != Some(first_source) {
            return Err(PROVIDER_FAILED);
        }
        if self.up.is_none() {
            for (index, sample) in input.iter().enumerate() {
                self.accepted_source =
                    self.accepted_source.checked_add(1).ok_or(PROVIDER_FAILED)?;
                self.push_sample(
                    *sample,
                    (i128::from(first_source) + index as i128) * BILLION,
                    &mut output,
                )?;
            }
            return Ok(());
        }
        let mut consumed = 0;
        // 8 kHz is the smallest source rate: no more than six outputs per input.
        for _ in 0..input.len() * 6 + 128 {
            let step = self
                .up
                .as_mut()
                .ok_or(PROVIDER_FAILED)?
                .process_one(&input[consumed..])?;
            consumed += step.consumed;
            self.accepted_source = self
                .accepted_source
                .checked_add(step.consumed as u64)
                .ok_or(PROVIDER_FAILED)?;
            if let Some(sample) = step.sample {
                self.push_sample(
                    sample,
                    i128::from(origin) * BILLION + step.native_position_billionths,
                    &mut output,
                )?;
            } else if consumed == input.len() {
                return Ok(());
            } else if step.consumed == 0 {
                return Err(PROVIDER_FAILED);
            }
        }
        Err(PROVIDER_FAILED)
    }
    fn push_sample(
        &mut self,
        sample: f32,
        source_position: i128,
        output: &mut impl FnMut(&[f32], u64, &SourceMap) -> Result<(), i32>,
    ) -> Result<(), i32> {
        if !sample.is_finite() {
            return Err(PROVIDER_FAILED);
        }
        let slot = self.produced as usize % MAP_CAPACITY;
        self.map.positions[slot] = source_position;
        self.map.tags[slot] = self.produced;
        self.produced = self.produced.checked_add(1).ok_or(PROVIDER_FAILED)?;
        self.frame[self.used] = sample.clamp(-1.0, 1.0);
        self.used += 1;
        if self.used != FRAME {
            return Ok(());
        }
        if let Some(filter) = &mut self.bandpass {
            filter.process(&self.frame, &mut self.filtered)?;
        } else {
            self.filtered.copy_from_slice(&self.frame);
        }
        if let Some(rn) = &mut self.rn {
            rn.process(&self.filtered, &mut self.output)?;
        } else {
            self.output.copy_from_slice(&self.filtered);
        }
        self.used = 0;
        output(&self.output, self.produced - FRAME as u64, &self.map)
    }
}
