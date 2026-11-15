//! Prepared shared Earshot analysis, independent of radio or phone admission.

use crate::session::{FRAME_COUNT_EXCEEDED, INVALID_ARGUMENT};
use crate::vad::{PreprocessSettings, SpeechVad};
use std::mem::size_of;
use std::ptr::{self, NonNull};

/// Opaque classifier owned serially by one source generation.
pub struct SpeechAnalysis {
    vad: SpeechVad,
    maximum_frames: u32,
}

/// Sized per-source setup; caller owns programme retention and hangtime.
#[repr(C)]
pub struct AnalysisConfig {
    /// Readable size of this configuration.
    pub struct_size: u32,
    /// Fixed mono source sample rate, 8000 through 192000 Hz.
    pub source_rate_hz: u32,
    /// Maximum source frames per call, 1 through 4096.
    pub maximum_source_frames: u32,
    /// Finite inclusive speech-score threshold, 0 through 1.
    pub speech_threshold: f32,
    /// Positive consecutive complete 256-sample frames required for onset.
    pub speech_onset_frames: u32,
    /// Selection bits: 1 overall preprocessing, 2 bandpass, 4 RNNoise; other bits invalid.
    pub preprocess_flags: u32,
    /// Lower bandpass edge in Hz; selected filters require 0 < low < high.
    pub bandpass_low_hz: f32,
    /// Upper edge, strictly below both source Nyquist and the 48 kHz processing Nyquist.
    pub bandpass_high_hz: f32,
}

/// Sized C result; source positions use the input sample clock.
#[repr(C)]
pub struct AnalysisResult {
    /// Caller-provided writable size, preserved as the current result size.
    pub struct_size: u32,
    /// Latest complete analysis frame satisfies the configured onset qualification.
    pub voice_active: u32,
    /// Latest finite classifier score, zero before a complete frame.
    pub score: f32,
    /// Reserved for alignment; always zero.
    pub reserved: u32,
    /// Last positive source position, or UINT64_MAX before speech/reset.
    pub last_voice_source_sample: u64,
}

impl Default for AnalysisResult {
    fn default() -> Self {
        Self {
            struct_size: size_of::<Self>() as u32,
            voice_active: 0,
            score: 0.0,
            reserved: 0,
            last_voice_source_sample: u64::MAX,
        }
    }
}

/// Prepare analysis storage and model state off the source callback.
/// The output points to writable handle storage; success transfers ownership.
pub(crate) unsafe extern "C" fn create(
    source_rate_hz: u32,
    maximum_source_frames: u32,
    output: *mut *mut SpeechAnalysis,
) -> i32 {
    let config = AnalysisConfig {
        struct_size: size_of::<AnalysisConfig>() as u32,
        source_rate_hz,
        maximum_source_frames,
        speech_threshold: 0.5,
        speech_onset_frames: 1,
        preprocess_flags: 0,
        bandpass_low_hz: 300.0,
        bandpass_high_hz: 3000.0,
    };
    // SAFETY: Local configuration is complete; caller promises writable output storage.
    unsafe { create_configured(&config, output) }
}

/// Prepare configured analysis off audio; retains the legacy descriptor prefix.
/// A short/null configuration is rejected before accessing its unavailable tail.
pub(crate) unsafe extern "C" fn create_configured(
    config: *const AnalysisConfig,
    output: *mut *mut SpeechAnalysis,
) -> i32 {
    let Some(output) = NonNull::new(output) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: Setup promises writable handle storage, initialized on every path.
    unsafe { output.as_ptr().write(ptr::null_mut()) };
    if config.is_null()
        // SAFETY: A nonnull configuration promises a readable size prefix.
        || unsafe { config.cast::<u32>().read() } < size_of::<AnalysisConfig>() as u32
    {
        return INVALID_ARGUMENT;
    }
    // SAFETY: The checked prefix covers this complete immutable configuration.
    let config = unsafe { &*config };
    let preprocessing = match PreprocessSettings::from_flags(
        config.preprocess_flags,
        config.bandpass_low_hz,
        config.bandpass_high_hz,
    ) {
        Ok(settings) => settings,
        Err(error) => return error,
    };
    match SpeechVad::preprocessed(
        config.source_rate_hz,
        2,
        0,
        config.maximum_source_frames as usize,
        config.speech_threshold,
        config.speech_onset_frames,
        preprocessing,
    ) {
        Ok(vad) => {
            let handle = Box::new(SpeechAnalysis {
                vad,
                maximum_frames: config.maximum_source_frames,
            });
            // SAFETY: Setup transfers exactly one owned opaque handle.
            unsafe { output.as_ptr().write(Box::into_raw(handle)) };
            0
        }
        Err(error) => error,
    }
}

/// Analyze a finite, bounded mono source span without changing programme PCM.
/// Handle use is serial; input and sized output are valid disjoint storage.
pub(crate) unsafe extern "C" fn process(
    handle: *mut SpeechAnalysis,
    input: *const f32,
    frames: u32,
    first_source_sample: u64,
    output: *mut AnalysisResult,
) -> i32 {
    if output.is_null()
        // SAFETY: A nonnull output promises at least its readable size prefix.
        || unsafe { output.cast::<u32>().read() } < size_of::<AnalysisResult>() as u32
    {
        return INVALID_ARGUMENT;
    }
    // SAFETY: The checked size promises writable complete result storage.
    unsafe { output.write(AnalysisResult::default()) };
    let Some(mut handle) = NonNull::new(handle) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: The caller owns this live handle and serializes all its uses.
    let handle = unsafe { handle.as_mut() };
    if input.is_null() || frames == 0 {
        return INVALID_ARGUMENT;
    }
    if frames > handle.maximum_frames {
        return FRAME_COUNT_EXCEEDED;
    }
    // SAFETY: The caller provides this checked input span for the complete call.
    let samples = unsafe { std::slice::from_raw_parts(input, frames as usize) };
    match handle.vad.process(samples, true, first_source_sample) {
        Ok(decision) => {
            // SAFETY: This result storage was checked before processing.
            unsafe {
                (*output).voice_active = u32::from(decision.voice_active);
                (*output).score = decision.score;
                (*output).last_voice_source_sample =
                    decision.last_voice_sample_index.unwrap_or(u64::MAX);
            }
            0
        }
        Err(error) => error,
    }
}

/// Retire source history without allocation; serial use of a live handle is required.
/// Selected preprocessing returns UNSUPPORTED after closing admission and must be
/// replaced by a newly prepared control-owned generation, not reset in place.
pub(crate) unsafe extern "C" fn reset(handle: *mut SpeechAnalysis) -> i32 {
    let Some(mut handle) = NonNull::new(handle) else {
        return INVALID_ARGUMENT;
    };
    // SAFETY: The caller owns the live handle and has serialized processing.
    unsafe { handle.as_mut() }
        .vad
        .reset()
        .map_or_else(|error| error, |()| 0)
}

/// Release an owned stopped handle off audio; null is accepted.
pub(crate) unsafe extern "C" fn destroy(handle: *mut SpeechAnalysis) {
    if !handle.is_null() {
        // SAFETY: Ownership was returned exactly once after processing stopped.
        drop(unsafe { Box::from_raw(handle) });
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::*;

    #[test]
    fn preprocessed_descriptor_checks_capability_configuration_and_retirement() {
        let descriptor = unsafe { &*crate::rptadv_radio_descriptor() };
        assert_eq!(descriptor.speech_analysis_capabilities & 1, 1);
        let mut config = AnalysisConfig {
            struct_size: size_of::<AnalysisConfig>() as u32,
            source_rate_hz: 8000,
            maximum_source_frames: 160,
            speech_threshold: 0.75,
            speech_onset_frames: 2,
            preprocess_flags: 7,
            bandpass_low_hz: 300.0,
            bandpass_high_hz: 3000.0,
        };
        assert_eq!(size_of::<AnalysisConfig>(), 32);
        let mut handle = ptr::null_mut();
        // SAFETY: The complete local configuration and output storage are disjoint.
        unsafe {
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(&config, &mut handle),
                0
            );
            let samples = [0.0; 160];
            let mut result = AnalysisResult::default();
            for block in 0..8 {
                assert_eq!(
                    (descriptor.speech_analysis_process)(
                        handle,
                        samples.as_ptr(),
                        160,
                        block * 160,
                        &mut result
                    ),
                    0
                );
                assert_eq!(result.voice_active, 0);
            }
            // A graph cannot be fully reset in place. Retire without admitting
            // successor PCM; the caller prepares a replacement outside audio.
            assert_eq!(
                (descriptor.speech_analysis_reset)(handle),
                crate::session::UNSUPPORTED
            );
            assert_ne!(
                (descriptor.speech_analysis_process)(handle, samples.as_ptr(), 160, 0, &mut result),
                0
            );
            assert_eq!(result.last_voice_source_sample, u64::MAX);
            (descriptor.speech_analysis_destroy)(handle);
            for flags in [8, u32::MAX] {
                config.preprocess_flags = flags;
                handle = ptr::dangling_mut();
                assert_eq!(
                    (descriptor.speech_analysis_create_configured)(&config, &mut handle),
                    INVALID_ARGUMENT
                );
                assert!(handle.is_null());
            }
            config.preprocess_flags = 7;
            config.bandpass_high_hz = 4000.0;
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(&config, &mut handle),
                INVALID_ARGUMENT
            );
            assert!(handle.is_null());
            config.bandpass_high_hz = 3000.0;
            config.struct_size = 20;
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(&config, &mut handle),
                INVALID_ARGUMENT
            );
            assert!(handle.is_null());
        }
    }

    #[test]
    fn configured_descriptor_validates_sized_setup_and_clears_output() {
        let descriptor = unsafe { &*crate::rptadv_radio_descriptor() };
        let mut config = AnalysisConfig {
            struct_size: size_of::<AnalysisConfig>() as u32,
            source_rate_hz: 8000,
            maximum_source_frames: 128,
            speech_threshold: 0.75,
            speech_onset_frames: 4,
            preprocess_flags: 0,
            bandpass_low_hz: 300.0,
            bandpass_high_hz: 3000.0,
        };
        let mut handle = ptr::null_mut();
        // SAFETY: Local records cover each advertised readable/writable prefix.
        unsafe {
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(&config, ptr::null_mut()),
                INVALID_ARGUMENT
            );
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(ptr::null(), &mut handle),
                INVALID_ARGUMENT
            );
            assert!(handle.is_null());
            let short = 4_u32;
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(
                    (&short as *const u32).cast(),
                    &mut handle
                ),
                INVALID_ARGUMENT
            );
            config.speech_onset_frames = 0;
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(&config, &mut handle),
                INVALID_ARGUMENT
            );
            assert!(handle.is_null());
            config.speech_onset_frames = 4;
            assert_eq!(
                (descriptor.speech_analysis_create_configured)(&config, &mut handle),
                0
            );
            assert!(!handle.is_null());
            (descriptor.speech_analysis_destroy)(handle);
        }
    }
}
