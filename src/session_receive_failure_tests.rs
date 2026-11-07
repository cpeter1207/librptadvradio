use super::*;

fn pcm(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(2)
        .map(|sample| i16::from_le_bytes([sample[0], sample[1]]) as f32 / 32768.0)
        .collect()
}

fn prepared(config: &SessionConfig, ports: &SessionPorts) -> TestSession {
    let mut raw = ptr::null_mut();
    // SAFETY: The exported table and all complete setup records remain live;
    // TestSession takes ownership before any subsequent assertion can fail.
    unsafe {
        let api = &*crate::rptadv_radio_descriptor();
        assert_eq!((api.session_create)(config, ports, &mut raw), OK);
        let session = TestSession(raw);
        assert_eq!((api.session_warm)(session.0), OK);
        session
    }
}

fn receive_pcm(
    session: &TestSession,
    mono: &[f32],
    keyed: bool,
) -> (c_int, [f32; 960], ReceiveResult) {
    assert_eq!(mono.len(), 960);
    let mut input = [0.0; 1920];
    for (frame, sample) in input.chunks_exact_mut(2).zip(mono) {
        frame[0] = *sample;
    }
    let mut output = [9.0; 960];
    let mut result = ReceiveResult::default();
    let controls = ReceiveInput {
        hardware_carrier: u32::from(keyed),
        ..ReceiveInput::default()
    };
    // SAFETY: This serial owner retains its session, ports, complete stereo
    // input, bounded mono output and result throughout the exported ABI call.
    let status = unsafe {
        ((*crate::rptadv_radio_descriptor()).session_receive)(
            session.0,
            input.as_ptr(),
            output.as_mut_ptr(),
            960,
            &controls,
            &mut result,
        )
    };
    (status, output, result)
}

#[test]
fn public_receive_failure_rearms_latched_vad_after_rf_closure() {
    let speech = pcm(include_bytes!("../tests/fixtures/vad/yes-48000-s16le.pcm"));
    let noise = pcm(include_bytes!(
        "../tests/fixtures/vad/noise-48000-s16le.pcm"
    ));
    for external_delay in [0, 1] {
        let mut config = valid_config();
        config.maximum_receive_frame_count = 960;
        config.receive.vad_mode = 1;
        config.receive.vad_external_delay = external_delay;
        let mut deemphasis = TestProcessor::default();
        let ports = SessionPorts {
            receive_deemphasis: test_port(&mut deemphasis),
            ..SessionPorts::default()
        };
        let session = prepared(&config, &ports);
        let mut admitted = ReceiveResult::default();
        for block in speech.chunks_exact(960) {
            let (status, _, result) = receive_pcm(&session, block, true);
            assert_eq!(status, OK);
            admitted = result;
        }
        assert_eq!(admitted.receiver_keyed, 1);
        assert_ne!(admitted.vad_voice_sample_index, u64::MAX);
        let before = session.snapshot();
        // SAFETY: This port still points to the retained test-owned processor.
        unsafe { (*ports.receive_deemphasis.context.cast::<TestProcessor>()).fail = true };
        let (status, output, _) = receive_pcm(&session, &[0.0; 960], false);
        assert_eq!(status, PROVIDER_FAILED);
        assert_eq!(output, [0.0; 960]);
        assert_eq!(session.snapshot().receive_frames, before.receive_frames);
        assert_eq!(session.snapshot().provider_failures, 1);
        // SAFETY: The same retained processor is restored before healthy input.
        unsafe { (*ports.receive_deemphasis.context.cast::<TestProcessor>()).fail = false };
        for block in noise.chunks_exact(960) {
            let (status, _, result) = receive_pcm(&session, block, true);
            assert_eq!(status, OK);
            assert_eq!(result.raw_receiver_keyed, 1);
            assert_eq!(result.rf_burst_id, admitted.rf_burst_id + 1);
            assert_eq!(result.receiver_keyed, 0);
            assert_eq!(result.vad_voice_active, 0);
            assert_eq!(result.vad_voice_sample_index, u64::MAX);
        }
    }
}

#[test]
fn public_receive_failure_discards_delayed_program_before_fresh_voice() {
    let speech = pcm(include_bytes!("../tests/fixtures/vad/yes-48000-s16le.pcm"));
    for stage in 0..3 {
        let mut config = valid_config();
        config.maximum_receive_frame_count = 960;
        config.receive.vad_mode = 1;
        config.receive.native_squelch_delay_frames = 48000;
        let mut deemphasis = TestProcessor::default();
        let mut failed = TestProcessor::default();
        let mut ports = SessionPorts {
            receive_deemphasis: test_port(&mut deemphasis),
            ..SessionPorts::default()
        };
        let port = test_port(&mut failed);
        match stage {
            0 => ports.receive_filter = port,
            1 => ports.receive_noise_reduction = port,
            _ => ports.receive_dynamics = port,
        }
        let session = prepared(&config, &ports);
        for block in speech.chunks_exact(960) {
            assert_eq!(receive_pcm(&session, block, true).0, OK);
        }
        assert_eq!(session.snapshot().receiver_keyed, 1);
        // SAFETY: This port retains the test-owned deemphasis processor.
        unsafe { (*ports.receive_deemphasis.context.cast::<TestProcessor>()).bias = 0.25 };
        for _ in 0..50 {
            assert_eq!(receive_pcm(&session, &[0.0; 960], true).0, OK);
        }
        // SAFETY: Both port contexts remain live and are changed only between calls.
        unsafe {
            (*ports.receive_deemphasis.context.cast::<TestProcessor>()).bias = 0.0;
            (*port.context.cast::<TestProcessor>()).fail = true;
        }
        let before = session.snapshot();
        let (status, output, _) = receive_pcm(&session, &[0.0; 960], true);
        assert_eq!(status, PROVIDER_FAILED);
        assert_eq!(output, [0.0; 960]);
        assert_eq!(session.snapshot().receive_frames, before.receive_frames);
        assert_eq!(session.snapshot().provider_failures, 1);
        // SAFETY: The retained failed processor is restored before fresh input.
        unsafe { (*port.context.cast::<TestProcessor>()).fail = false };
        let mut heard_voice = false;
        for block in speech.chunks_exact(960) {
            let (status, output, result) = receive_pcm(&session, block, true);
            assert_eq!(status, OK);
            assert_eq!(result.rf_burst_id, before.rf_burst_id + 1);
            heard_voice |= result.receiver_keyed != 0;
            if result.receiver_keyed != 0 {
                assert_ne!(result.vad_voice_sample_index, u64::MAX);
                assert!(result.vad_voice_sample_index >= before.receive_frames);
                assert!(result.vad_voice_sample_index < result.first_sample_index + 960);
            }
            assert_eq!(
                output, [0.0; 960],
                "old programme replayed at stage {stage}"
            );
        }
        assert!(heard_voice);
        let mut resumed = false;
        for _ in 0..50 {
            let (status, output, _) = receive_pcm(&session, &[0.0; 960], true);
            assert_eq!(status, OK);
            resumed |= output.iter().any(|sample| *sample != 0.0);
        }
        assert!(resumed);
    }
}
