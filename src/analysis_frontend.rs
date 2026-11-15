//! Prepared analysis-only preprocessing through released neutral adapters.
//!
//! A front end owns one continuous source generation. Carrier admission is a
//! separate classifier policy: closed spans still advance this state. Creation
//! and destruction belong to the control owner; no full graph reset is implied.

use crate::samplerate_client::Client;
use crate::session::{PROVIDER_FAILED, UNSUPPORTED};
use crate::vad::PreprocessSettings;
use std::ffi::{CStr, c_char, c_void};
use std::mem::size_of;
use std::ptr::{self, NonNull};

const FRAME: usize = 480;
const MAP_CAPACITY: usize = 4096;
const BILLION: i128 = 1_000_000_000;
const RN_DELAY: i128 = 960;
// Experimental smallest candidate; qualify against513 taps before release.
const BANDPASS_TAPS: usize = 255;

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

/// Prepared symmetric Q15 bandpass through the existing Radio FIR primitive.
/// The nominal edges use the windowed-sinc half-amplitude (-6dB) convention.
struct Bandpass {
    coefficients: Box<[i16]>,
    history: Box<[i16]>,
}
impl Bandpass {
    fn new(settings: PreprocessSettings) -> Result<Self, i32> {
        Self::with_taps(settings, BANDPASS_TAPS)
    }
    /// Setup-only experimental recipes; no user-configurable unbounded tap count.
    fn with_taps(settings: PreprocessSettings, taps: usize) -> Result<Self, i32> {
        use crate::session::INVALID_ARGUMENT;
        if !matches!(taps, 255 | 513) {
            return Err(INVALID_ARGUMENT);
        }
        PreprocessSettings {
            enabled: true,
            bandpass_enabled: true,
            ..settings
        }
        .validate(48000)?;
        let low = f64::from(settings.low_hz) / 48000.0;
        let high = f64::from(settings.high_hz) / 48000.0;
        let middle = (taps - 1) / 2;
        let sinc = |value: f64| {
            if value == 0.0 {
                1.0
            } else {
                (std::f64::consts::PI * value).sin() / (std::f64::consts::PI * value)
            }
        };
        let mut kernel: Vec<f64> = (0..taps)
            .map(|index| {
                let offset = index as f64 - middle as f64;
                let window =
                    0.54 - 0.46 * (std::f64::consts::TAU * index as f64 / (taps - 1) as f64).cos();
                (2.0 * high * sinc(2.0 * high * offset) - 2.0 * low * sinc(2.0 * low * offset))
                    * window
            })
            .collect();
        // Preserve exact symmetry after floating-point calculation and Q15 rounding.
        for index in 0..middle {
            kernel[taps - 1 - index] = kernel[index];
        }
        // Normalize in the passband at the geometric center, before quantization.
        let center = (low * high).sqrt();
        let gain: f64 = kernel
            .iter()
            .enumerate()
            .map(|(index, tap)| {
                tap * (std::f64::consts::TAU * center * (index as f64 - middle as f64)).cos()
            })
            .sum();
        if !gain.is_finite() || gain <= f64::EPSILON {
            return Err(INVALID_ARGUMENT);
        }
        let coefficients: Result<Vec<i16>, i32> = kernel
            .iter()
            .map(|tap| {
                let value = (tap / gain * 32768.0).round();
                if !value.is_finite() || value < f64::from(i16::MIN) || value > f64::from(i16::MAX)
                {
                    Err(INVALID_ARGUMENT)
                } else {
                    Ok(value as i16)
                }
            })
            .collect();
        Ok(Self {
            coefficients: coefficients?.into_boxed_slice(),
            history: vec![0; taps].into_boxed_slice(),
        })
    }
    fn delay_48k(&self) -> usize {
        (self.coefficients.len() - 1) / 2
    }
    fn process(&mut self, input: &[f32; FRAME], output: &mut [f32; FRAME]) -> Result<(), i32> {
        // SAFETY: This serial owner retains fixed preallocated history/taps, and
        // the exact caller PCM spans are disjoint from each other and that storage.
        unsafe {
            crate::fir::process(crate::fir::Request {
                input: input.as_ptr(),
                output: output.as_mut_ptr(),
                sample_count: FRAME as u32,
                history: self.history.as_mut_ptr(),
                history_count: self.coefficients.len() as u32,
                coefficients: self.coefficients.as_ptr(),
                input_gain: 256,
                output_gain: 256,
                calc_adjust: 32768,
            })
        }
    }
}

/// Tagged source coordinates, with the symmetric FIR delay and nominal RNNoise delay.
pub(crate) struct SourceMap {
    positions: Box<[i128]>,
    tags: Box<[u64]>,
    processing_delay: i128,
}
impl SourceMap {
    /// Map after the known symmetric FIR delay and nominal RNNoise latency.
    /// Initial output preceding this generation is unqualified, not a source sample.
    pub(crate) fn source_position(&self, position: i128) -> Result<Option<u64>, i32> {
        let position = position - self.processing_delay;
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
        let bandpass = if settings.bandpass_enabled {
            Some(Bandpass::new(settings)?)
        } else {
            None
        };
        let processing_delay = bandpass
            .as_ref()
            .map_or(0, |filter| filter.delay_48k() as i128)
            + if settings.rnnoise_enabled {
                RN_DELAY
            } else {
                0
            };
        Ok(Self {
            up: if source_rate == 48000 {
                None
            } else {
                Some(Client::to_48k(source_rate, maximum_frames)?)
            },
            bandpass,
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
                processing_delay,
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

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod allocation_tests {
    use super::*;
    use crate::vad::{SpeechVad, tests::no_callback_allocations};

    // These are modest experimental voice-analysis checks, not user-mandated
    // rejection specifications or a claim that either recipe improves VAD.
    fn response_db(coefficients: &[i16], frequency: f64) -> f64 {
        let mut real = 0.0;
        let mut imaginary = 0.0;
        for (index, coefficient) in coefficients.iter().enumerate() {
            let phase = std::f64::consts::TAU * frequency * index as f64 / 48000.0;
            let gain = f64::from(*coefficient) / 32768.0;
            real += gain * phase.cos();
            imaginary -= gain * phase.sin();
        }
        20.0 * real.hypot(imaginary).log10()
    }

    #[test]
    fn bounded_fir_bandpass_recipes_preserve_input_and_have_declared_response() {
        for taps in [255, 513] {
            let mut stage = Bandpass::with_taps(PreprocessSettings::default(), taps).unwrap();
            assert_eq!(stage.coefficients.len(), taps);
            assert_eq!(stage.delay_48k(), (taps - 1) / 2);
            assert!(
                stage
                    .coefficients
                    .iter()
                    .eq(stage.coefficients.iter().rev())
            );
            for frequency in [300.0, 3000.0] {
                assert!((response_db(&stage.coefficients, frequency) + 6.0).abs() < 0.2);
            }
            for frequency in [500.0, 1000.0, 2000.0, 2800.0] {
                assert!(response_db(&stage.coefficients, frequency).abs() < 1.0);
            }
            for frequency in [100.0, 3300.0] {
                assert!(response_db(&stage.coefficients, frequency) < -20.0);
            }
            let mut input = [0.0_f32; FRAME];
            input[0] = 0.5;
            let mut output = [0.0_f32; FRAME];
            no_callback_allocations(|| stage.process(&input, &mut output).unwrap());
            let peak = output
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .unwrap()
                .0;
            assert_eq!(peak, stage.delay_48k());
            assert_eq!(input[0].to_bits(), 0.5_f32.to_bits());
            assert!(input[1..].iter().all(|sample| sample.to_bits() == 0));
        }
    }

    #[test]
    fn bounded_fir_bandpass_mapping_accounts_for_linear_phase_without_old_samples() {
        let settings = PreprocessSettings {
            enabled: true,
            rnnoise_enabled: false,
            ..PreprocessSettings::default()
        };
        let delay = Bandpass::new(settings).unwrap().delay_48k();
        let mut front = FrontEnd::new(48000, FRAME, settings).unwrap();
        let mut input = [0.0_f32; FRAME];
        input[0] = 0.5;
        let mut frames = 0;
        front
            .process(&input, 500, |output, first, map| {
                assert_eq!(first, 0);
                for position in 0..delay {
                    assert_eq!(map.source_position(position as i128)?, None);
                }
                assert_eq!(map.source_position(delay as i128)?, Some(500));
                let peak = output
                    .iter()
                    .enumerate()
                    .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                    .unwrap()
                    .0;
                assert_eq!(peak, delay);
                frames += 1;
                Ok(())
            })
            .unwrap();
        assert_eq!(frames, 1);
        assert_eq!(input[0], 0.5);
    }

    #[test]
    fn bounded_fir_bandpass_rejects_invalid_setup_without_fallback() {
        for (low, high) in [
            (0.0, 3000.0),
            (f32::NAN, 3000.0),
            (300.0, 24000.0),
            (3000.0, 300.0),
        ] {
            let settings = PreprocessSettings {
                low_hz: low,
                high_hz: high,
                ..PreprocessSettings::default()
            };
            assert!(Bandpass::with_taps(settings, 255).is_err());
        }
        for taps in [0, 1, 254, 514] {
            assert!(Bandpass::with_taps(PreprocessSettings::default(), taps).is_err());
        }
    }

    fn current_thread_cpu_ticks() -> Option<u64> {
        let stat = std::fs::read_to_string("/proc/thread-self/stat").ok()?;
        let (_, fields) = stat.rsplit_once(')')?;
        let mut fields = fields.split_whitespace();
        let user: u64 = fields.nth(11)?.parse().ok()?;
        let system: u64 = fields.next()?.parse().ok()?;
        user.checked_add(system)
    }

    #[test]
    #[ignore = "short optimized processing comparison; run explicitly with release profile"]
    fn bounded_fir_optimized_processing_comparison() {
        assert!(
            !cfg!(debug_assertions),
            "CPU comparison requires optimized code"
        );
        let input: [f32; FRAME] = std::array::from_fn(|index| {
            (std::f64::consts::TAU * 1000.0 * index as f64 / 48000.0).sin() as f32 * 0.125
        });
        for taps in [255, 513] {
            let mut stage = Bandpass::with_taps(PreprocessSettings::default(), taps).unwrap();
            let mut output = [0.0_f32; FRAME];
            for _ in 0..4 {
                stage.process(&input, &mut output).unwrap();
            }
            let cpu_before = current_thread_cpu_ticks();
            let start = std::time::Instant::now();
            no_callback_allocations(|| {
                for _ in 0..1000 {
                    stage
                        .process(std::hint::black_box(&input), &mut output)
                        .unwrap();
                    std::hint::black_box(&output);
                }
            });
            let elapsed = start.elapsed().as_secs_f64();
            let ticks = cpu_before
                .zip(current_thread_cpu_ticks())
                .map(|(start, end)| end.saturating_sub(start));
            eprintln!(
                "FIR_BENCH taps={taps} audio_seconds=10 wall_seconds={elapsed:.9} cpu_ticks={ticks:?} tap_history_bytes={} delay_samples_48k={}",
                taps * 4,
                stage.delay_48k()
            );
            for frequency in [
                0.0, 50.0, 100.0, 150.0, 200.0, 300.0, 400.0, 500.0, 1000.0, 2000.0, 2800.0,
                2900.0, 3000.0, 3100.0, 3300.0, 3500.0,
            ] {
                eprintln!(
                    "FIR_RESPONSE taps={taps} frequency_hz={frequency} gain_db={:.6}",
                    response_db(&stage.coefficients, frequency)
                );
            }
        }
    }

    #[test]
    fn native_allocation_matched_src_only() {
        let settings = PreprocessSettings {
            enabled: true,
            bandpass_enabled: false,
            rnnoise_enabled: false,
            ..PreprocessSettings::default()
        };
        // Explicitly compose both converters: the ordinary public constructor
        // correctly bypasses the frontend when no preprocessing stage is selected.
        let mut front = FrontEnd::new(8000, 960, settings).unwrap();
        let mut classifier = SpeechVad::configured(48000, 2, 1000, FRAME, 0.75, 2).unwrap();
        let input = [0.0; 127];
        let mut frames_48k = 0;
        eprintln!("matched8→48→16k SRC: preparation complete; arming processing only");
        no_callback_allocations(|| {
            for block in 0..20 {
                front
                    .process(&input, 500 + block * 127, |frame, first, _| {
                        assert_eq!(first, frames_48k);
                        // Keep classifier admission open so its 48→16 kHz SRC runs;
                        // plain closed-carrier handling intentionally skips that stage.
                        classifier.process(frame, true, first)?;
                        frames_48k += frame.len() as u64;
                        Ok(())
                    })
                    .unwrap();
            }
        });
        assert_eq!(front.accepted_source, 2540);
        assert!(frames_48k > 2540);
        eprintln!("matched SRC: processing complete; control-owned destruction unarmed");
        drop(classifier);
        drop(front);
    }

    #[test]
    fn native_allocation_rnnoise_only() {
        let mut stage = Denoiser::new().unwrap();
        let input = [0.0; FRAME];
        let mut output = [0.0_f32; FRAME];
        for _ in 0..2 {
            stage.process(&input, &mut output).unwrap();
        }
        eprintln!("RNNoise: preparation/warmup complete; arming one 480-frame process only");
        no_callback_allocations(|| stage.process(&input, &mut output).unwrap());
        eprintln!("RNNoise: processing complete; control-owned destruction unarmed");
        drop(stage);
    }

    #[test]
    fn native_allocation_bandpass_only() {
        let mut stage = Bandpass::new(PreprocessSettings::default()).unwrap();
        let input = [0.0; FRAME];
        let mut output = [0.0_f32; FRAME];
        eprintln!("bandpass: preparation/warmup complete; arming one 480-frame process only");
        no_callback_allocations(|| stage.process(&input, &mut output).unwrap());
        eprintln!("bandpass: processing complete; control-owned destruction unarmed");
        drop(stage);
    }
}
