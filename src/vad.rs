//! Local speech analysis and admission on the native receive sample clock.

/// Speech classification and burst-qualified receive admission after one span.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Decision {
    /// Whether the selected speech policy admits this raw-qualified burst.
    pub admitted: bool,
    /// Latest classifier score is at least 0.5 while raw qualification is open.
    pub voice_active: bool,
    /// Latest score, cleared when raw qualification closes.
    pub score: f32,
    /// Inclusive native source position of the last positive analysis frame.
    pub last_voice_sample_index: Option<u64>,
    /// Nonzero identity assigned at the current raw qualification rise.
    pub rf_burst_id: u64,
}

/// Mode policy and native-sample hang, independent of classifier framing.
struct Policy {
    mode: u32,
    hang_frames: u64,
    raw: bool,
    latched: bool,
    score: f32,
    last_voice: Option<u64>,
    rf_burst_id: u64,
}
impl Policy {
    fn new(mode: u32, hang_frames: u64) -> Self {
        Self {
            mode,
            hang_frames,
            raw: false,
            latched: false,
            score: 0.0,
            last_voice: None,
            rf_burst_id: 0,
        }
    }
    fn set_raw(&mut self, raw: bool) {
        if raw && !self.raw {
            self.rf_burst_id = self.rf_burst_id.wrapping_add(1).max(1);
        }
        self.raw = raw;
        if !raw {
            self.latched = false;
            self.score = 0.0;
            self.last_voice = None;
        }
    }
    fn observe(&mut self, score: f32, position: u64) {
        if self.raw {
            self.score = score;
            if score >= 0.5 {
                self.last_voice = Some(position);
                self.latched = true;
            }
        }
    }
    fn decision(&self, end_exclusive: u64) -> Decision {
        let voice_active = self.raw && self.score >= 0.5;
        let held = self.last_voice.is_some_and(|index| {
            end_exclusive.saturating_sub(index.saturating_add(1)) < self.hang_frames
        });
        let admitted = self.raw
            && match self.mode {
                0 => true,
                1 => self.latched,
                _ => voice_active || held,
            };
        Decision {
            admitted,
            voice_active,
            score: self.score,
            last_voice_sample_index: self.last_voice,
            rf_burst_id: self.rf_burst_id,
        }
    }
}

/// One serial receive owner with prepared analysis storage; Off has no SRC or NN.
pub struct SpeechVad {
    source_rate: u32,
    mode: u32,
    hang_ms: u32,
    maximum_native_frames: usize,
    policy: Policy,
    active: Option<Active>,
    expected_next: Option<u64>,
    needs_rearm: bool,
}
impl SpeechVad {
    /// Prepare mode 0/1/2 and a 0..32767 ms hang outside the audio callback.
    /// Active analysis accepts at most 4096 native frames; Off retains positive bounds.
    pub fn new(mode: u32, hang_ms: u32, maximum_native_frames: usize) -> Result<Self, i32> {
        Self::new_source_rate(48000, mode, hang_ms, maximum_native_frames)
    }
    /// Prepare the shared classifier on a mono 8..192 kHz source sample clock.
    /// Only analysis is converted to 16 kHz; programme PCM remains untouched.
    pub fn new_source_rate(
        source_rate: u32,
        mode: u32,
        hang_ms: u32,
        maximum_native_frames: usize,
    ) -> Result<Self, i32> {
        if !(8000..=192000).contains(&source_rate)
            || mode > 2
            || hang_ms > 32767
            || maximum_native_frames == 0
            || (mode != 0 && maximum_native_frames > 4096)
        {
            return Err(crate::session::INVALID_ARGUMENT);
        }
        let active = if mode == 0 {
            None
        } else {
            Some(Active::new(source_rate, maximum_native_frames)?)
        };
        Ok(Self {
            source_rate,
            mode,
            hang_ms,
            maximum_native_frames,
            policy: Policy::new(mode, u64::from(hang_ms) * u64::from(source_rate) / 1000),
            active,
            expected_next: None,
            needs_rearm: false,
        })
    }
    /// Analyze an undelayed finite mono source span without changing programme PCM.
    /// Raw loss closes immediately; a discontinuous source clock rearms a fresh burst.
    pub fn process(
        &mut self,
        native_mono: &[f32],
        raw_qualified: bool,
        first_native_sample: u64,
    ) -> Result<Decision, i32> {
        use crate::session::{FRAME_COUNT_EXCEEDED, INVALID_ARGUMENT, PROVIDER_FAILED};
        if native_mono.is_empty() || native_mono.iter().any(|s| !s.is_finite()) {
            return Err(INVALID_ARGUMENT);
        }
        if native_mono.len() > self.maximum_native_frames {
            return Err(FRAME_COUNT_EXCEEDED);
        }
        let end = first_native_sample
            .checked_add(native_mono.len() as u64)
            .ok_or(INVALID_ARGUMENT)?;
        if self
            .expected_next
            .is_some_and(|next| next != first_native_sample)
            || self.needs_rearm
        {
            self.reset()?;
        }
        if !raw_qualified {
            if self.policy.raw {
                self.reset()?;
            } else {
                self.policy.set_raw(false);
            }
            self.expected_next = Some(end);
            return Ok(self.policy.decision(end));
        }
        if !self.policy.raw {
            self.policy.set_raw(true);
            if let Some(active) = &mut self.active {
                active.origin = first_native_sample;
            }
        }
        if let Some(active) = &mut self.active {
            if active.process(native_mono, &mut self.policy, end).is_err() {
                let _ = self.reset();
                return Err(PROVIDER_FAILED);
            }
        }
        self.expected_next = Some(end);
        Ok(self.policy.decision(end))
    }
    /// Clear burst admission and rearm SRC/NN with three synthetic silent frames.
    /// Priming advances no native sample clock; a failed reset is retried before input.
    pub fn reset(&mut self) -> Result<(), i32> {
        self.needs_rearm = true;
        self.expected_next = None;
        self.policy.set_raw(false);
        if let Some(active) = &mut self.active {
            active.rearm()?;
        }
        self.needs_rearm = false;
        Ok(())
    }
    /// Carry the previous counter into a freshly prepared, unused replacement only.
    /// This copies metadata without calling the provider or transferring audio history.
    pub fn inherit_burst_id(&mut self, previous: &Self) {
        self.policy.rf_burst_id = previous.policy.rf_burst_id;
    }
    /// Whether an update can retain this exact live analysis instance and history.
    pub fn settings_equal(&self, other: &Self) -> bool {
        (
            self.source_rate,
            self.mode,
            self.hang_ms,
            self.maximum_native_frames,
        ) == (
            other.source_rate,
            other.mode,
            other.hang_ms,
            other.maximum_native_frames,
        )
    }
}
/// Setup-owned converter, fixed neural framer, and reusable clamped analysis copy.
struct Active {
    client: crate::samplerate_client::Client,
    detector: earshot::Detector,
    frame: [f32; 256],
    used: usize,
    input: Box<[f32]>,
    origin: u64,
}
impl Active {
    fn new(source_rate: u32, maximum_frames: usize) -> Result<Self, i32> {
        let mut active = Self {
            client: crate::samplerate_client::Client::new(source_rate, maximum_frames)?,
            detector: earshot::Detector::default(),
            frame: [0.0; 256],
            used: 0,
            input: vec![0.0; maximum_frames].into_boxed_slice(),
            origin: 0,
        };
        active.rearm()?;
        Ok(active)
    }
    fn rearm(&mut self) -> Result<(), i32> {
        self.used = 0;
        self.frame.fill(0.0);
        self.origin = 0;
        self.client.reset()?;
        self.detector.reset();
        for _ in 0..3 {
            self.detector.predict_f32(&[0.0; 256]);
        }
        Ok(())
    }
    fn process(&mut self, native: &[f32], policy: &mut Policy, end: u64) -> Result<(), i32> {
        use crate::session::PROVIDER_FAILED;
        for (output, input) in self.input[..native.len()].iter_mut().zip(native) {
            *output = input.clamp(-1.0, 1.0);
        }
        let mut consumed = 0;
        // The lowest supported source rate can produce two analysis samples
        // per source sample; retain a fixed allowance for the prepared filter.
        for _ in 0..native.len() * 2 + 64 {
            let step = self
                .client
                .process_one(&self.input[consumed..native.len()])?;
            consumed += step.consumed;
            if let Some(sample) = step.sample {
                self.frame[self.used] = sample.clamp(-1.0, 1.0);
                self.used += 1;
                if self.used == 256 {
                    let score = self.detector.predict_f32(&self.frame);
                    self.record_frame(score, step.native_position_billionths, policy, end)?;
                }
            } else if consumed == native.len() {
                return Ok(());
            } else if step.consumed == 0 {
                return Err(PROVIDER_FAILED);
            }
        }
        Err(PROVIDER_FAILED)
    }

    /// Validate classifier/SRC observations before publishing their source position.
    fn record_frame(
        &mut self,
        score: f32,
        source_position_billionths: i128,
        policy: &mut Policy,
        end: u64,
    ) -> Result<(), i32> {
        use crate::session::PROVIDER_FAILED;
        if !score.is_finite() || !(0.0..=1.0).contains(&score) {
            return Err(PROVIDER_FAILED);
        }
        let position =
            i128::from(self.origin) + source_position_billionths.div_euclid(1_000_000_000);
        if position < i128::from(self.origin) || position >= i128::from(end) {
            return Err(PROVIDER_FAILED);
        }
        policy.observe(score, position as u64);
        self.used = 0;
        Ok(())
    }
}
#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{Policy, SpeechVad};

    #[test]
    fn source_rate_analysis_preserves_source_positions_and_prepared_storage() {
        for rate in [8000, 16000, 48000, 96000] {
            let mut vad = SpeechVad::new_source_rate(rate, 2, 1000, 256).unwrap();
            assert_eq!(vad.policy.hang_frames, u64::from(rate));
            AUDIO_ALLOCATIONS.with(|count| count.set(0));
            AUDIO_ACTIVE.with(|active| active.set(true));
            for block in 0..40 {
                let first = 1000 + block * 127;
                let decision = vad.process(&[0.0; 127], true, first).unwrap();
                assert!(!decision.admitted);
                assert!(!decision.voice_active);
                assert!(decision.score.is_finite());
                assert_eq!(vad.expected_next, Some(first + 127));
            }
            vad.reset().unwrap();
            AUDIO_ACTIVE.with(|active| active.set(false));
            AUDIO_ALLOCATIONS.with(|count| assert_eq!(count.get(), 0));
        }
    }

    #[test]
    fn source_rate_analysis_rejects_unsupported_source_rates() {
        for rate in [0, 7999, 192001, u32::MAX] {
            assert!(matches!(
                SpeechVad::new_source_rate(rate, 2, 1000, 256),
                Err(crate::session::INVALID_ARGUMENT)
            ));
        }
    }

    #[test]
    fn shared_analysis_descriptor_uses_one_prepared_source_classifier() {
        // SAFETY: The provider descriptor has immutable process lifetime.
        let descriptor = unsafe { &*crate::rptadv_radio_descriptor() };
        let mut handle = std::ptr::null_mut();
        // SAFETY: Setup receives initialized, writable handle storage.
        assert_eq!(
            unsafe { (descriptor.speech_analysis_create)(8000, 128, &mut handle) },
            0
        );
        let mut result = crate::speech_analysis::AnalysisResult::default();
        AUDIO_ALLOCATIONS.with(|count| count.set(0));
        AUDIO_ACTIVE.with(|active| active.set(true));
        for first in (0..2560).step_by(128) {
            // SAFETY: Handle, bounded input and sized result remain alive and disjoint.
            assert_eq!(
                unsafe {
                    (descriptor.speech_analysis_process)(
                        handle,
                        [0.0; 128].as_ptr(),
                        128,
                        first,
                        &mut result,
                    )
                },
                0
            );
            assert_eq!(result.voice_active, 0);
            assert!(result.score.is_finite());
            assert_eq!(result.last_voice_source_sample, u64::MAX);
        }
        // SAFETY: Reset is serialized with this handle's processing.
        assert_eq!(unsafe { (descriptor.speech_analysis_reset)(handle) }, 0);
        AUDIO_ACTIVE.with(|active| active.set(false));
        AUDIO_ALLOCATIONS.with(|count| assert_eq!(count.get(), 0));
        // SAFETY: Processing has stopped; this is the unique owned handle.
        unsafe { (descriptor.speech_analysis_destroy)(handle) };
    }

    #[test]
    fn shared_analysis_descriptor_rejects_invalid_spans_and_clears_results() {
        use crate::session::{FRAME_COUNT_EXCEEDED, INVALID_ARGUMENT};
        // SAFETY: The descriptor has immutable process lifetime.
        let descriptor = unsafe { &*crate::rptadv_radio_descriptor() };
        let mut handle = std::ptr::null_mut();
        // SAFETY: Null setup output must be rejected without dereferencing it.
        assert_eq!(
            unsafe { (descriptor.speech_analysis_create)(8000, 32, std::ptr::null_mut()) },
            INVALID_ARGUMENT
        );
        // SAFETY: Invalid rate with valid output storage leaves no handle.
        assert_eq!(
            unsafe { (descriptor.speech_analysis_create)(0, 32, &mut handle) },
            INVALID_ARGUMENT
        );
        assert!(handle.is_null());
        // SAFETY: Writable setup output owns the successful prepared handle.
        assert_eq!(
            unsafe { (descriptor.speech_analysis_create)(8000, 32, &mut handle) },
            0
        );
        let mut result = crate::speech_analysis::AnalysisResult::default();
        let oversized = [0.0; 33];
        let nonfinite = [f32::NAN; 32];
        let valid = [0.0; 32];
        for (pointer, count, first, expected) in [
            (std::ptr::null(), 1, 0, INVALID_ARGUMENT),
            (valid.as_ptr(), 0, 0, INVALID_ARGUMENT),
            (oversized.as_ptr(), 33, 0, FRAME_COUNT_EXCEEDED),
            (nonfinite.as_ptr(), 32, 0, INVALID_ARGUMENT),
            (valid.as_ptr(), 32, u64::MAX, INVALID_ARGUMENT),
        ] {
            result.voice_active = 1;
            result.score = 1.0;
            // SAFETY: Each pointer is either valid for the stated test span or rejected before access.
            assert_eq!(
                unsafe {
                    (descriptor.speech_analysis_process)(handle, pointer, count, first, &mut result)
                },
                expected
            );
            assert_eq!(result.voice_active, 0);
            assert_eq!(result.score, 0.0);
            assert_eq!(result.last_voice_source_sample, u64::MAX);
        }
        // SAFETY: Null result and null handles are rejected before dereference.
        assert_eq!(
            unsafe {
                (descriptor.speech_analysis_process)(
                    handle,
                    std::ptr::null(),
                    0,
                    0,
                    std::ptr::null_mut(),
                )
            },
            INVALID_ARGUMENT
        );
        assert_eq!(
            unsafe {
                (descriptor.speech_analysis_process)(
                    std::ptr::null_mut(),
                    [0.0].as_ptr(),
                    1,
                    0,
                    &mut result,
                )
            },
            INVALID_ARGUMENT
        );
        result.struct_size = 0;
        // SAFETY: The short prefix is readable and must be rejected before tail access.
        assert_eq!(
            unsafe {
                (descriptor.speech_analysis_process)(handle, [0.0].as_ptr(), 1, 0, &mut result)
            },
            INVALID_ARGUMENT
        );
        // SAFETY: Null reset/destroy are respectively rejected/accepted without access.
        assert_eq!(
            unsafe { (descriptor.speech_analysis_reset)(std::ptr::null_mut()) },
            INVALID_ARGUMENT
        );
        unsafe {
            (descriptor.speech_analysis_destroy)(std::ptr::null_mut());
            (descriptor.speech_analysis_destroy)(handle);
        }
    }

    #[test]
    fn shared_analysis_voice_positions_follow_each_source_rate_and_reset() {
        let speech = pcm(include_bytes!("../tests/fixtures/vad/yes-48000-s16le.pcm"));
        // SAFETY: The provider descriptor has immutable process lifetime.
        let descriptor = unsafe { &*crate::rptadv_radio_descriptor() };
        for rate in [8000, 16000, 48000] {
            // Test-only decimation retains an actual speech fixture; production
            // analysis uses the prepared samplerate provider, not this copy.
            let source: Vec<f32> = speech
                .iter()
                .step_by((48000 / rate) as usize)
                .copied()
                .collect();
            let mut handle = std::ptr::null_mut();
            // SAFETY: Setup receives writable unique handle storage.
            assert_eq!(
                unsafe { (descriptor.speech_analysis_create)(rate, 256, &mut handle) },
                0
            );
            let mut result = crate::speech_analysis::AnalysisResult::default();
            let mut first = 1000;
            for block in source.chunks(127) {
                // SAFETY: Bounded live source input and sized result are disjoint.
                assert_eq!(
                    unsafe {
                        (descriptor.speech_analysis_process)(
                            handle,
                            block.as_ptr(),
                            block.len() as u32,
                            first,
                            &mut result,
                        )
                    },
                    0
                );
                first += block.len() as u64;
            }
            assert!((1000..first).contains(&result.last_voice_source_sample));
            // SAFETY: The handle is used by one serial owner throughout.
            assert_eq!(unsafe { (descriptor.speech_analysis_reset)(handle) }, 0);
            assert_eq!(
                unsafe {
                    (descriptor.speech_analysis_process)(
                        handle,
                        [0.0; 32].as_ptr(),
                        32,
                        0,
                        &mut result,
                    )
                },
                0
            );
            assert_eq!(result.voice_active, 0);
            assert_eq!(result.last_voice_source_sample, u64::MAX);
            // SAFETY: Processing is stopped and ownership is returned once.
            unsafe { (descriptor.speech_analysis_destroy)(handle) };
        }
    }
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;
    thread_local! {
        static AUDIO_ACTIVE:Cell<bool>=const {Cell::new(false)};
        static AUDIO_ALLOCATIONS:Cell<usize>=const {Cell::new(0)};
    }
    struct CheckedAllocator;
    // SAFETY: Allocator operations delegate unchanged to System; thread-local counters do not allocate.
    unsafe impl GlobalAlloc for CheckedAllocator {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            AUDIO_ACTIVE.with(|active| {
                if active.get() {
                    AUDIO_ALLOCATIONS.with(|count| count.set(count.get() + 1));
                }
            });
            // SAFETY: Forward the original valid allocator layout to System.
            unsafe { System.alloc(layout) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            AUDIO_ACTIVE.with(|active| {
                if active.get() {
                    AUDIO_ALLOCATIONS.with(|count| count.set(count.get() + 1));
                }
            });
            // SAFETY: Forward the matching allocation pointer and layout to System.
            unsafe { System.dealloc(pointer, layout) }
        }
    }
    #[global_allocator]
    static ALLOCATOR: CheckedAllocator = CheckedAllocator;
    #[link(name = "dl")]
    unsafe extern "C" {
        fn dlsym(
            handle: *mut std::ffi::c_void,
            name: *const std::ffi::c_char,
        ) -> *mut std::ffi::c_void;
    }
    fn no_callback_allocations(action: impl FnOnce()) {
        // SAFETY: The test looks up a named interposer symbol without calling a null pointer.
        let begin = unsafe { dlsym(std::ptr::null_mut(), c"swr_test_realtime_begin".as_ptr()) };
        // SAFETY: The test looks up a named interposer symbol without calling a null pointer.
        let end = unsafe { dlsym(std::ptr::null_mut(), c"swr_test_realtime_end".as_ptr()) };
        let required = std::env::var_os("RPTADV_REQUIRE_ALLOC_GUARD").is_some();
        if required {
            assert!(
                !begin.is_null() && !end.is_null(),
                "required C allocation interposer missing"
            );
        }
        let guard = if begin.is_null() || end.is_null() {
            None
        } else {
            // SAFETY: Both nonnull symbols have the known void(void) allocation-guard signature.
            Some(unsafe {
                (
                    std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn()>(begin),
                    std::mem::transmute::<*mut std::ffi::c_void, unsafe extern "C" fn()>(end),
                )
            })
        };
        if let Some((begin, end)) = guard {
            // SAFETY: Setup resolved both allocation-guard callbacks with their exact signatures.
            unsafe {
                begin();
                end();
            }
        }
        AUDIO_ALLOCATIONS.with(|count| count.set(0));
        AUDIO_ACTIVE.with(|active| active.set(true));
        if let Some((begin, _)) = guard {
            // SAFETY: Setup resolved both allocation-guard callbacks with their exact signatures.
            unsafe {
                begin();
            }
        }
        action();
        if let Some((_, end)) = guard {
            // SAFETY: Setup resolved both allocation-guard callbacks with their exact signatures.
            unsafe {
                end();
            }
        }
        AUDIO_ACTIVE.with(|active| active.set(false));
        AUDIO_ALLOCATIONS.with(|count| assert_eq!(count.get(), 0, "Rust callback allocation/free"));
    }
    #[test]
    fn off_preserves_raw_admission_without_speech() {
        let mut vad = SpeechVad::new(0, 1000, 960).unwrap();
        let open = vad.process(&[0.0; 17], true, 5).unwrap();
        assert!(open.admitted);
        assert!(!open.voice_active);
        assert_eq!(open.score, 0.0);
        assert_eq!(open.last_voice_sample_index, None);
        assert!(!vad.process(&[0.0], false, 22).unwrap().admitted);
        vad.reset().unwrap();
        assert!(vad.settings_equal(&SpeechVad::new(0, 1000, 960).unwrap()));
        assert!(!vad.settings_equal(&SpeechVad::new(0, 999, 960).unwrap()));
        let mut changed = SpeechVad::new(0, 999, 960).unwrap();
        changed.inherit_burst_id(&vad);
        assert_eq!(changed.process(&[0.0], true, 23).unwrap().rf_burst_id, 2);
    }
    #[test]
    fn kerchunk_requires_its_own_speech_and_latches_through_pauses() {
        let mut policy = Policy::new(1, 48000);
        policy.set_raw(true);
        assert!(!policy.decision(768).admitted);
        policy.observe(0.499, 767);
        assert!(!policy.decision(768).admitted);
        policy.observe(0.5, 1535);
        assert!(policy.decision(1536).admitted);
        assert_eq!(policy.decision(1536).rf_burst_id, 1);
        policy.observe(0.0, 2303);
        assert!(!policy.decision(480000).voice_active);
        assert!(policy.decision(480000).admitted);
        policy.set_raw(false);
        let closed = policy.decision(480001);
        assert!(!closed.admitted);
        assert_eq!(closed.score, 0.0);
        assert_eq!(closed.last_voice_sample_index, None);
        policy.set_raw(true);
        let rekeyed = policy.decision(480002);
        assert_eq!(rekeyed.rf_burst_id, 2);
        assert!(!rekeyed.admitted);
    }
    #[test]
    fn on_hang_uses_exact_native_sample_time_and_rf_loss_closes_now() {
        let mut policy = Policy::new(2, 48000);
        policy.set_raw(true);
        policy.observe(0.5, 767);
        let voice = policy.decision(768);
        assert!(voice.admitted && voice.voice_active);
        policy.observe(0.49, 1535);
        assert!(!policy.decision(1536).voice_active);
        assert!(policy.decision(48767).admitted);
        assert!(!policy.decision(48768).admitted);
        policy.observe(0.8, 49000);
        policy.set_raw(false);
        let closed = policy.decision(49001);
        assert!(!closed.admitted && !closed.voice_active);
        assert_eq!(closed.score, 0.0);
        assert_eq!(closed.last_voice_sample_index, None);
        policy.set_raw(true);
        assert!(!policy.decision(49002).admitted);
    }
    fn pcm(bytes: &[u8]) -> Vec<f32> {
        bytes
            .chunks_exact(2)
            .map(|p| i16::from_le_bytes([p[0], p[1]]) as f32 / 32768.0)
            .collect()
    }
    #[test]
    fn real_speech_admits_within_preroll_and_rapid_rekey_noise_needs_own_speech() {
        let speech = pcm(include_bytes!("../tests/fixtures/vad/yes-48000-s16le.pcm"));
        let noise = pcm(include_bytes!(
            "../tests/fixtures/vad/noise-48000-s16le.pcm"
        ));
        let mut vad = SpeechVad::new(1, 1000, 960).unwrap();
        let mut clock = 0_u64;
        let mut first_admitted = None;
        for block in speech[..16896].chunks(960) {
            let decision = vad.process(block, true, clock).unwrap();
            clock += block.len() as u64;
            if decision.admitted {
                first_admitted.get_or_insert(clock);
            }
        }
        assert!(first_admitted.unwrap() <= 10080 + 7680);
        let closed = vad.process(&[0.0; 480], false, clock).unwrap();
        clock += 480;
        assert!(!closed.admitted && !closed.voice_active);
        assert_eq!(closed.score, 0.0);
        assert_eq!(closed.last_voice_sample_index, None);
        for block in noise.chunks(960) {
            let decision = vad.process(block, true, clock).unwrap();
            clock += block.len() as u64;
            assert!(
                !decision.admitted,
                "noise admitted at{clock} score{}",
                decision.score
            );
            assert_eq!(decision.rf_burst_id, 2);
        }
    }
    #[test]
    fn actual_src_and_neural_history_are_invariant_to_callback_partition() {
        let speech = pcm(include_bytes!("../tests/fixtures/vad/no-48000-s16le.pcm"));
        let mut expected = None;
        for size in [1, 17, 960, 4096] {
            let mut vad = SpeechVad::new(1, 1000, 4096).unwrap();
            let mut clock = 100_u64;
            let mut final_decision = super::Decision::default();
            for block in speech.chunks(size) {
                final_decision = vad.process(block, true, clock).unwrap();
                clock += block.len() as u64;
            }
            assert!(final_decision.admitted);
            let result = (
                final_decision.score.to_bits(),
                final_decision.last_voice_sample_index,
                final_decision.rf_burst_id,
            );
            if let Some(expected) = expected {
                assert_eq!(result, expected);
            } else {
                expected = Some(result);
            }
        }
    }
    #[test]
    fn invalid_setup_and_input_bounds_do_not_enter_neural_processing() {
        use crate::session::{FRAME_COUNT_EXCEEDED, INVALID_ARGUMENT};
        for (mode, hang, max) in [
            (3, 1000, 960),
            (1, 32768, 960),
            (1, 1000, 0),
            (1, 1000, 4097),
        ] {
            assert!(matches!(
                SpeechVad::new(mode, hang, max),
                Err(INVALID_ARGUMENT)
            ));
        }
        let mut vad = SpeechVad::new(1, 0, 1).unwrap();
        assert!(matches!(vad.process(&[], true, 0), Err(INVALID_ARGUMENT)));
        assert!(matches!(
            vad.process(&[0.0; 2], true, 0),
            Err(FRAME_COUNT_EXCEEDED)
        ));
        assert!(matches!(
            vad.process(&[f32::NAN], true, 0),
            Err(INVALID_ARGUMENT)
        ));
        assert!(matches!(
            vad.process(&[0.0], true, u64::MAX),
            Err(INVALID_ARGUMENT)
        ));
    }
    #[test]
    fn discontinuity_and_explicit_reset_require_a_fresh_burst() {
        let speech = pcm(include_bytes!("../tests/fixtures/vad/yes-48000-s16le.pcm"));
        let noise = pcm(include_bytes!(
            "../tests/fixtures/vad/noise-48000-s16le.pcm"
        ));
        let mut vad = SpeechVad::new(1, 1000, 960).unwrap();
        let mut clock = 0_u64;
        let mut decision = super::Decision::default();
        for block in speech.chunks(960) {
            decision = vad.process(block, true, clock).unwrap();
            clock += block.len() as u64;
        }
        assert!(decision.admitted);
        clock += 100;
        let gap = vad.process(&noise[..960], true, clock).unwrap();
        assert_eq!(gap.rf_burst_id, 2);
        assert!(!gap.admitted);
        clock += 960;
        vad.reset().unwrap();
        let reset = vad.process(&noise[..960], true, clock).unwrap();
        assert_eq!(reset.rf_burst_id, 3);
        assert!(!reset.admitted);
    }
    #[test]
    fn analysis_clamps_headroom_without_modifying_programme() {
        let mut vad = SpeechVad::new(2, 0, 960).unwrap();
        let programme = [4.0; 960];
        for index in 0..4 {
            vad.process(&programme, true, index * 960).unwrap();
        }
        assert_eq!(programme, [4.0; 960]);
    }
    #[test]
    fn src_faults_fail_closed_and_next_valid_span_rearms() {
        use crate::samplerate_client::{TestFailure, with_test_failure};
        use crate::session::PROVIDER_FAILED;
        assert!(matches!(
            with_test_failure(TestFailure::Create, || SpeechVad::new(1, 1000, 960)),
            Err(PROVIDER_FAILED)
        ));
        let mut vad = SpeechVad::new(1, 1000, 960).unwrap();
        vad.process(&[0.0; 960], true, 0).unwrap();
        assert!(matches!(
            with_test_failure(TestFailure::Reset, || vad.process(&[0.0; 960], false, 960)),
            Err(PROVIDER_FAILED)
        ));
        let recovered = vad.process(&[0.0; 960], true, 1920).unwrap();
        assert!(!recovered.admitted);
        assert_eq!(recovered.rf_burst_id, 2);
        assert!(matches!(
            with_test_failure(TestFailure::Process, || vad.process(
                &[0.0; 960],
                true,
                2880
            )),
            Err(PROVIDER_FAILED)
        ));
        let recovered = vad.process(&[0.0; 960], true, 3840).unwrap();
        assert!(!recovered.admitted);
        assert_eq!(recovered.rf_burst_id, 3);
    }
    #[test]
    fn repeated_bursts_process_and_rearm_without_callback_allocations() {
        let mut vad = SpeechVad::new(1, 1000, 4096).unwrap();
        let silence = [0.0; 4096];
        let mut clock = 0_u64;
        no_callback_allocations(|| {
            for _ in 0..50 {
                vad.process(&silence[..1], false, clock).unwrap();
                clock += 1;
                for count in [1, 17, 960, 4096] {
                    vad.process(&silence[..count], true, clock).unwrap();
                    clock += count as u64;
                }
                vad.reset().unwrap();
            }
        });
    }
    #[test]
    fn off_never_opens_src_even_when_provider_is_unavailable() {
        use crate::samplerate_client::{TestFailure, with_test_failure};
        let mut vad =
            with_test_failure(TestFailure::Create, || SpeechVad::new(0, 1000, 960)).unwrap();
        let mut large = SpeechVad::new(0, 1000, 4097).unwrap();
        assert!(large.process(&[0.0; 4097], true, 0).unwrap().admitted);
        assert!(SpeechVad::new(0, 1000, u32::MAX as usize).is_ok());
        let decision =
            with_test_failure(TestFailure::Process, || vad.process(&[0.0; 960], true, 0)).unwrap();
        assert!(decision.admitted);
        with_test_failure(TestFailure::Reset, || vad.reset()).unwrap();
    }
    #[test]
    fn stale_classifier_updates_cannot_reopen_an_unqualified_policy() {
        let mut policy = Policy::new(2, 0);
        policy.rf_burst_id = u64::MAX;
        policy.set_raw(true);
        assert_eq!(policy.decision(1).rf_burst_id, 1);
        policy.set_raw(true);
        assert_eq!(policy.decision(1).rf_burst_id, 1);
        policy.observe(0.5, 767);
        assert!(policy.decision(768).admitted);
        policy.observe(0.49, 1535);
        assert!(!policy.decision(1536).admitted);
        policy.set_raw(false);
        policy.observe(0.99, 1600);
        let decision = policy.decision(1601);
        assert!(!decision.admitted && !decision.voice_active);
        assert_eq!(decision.score, 0.0);
        assert_eq!(decision.last_voice_sample_index, None);
    }
    #[test]
    fn real_on_mode_hang_expires_at_the_source_clock_deadline() {
        let speech = pcm(include_bytes!("../tests/fixtures/vad/no-48000-s16le.pcm"));
        let mut vad = SpeechVad::new(2, 1000, 4096).unwrap();
        let mut clock = 0_u64;
        let mut decision = super::Decision::default();
        for block in speech.chunks(960) {
            decision = vad.process(block, true, clock).unwrap();
            clock += block.len() as u64;
        }
        assert!(decision.admitted && !decision.voice_active);
        let last_voice = decision.last_voice_sample_index.unwrap();
        let expiry = last_voice + 1 + 48000;
        let silence = [0.0; 4096];
        while clock < expiry - 1 {
            let count = ((expiry - 1 - clock) as usize).min(4096);
            decision = vad.process(&silence[..count], true, clock).unwrap();
            clock += count as u64;
        }
        assert_eq!(decision.last_voice_sample_index, Some(last_voice));
        assert!(decision.admitted);
        assert!(!vad.process(&silence[..1], true, clock).unwrap().admitted);
    }
    #[test]
    fn invalid_external_analysis_observations_do_not_grant_admission() {
        use crate::session::PROVIDER_FAILED;
        let mut active = super::Active::new(48000, 960).unwrap();
        active.origin = 100;
        let mut policy = Policy::new(1, 48000);
        policy.set_raw(true);
        for score in [f32::NAN, f32::INFINITY, -0.01, 1.01] {
            assert!(matches!(
                active.record_frame(score, 0, &mut policy, 1000),
                Err(PROVIDER_FAILED)
            ));
        }
        for position in [-1_000_000_000, 900_000_000_000] {
            assert!(matches!(
                active.record_frame(0.5, position, &mut policy, 1000),
                Err(PROVIDER_FAILED)
            ));
        }
        assert!(!policy.decision(1000).admitted);
        active
            .record_frame(0.5, 1_999_999_999, &mut policy, 1000)
            .unwrap();
        assert_eq!(policy.decision(1000).last_voice_sample_index, Some(101));
    }
    #[test]
    fn partial_progress_is_valid_but_stalled_and_flooded_providers_are_bounded() {
        use crate::samplerate_client::{TestFailure, with_test_failure};
        use crate::session::PROVIDER_FAILED;
        let mut vad = SpeechVad::new(1, 1000, 4096).unwrap();
        with_test_failure(TestFailure::PartialInput, || {
            vad.process(&[0.0; 960], true, 0)
        })
        .unwrap();
        for (failure, count) in [
            (TestFailure::Stall, 17),
            (TestFailure::Flood, 1),
            (TestFailure::Flood, 4096),
        ] {
            assert!(matches!(
                with_test_failure(failure, || vad.process(&[0.0; 4096][..count], true, 960)),
                Err(PROVIDER_FAILED)
            ));
            assert!(!vad.process(&[0.0; 17], true, 960).unwrap().admitted);
            vad.reset().unwrap();
        }
    }
}
