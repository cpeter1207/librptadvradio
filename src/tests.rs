use super::*;
use std::ffi::CStr;

fn valid_config() -> RadioConfig {
    RadioConfig {
        struct_size: size_of::<RadioConfig>() as u32,
        abi_version: ABI_VERSION,
        native_sample_rate_hz: 48_000,
        maximum_frame_count: 4,
        interleaved_channels: CANONICAL_CHANNELS,
    }
}

fn render_config(output_a_route: u32, output_b_route: u32) -> TransmitRenderConfig {
    TransmitRenderConfig {
        struct_size: size_of::<TransmitRenderConfig>() as u32,
        output_a_route,
        output_b_route,
        ctcss_peak_a: 1.0,
        ctcss_bias_a: 0.0,
        ctcss_peak_b: 1.0,
        ctcss_bias_b: 0.0,
    }
}

fn create_radio() -> *mut Radio {
    create_radio_with_maximum_frame_count(4)
}

fn create_radio_with_maximum_frame_count(maximum_frame_count: u32) -> *mut Radio {
    let mut config = valid_config();
    let mut radio = ptr::null_mut();
    config.maximum_frame_count = maximum_frame_count;
    assert_eq!(radio_create(&config, &mut radio), RADIO_OK);
    radio
}

#[test]
fn descriptor_is_complete_and_versioned() {
    let descriptor = rptadv_radio_descriptor();
    let descriptor = unsafe { descriptor.as_ref() }.expect("descriptor must be static");
    assert_eq!(
        descriptor.struct_size as usize,
        size_of::<RadioDescriptor>()
    );
    assert_eq!(descriptor.abi_version, ABI_VERSION);
    let capability = unsafe { CStr::from_ptr(descriptor.capability_name) };
    assert_eq!(capability.to_bytes(), b"rptadv.radio-core");
    assert!(descriptor.radio_ctcss_frequency_supported as usize != 0);
    assert!(descriptor.radio_ctcss_legacy_frequency as usize != 0);
    assert!(descriptor.radio_ctcss_legacy_peak as usize != 0);
    assert!(descriptor.radio_ctcss_legacy_scaled_peak as usize != 0);
    assert!(descriptor.radio_ctcss_legacy_scaled_levels as usize != 0);
    assert!(descriptor.radio_ctcss_generate_f32 as usize != 0);
    assert!(descriptor.radio_ctcss_generate_tail_f32 as usize != 0);
    assert!(descriptor.radio_ctcss_phase_radians as usize != 0);
    assert!(descriptor.radio_dcs_code_supported as usize != 0);
    assert!(descriptor.radio_dcs_configure_transmit as usize != 0);
    assert!(descriptor.radio_dcs_generate_f32 as usize != 0);
    assert!(descriptor.radio_dcs_tail_phase_radians as usize != 0);
    assert!(descriptor.radio_extract_receive_f32 as usize != 0);
    assert!(descriptor.radio_render_calibrated_test_tone_f32 as usize != 0);
    assert!(descriptor.radio_calibrated_test_tone_phase_radians as usize != 0);
    assert!(descriptor.radio_native_parrot_bind_f32 as usize != 0);
    assert!(descriptor.radio_native_parrot_reset as usize != 0);
    assert!(descriptor.radio_native_parrot_rx_transition as usize != 0);
    assert!(descriptor.radio_native_parrot_record_f32 as usize != 0);
    assert!(descriptor.radio_native_parrot_play_f32 as usize != 0);
    assert!(descriptor.radio_native_parrot_status as usize != 0);
    assert!(descriptor.radio_parse_rx_audio_mode as usize != 0);
    assert!(descriptor.radio_parse_carrier_source as usize != 0);
    assert!(descriptor.radio_parse_ctcss_source as usize != 0);
    assert!(descriptor.radio_parse_tone_off_mode as usize != 0);
    assert!(descriptor.radio_dcs_configure_receive as usize != 0);
    assert!(descriptor.radio_dcs_process_receive_f32 as usize != 0);
    assert!(descriptor.radio_ctcss_configure_receive as usize != 0);
    assert!(descriptor.radio_ctcss_process_receive_f32 as usize != 0);
    assert!(descriptor.radio_measure_raw_pcm_f32 as usize != 0);
    assert!(descriptor.radio_micor_squelch_update as usize != 0);
    assert!(descriptor.radio_measure_envelope_f32 as usize != 0);
    assert!(descriptor.radio_delay_line_f32 as usize != 0);
    assert!(descriptor.radio_center_slicer_f32 as usize != 0);
    assert!(descriptor.radio_deemphasis_integrator_f32 as usize != 0);
    assert!(descriptor.radio_fir_mono_f32 as usize != 0);
    assert!(descriptor.radio_receive_frontend_f32 as usize != 0);
    assert!(descriptor.radio_elapsed_ms as usize != 0);
    assert!(descriptor.radio_timer_consume as usize != 0);
    assert!(descriptor.radio_signal_mode_advance as usize != 0);
    assert!(descriptor.radio_ctcss_render_state_advance as usize != 0);
    assert!(descriptor.radio_tx_finish_advance as usize != 0);
    assert!(descriptor.radio_tx_finish_continue as usize != 0);
    assert!(descriptor.radio_tx_complete as usize != 0);
    assert!(descriptor.radio_rx_blanking_advance as usize != 0);
    assert!(descriptor.radio_vox_carrier_advance as usize != 0);
    assert!(descriptor.radio_tx_cpu_saver_advance as usize != 0);
    assert!(descriptor.radio_rx_cpu_saver_advance as usize != 0);
}

#[test]
fn timer_descriptor_preserves_sample_clocked_compatibility_arithmetic() {
    let mut remainder = 47_u32;
    let mut elapsed = -1_i32;
    let mut timer = 2_i32;
    let mut residual = -1_i32;

    assert_eq!(radio_elapsed_ms(&mut remainder, 49, &mut elapsed), RADIO_OK);
    assert_eq!(elapsed, 2);
    assert_eq!(remainder, 0);
    assert_eq!(
        radio_timer_consume(&mut timer, elapsed, &mut residual),
        RADIO_OK
    );
    assert_eq!(timer, 0);
    assert_eq!(residual, 0);
    assert_eq!(
        radio_elapsed_ms(std::ptr::null_mut(), 48, &mut elapsed),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_timer_consume(&mut timer, 1, std::ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
}

fn legacy_raw_f32(codes: &[i16]) -> Vec<f32> {
    codes
        .iter()
        .map(|code| f32::from(*code) / 32_768.0)
        .collect()
}

fn stereo_with_legacy_selected_samples(selected: &[i16]) -> Vec<f32> {
    let mut samples = vec![0.0_f32; 12 * 160];
    for (offset, code) in selected.iter().enumerate() {
        samples[10 + offset * 12] = f32::from(*code) / 32_768.0;
    }
    samples
}

#[test]
fn raw_pcm_meter_matches_asl3_stereo_selection_power_and_clip_counter() {
    let mut statistics = AudioStatistics {
        maxbuf: [0; AUDIO_STATS_LEN],
        clipbuf: [0; AUDIO_STATS_LEN],
        pwrbuf: [0; AUDIO_STATS_LEN],
        index: 0,
    };
    let mut selected = [0_i16; 160];
    let mut clipping = 0_u32;
    let mut samples;

    selected[..4].fill(i16::MAX);
    selected[4] = i16::MIN;
    samples = stereo_with_legacy_selected_samples(&selected);
    // A full-scale value at a non-selected 48 kHz phase has no meter effect.
    samples[0] = 1.0;

    assert_eq!(
        radio_measure_raw_pcm_f32(
            samples.as_ptr(),
            samples.len() as u32,
            2,
            &mut statistics,
            &mut clipping,
        ),
        RADIO_OK
    );
    assert_eq!(clipping, 1);
    assert_eq!(statistics.index, 1);
    assert_eq!(statistics.maxbuf[0], 32_768);
    assert_eq!(statistics.clipbuf[0], 4);
    assert_eq!(
        statistics.pwrbuf[0],
        ((4_u64 * 32_767_u64 * 32_767_u64 + 32_768_u64 * 32_768_u64) / 160) as u32
    );

    selected.fill(0);
    // The ASL3 routine accumulates adjacent pairs rather than resetting the
    // counter after a gap. Three two-sample runs therefore report clipping.
    for offset in [0_usize, 1, 3, 4, 6, 7] {
        selected[offset] = i16::MAX;
    }
    samples = stereo_with_legacy_selected_samples(&selected);
    assert_eq!(
        radio_measure_raw_pcm_f32(
            samples.as_ptr(),
            samples.len() as u32,
            2,
            &mut statistics,
            &mut clipping,
        ),
        RADIO_OK
    );
    assert_eq!(clipping, 1);
    assert_eq!(statistics.clipbuf[1], 3);
}

#[test]
fn raw_pcm_meter_preserves_mono_bounds_slots_and_normalized_quantization() {
    let mut statistics = AudioStatistics {
        maxbuf: [0; AUDIO_STATS_LEN],
        clipbuf: [0; AUDIO_STATS_LEN],
        pwrbuf: [0; AUDIO_STATS_LEN],
        index: 49,
    };
    let mut samples = vec![0.0_f32; 1_000];
    let mut clipping = 99_u32;

    samples[5] = 1.0;
    samples[959] = -1.0;
    samples[965] = 1.0;
    assert_eq!(
        radio_measure_raw_pcm_f32(
            samples.as_ptr(),
            samples.len() as u32,
            1,
            &mut statistics,
            &mut clipping,
        ),
        RADIO_OK
    );
    assert_eq!(clipping, 0);
    assert_eq!(statistics.index, 0);
    assert_eq!(statistics.maxbuf[49], 32_768);
    assert_eq!(
        statistics.pwrbuf[49],
        ((32_767_u64 * 32_767_u64 + 32_768_u64 * 32_768_u64) / 160) as u32
    );

    statistics.maxbuf[0] = 123;
    assert_eq!(
        radio_measure_raw_pcm_f32(std::ptr::null(), 0, 1, &mut statistics, &mut clipping),
        RADIO_OK
    );
    assert_eq!(statistics.index, 1);
    assert_eq!(statistics.maxbuf[0], 123);

    statistics.index = -1;
    assert_eq!(
        radio_measure_raw_pcm_f32(std::ptr::null(), 0, 1, &mut statistics, &mut clipping),
        RADIO_OK
    );
    assert_eq!(statistics.index, 1);
}

#[test]
fn raw_pcm_meter_rejects_invalid_input_without_mutating_statistics() {
    let mut statistics = AudioStatistics {
        maxbuf: [7; AUDIO_STATS_LEN],
        clipbuf: [8; AUDIO_STATS_LEN],
        pwrbuf: [9; AUDIO_STATS_LEN],
        index: 11,
    };
    let expected = statistics;
    let mut clipping = 99_u32;
    let invalid = legacy_raw_f32(&[0, 0, 0, 0, 0, 0]);
    let mut non_finite = invalid.clone();
    non_finite[5] = f32::NAN;

    assert_eq!(
        radio_measure_raw_pcm_f32(
            non_finite.as_ptr(),
            non_finite.len() as u32,
            1,
            &mut statistics,
            &mut clipping,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(clipping, 0);
    assert_eq!(statistics.maxbuf, expected.maxbuf);
    assert_eq!(statistics.clipbuf, expected.clipbuf);
    assert_eq!(statistics.pwrbuf, expected.pwrbuf);
    assert_eq!(statistics.index, expected.index);
    assert_eq!(
        radio_measure_raw_pcm_f32(std::ptr::null(), 6, 1, &mut statistics, &mut clipping,),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_measure_raw_pcm_f32(
            invalid.as_ptr(),
            invalid.len() as u32,
            3,
            &mut statistics,
            &mut clipping,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_measure_raw_pcm_f32(
            invalid.as_ptr(),
            invalid.len() as u32,
            1,
            std::ptr::null_mut(),
            &mut clipping,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_measure_raw_pcm_f32(
            invalid.as_ptr(),
            invalid.len() as u32,
            1,
            &mut statistics,
            std::ptr::null_mut(),
        ),
        RADIO_INVALID_ARGUMENT
    );
}

#[test]
fn native_parrot_preserves_legacy_recording_playback_and_transition_behavior() {
    let radio = create_radio();
    let mut storage = [0.0_f32; 6];
    let input = [0.125_f32, -0.25, 0.5, -0.75];
    let mut output = [-1.0_f32; 4];
    let mut playback_started = u32::MAX;
    let mut recorded = u32::MAX;
    let mut played = u32::MAX;
    let mut status = NativeParrotStatus {
        recorded_samples: u64::MAX,
        playback_offset: u64::MAX,
        playing: u32::MAX,
        truncated: u32::MAX,
    };

    assert_eq!(
        radio_native_parrot_bind_f32(radio, storage.as_mut_ptr(), storage.len() as u32),
        RADIO_OK
    );
    assert_eq!(
        radio_native_parrot_rx_transition(radio, 0, 7, &mut playback_started),
        RADIO_OK
    );
    assert_eq!(playback_started, 0);
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 4, 6, &mut recorded),
        RADIO_OK
    );
    assert_eq!(recorded, 4);
    assert_eq!(radio_native_parrot_status(radio, &mut status), RADIO_OK);
    assert_eq!(status.recorded_samples, 4);
    assert_eq!(status.playback_offset, 0);
    assert_eq!(status.playing, 0);
    assert_eq!(status.truncated, 0);

    assert_eq!(
        radio_native_parrot_rx_transition(radio, 1, 0, &mut playback_started),
        RADIO_OK
    );
    assert_eq!(playback_started, 1);
    assert_eq!(
        radio_native_parrot_play_f32(radio, output.as_mut_ptr(), 2, &mut played),
        RADIO_OK
    );
    assert_eq!(played, 2);
    assert_eq!(&output[..2], &input[..2]);
    assert_eq!(&output[2..], &[-1.0_f32; 2]);
    assert_eq!(radio_native_parrot_status(radio, &mut status), RADIO_OK);
    assert_eq!(status.playback_offset, 2);
    assert_eq!(status.playing, 1);

    assert_eq!(
        radio_native_parrot_play_f32(radio, output[2..].as_mut_ptr(), 2, &mut played),
        RADIO_OK
    );
    assert_eq!(played, 2);
    assert_eq!(output, input);
    assert_eq!(radio_native_parrot_status(radio, &mut status), RADIO_OK);
    assert_eq!(status.playback_offset, 4);
    assert_eq!(status.playing, 0);

    output[0] = 0.25;
    assert_eq!(
        radio_native_parrot_play_f32(radio, output.as_mut_ptr(), 1, &mut played),
        RADIO_OK
    );
    assert_eq!(played, 0);
    assert_eq!(output[0], 0.25);

    assert_eq!(
        radio_native_parrot_rx_transition(radio, 0, 1, &mut playback_started),
        RADIO_OK
    );
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 4, 2, &mut recorded),
        RADIO_OK
    );
    assert_eq!(recorded, 2);
    assert_eq!(radio_native_parrot_status(radio, &mut status), RADIO_OK);
    assert_eq!(status.recorded_samples, 2);
    assert_eq!(status.truncated, 1);
    assert_eq!(radio_native_parrot_reset(radio), RADIO_OK);
    assert_eq!(radio_native_parrot_status(radio, &mut status), RADIO_OK);
    assert_eq!(
        (
            status.recorded_samples,
            status.playback_offset,
            status.playing,
            status.truncated
        ),
        (0, 0, 0, 0)
    );
    radio_destroy(radio);
}

#[test]
fn native_parrot_validates_storage_state_and_bounded_callbacks() {
    let radio = create_radio();
    let mut storage = [0.0_f32; 4];
    let input = [0.25_f32; 4];
    let mut playback_started = 99_u32;
    let mut recorded = 99_u32;
    let mut played = 99_u32;
    let mut status = NativeParrotStatus {
        recorded_samples: 0,
        playback_offset: 0,
        playing: 0,
        truncated: 0,
    };

    assert_eq!(
        radio_native_parrot_bind_f32(ptr::null_mut(), storage.as_mut_ptr(), 4),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_bind_f32(radio, ptr::null_mut(), 4),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_bind_f32(radio, storage.as_mut_ptr(), 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 1, 1, &mut recorded),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(recorded, 0);
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 1, 0, &mut recorded),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(recorded, 0);
    assert_eq!(
        radio_native_parrot_bind_f32(radio, storage.as_mut_ptr(), 4),
        RADIO_OK
    );
    assert_eq!(
        radio_native_parrot_rx_transition(radio, 0, 1, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_rx_transition(radio, 0, 0, &mut playback_started),
        RADIO_OK
    );
    assert_eq!(playback_started, 0);
    assert_eq!(
        radio_native_parrot_rx_transition(radio, 1, 1, &mut playback_started),
        RADIO_OK
    );
    assert_eq!(playback_started, 0);
    assert_eq!(
        radio_native_parrot_rx_transition(radio, 1, 0, &mut playback_started),
        RADIO_OK
    );
    assert_eq!(playback_started, 0);
    assert_eq!(
        radio_native_parrot_record_f32(radio, ptr::null(), 1, 4, &mut recorded),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(recorded, 0);
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 5, 4, &mut recorded),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 1, 5, &mut recorded),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 1, 4, &mut recorded),
        RADIO_OK
    );
    assert_eq!(
        radio_native_parrot_bind_f32(radio, storage.as_mut_ptr(), 4),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_rx_transition(radio, 1, 0, &mut playback_started),
        RADIO_OK
    );
    assert_eq!(playback_started, 1);
    assert_eq!(
        radio_native_parrot_play_f32(radio, ptr::null_mut(), 1, &mut played),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(played, 0);
    assert_eq!(
        radio_native_parrot_play_f32(radio, ptr::null_mut(), 0, &mut played),
        RADIO_OK
    );
    assert_eq!(played, 0);
    assert_eq!(
        radio_native_parrot_play_f32(radio, storage.as_mut_ptr(), 5, &mut played),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(
        radio_native_parrot_play_f32(radio, storage.as_mut_ptr(), 1, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_status(ptr::null(), &mut status),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_status(radio, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(radio_native_parrot_reset(radio), RADIO_OK);
    assert_eq!(
        radio_native_parrot_record_f32(radio, input.as_ptr(), 1, 0, &mut recorded),
        RADIO_OK
    );
    assert_eq!(recorded, 0);
    assert_eq!(
        radio_native_parrot_bind_f32(radio, storage.as_mut_ptr(), 4),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(radio_native_parrot_reset(radio), RADIO_OK);
    assert_eq!(
        radio_native_parrot_record_f32(radio, ptr::null(), 0, 4, &mut recorded),
        RADIO_OK
    );
    assert_eq!(recorded, 0);
    radio_destroy(radio);
}

#[test]
fn calibrated_test_tone_matches_legacy_pcm_amplitude_phase_and_reset() {
    const FRAME_COUNT: usize = 48;
    const PEAK_PCM_CODES: f64 = 7_518.0;
    const PCM_CODE_SCALE: f64 = 32_767.0;
    const STEP: f64 = std::f64::consts::TAU * 1_000.0 / 48_000.0;
    let contiguous = create_radio_with_maximum_frame_count(FRAME_COUNT as u32);
    let fragmented = create_radio_with_maximum_frame_count(FRAME_COUNT as u32);
    let mut full_block = [0.0_f32; FRAME_COUNT];
    let mut fragmented_block = [0.0_f32; FRAME_COUNT];
    let mut expected_phase = 0.0_f64;
    let mut reported_phase = -1.0_f64;

    assert_eq!(
        radio_render_calibrated_test_tone_f32(
            contiguous,
            full_block.as_mut_ptr(),
            FRAME_COUNT as u32,
            1,
        ),
        RADIO_OK
    );
    assert_eq!(
        radio_render_calibrated_test_tone_f32(fragmented, fragmented_block.as_mut_ptr(), 17, 1,),
        RADIO_OK
    );
    assert_eq!(
        radio_render_calibrated_test_tone_f32(
            fragmented,
            fragmented_block[17..].as_mut_ptr(),
            (FRAME_COUNT - 17) as u32,
            1,
        ),
        RADIO_OK
    );
    assert_eq!(full_block, fragmented_block);

    for sample in full_block {
        let expected = (PEAK_PCM_CODES * expected_phase.sin() / PCM_CODE_SCALE) as f32;
        assert_eq!(sample.to_bits(), expected.to_bits());
        expected_phase += STEP;
        if expected_phase >= std::f64::consts::TAU {
            expected_phase -= std::f64::consts::TAU;
        }
    }
    assert_eq!(full_block[0], 0.0);
    assert_eq!(
        radio_calibrated_test_tone_phase_radians(contiguous, &mut reported_phase),
        RADIO_OK
    );
    assert_eq!(reported_phase.to_bits(), expected_phase.to_bits());

    let mut preserved_program = [0.125_f32; FRAME_COUNT];
    assert_eq!(
        radio_render_calibrated_test_tone_f32(
            contiguous,
            preserved_program.as_mut_ptr(),
            FRAME_COUNT as u32,
            0,
        ),
        RADIO_OK
    );
    assert_eq!(preserved_program, [0.125_f32; FRAME_COUNT]);
    assert_eq!(
        radio_calibrated_test_tone_phase_radians(contiguous, &mut reported_phase),
        RADIO_OK
    );
    assert_eq!(reported_phase, 0.0);
    assert_eq!(
        radio_render_calibrated_test_tone_f32(contiguous, preserved_program.as_mut_ptr(), 1, 1),
        RADIO_OK
    );
    assert_eq!(preserved_program[0], 0.0);

    radio_destroy(contiguous);
    radio_destroy(fragmented);
}

#[test]
fn calibrated_test_tone_validates_radio_rate_bounds_and_buffers() {
    let radio = create_radio();
    let mut output = [0.0_f32; 4];
    let mut phase = 0.0_f64;

    assert_eq!(
        radio_render_calibrated_test_tone_f32(ptr::null_mut(), output.as_mut_ptr(), 1, 1),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_render_calibrated_test_tone_f32(radio, ptr::null_mut(), 1, 1),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_render_calibrated_test_tone_f32(radio, output.as_mut_ptr(), 5, 1),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(
        radio_render_calibrated_test_tone_f32(radio, ptr::null_mut(), 0, 1),
        RADIO_OK
    );
    assert_eq!(
        radio_render_calibrated_test_tone_f32(radio, ptr::null_mut(), 0, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_calibrated_test_tone_phase_radians(ptr::null(), &mut phase),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_calibrated_test_tone_phase_radians(radio, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    radio_destroy(radio);
}

#[test]
fn receive_extract_preserves_left_channel_measurement_and_delay_order() {
    let radio = create_radio_with_maximum_frame_count(3);
    let positive_rail = i16::MAX as f32 / 32_768.0;
    let stereo = [positive_rail, 0.25, -1.0, -0.25, 100.0 / 32_768.0, 0.5];
    let mut mono = [0.0_f32; 3];
    let mut delay = [10.0 / 32_768.0, 20.0 / 32_768.0];
    let mut delay_index = 9_u32;
    let mut stats = ReceiveExtractStats {
        peak: -1.0,
        rail_samples: u64::MAX,
    };

    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo.as_ptr(),
            mono.as_mut_ptr(),
            3,
            delay.as_mut_ptr(),
            delay.len() as u32,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_OK
    );
    assert_eq!(stats.peak, 1.0);
    assert_eq!(stats.rail_samples, 2);
    assert_eq!(delay_index, 1);
    assert_eq!(mono, [10.0 / 32_768.0, 20.0 / 32_768.0, positive_rail]);
    assert_eq!(delay, [100.0 / 32_768.0, -1.0]);

    delay_index = 0;
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo[4..].as_ptr(),
            mono.as_mut_ptr(),
            1,
            ptr::null_mut(),
            0,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_OK
    );
    assert_eq!(mono[0], 100.0 / 32_768.0);
    assert_eq!(stats.peak, 100.0 / 32_768.0);
    assert_eq!(stats.rail_samples, 0);
    assert_eq!(delay_index, 0);
    radio_destroy(radio);
}

#[test]
fn receive_extract_validates_f32_buffers_and_bounds() {
    let radio = create_radio();
    let stereo = [0.0_f32; 8];
    let mut mono = [1.0_f32; 4];
    let mut delay = [0.0_f32; 1];
    let mut delay_index = 0_u32;
    let mut stats = ReceiveExtractStats {
        peak: 9.0,
        rail_samples: 9,
    };

    assert_eq!(
        radio_extract_receive_f32(
            ptr::null(),
            stereo.as_ptr(),
            mono.as_mut_ptr(),
            1,
            ptr::null_mut(),
            0,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo.as_ptr(),
            mono.as_mut_ptr(),
            1,
            ptr::null_mut(),
            0,
            &mut delay_index,
            ptr::null_mut(),
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo.as_ptr(),
            mono.as_mut_ptr(),
            5,
            ptr::null_mut(),
            0,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(stats.peak, 0.0);
    assert_eq!(stats.rail_samples, 0);
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            ptr::null(),
            mono.as_mut_ptr(),
            1,
            ptr::null_mut(),
            0,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo.as_ptr(),
            ptr::null_mut(),
            1,
            ptr::null_mut(),
            0,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo.as_ptr(),
            mono.as_mut_ptr(),
            1,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut stats,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo.as_ptr(),
            mono.as_mut_ptr(),
            1,
            ptr::null_mut(),
            1,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            ptr::null(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            &mut stats,
        ),
        RADIO_OK
    );
    assert_eq!(stats.peak, 0.0);
    assert_eq!(stats.rail_samples, 0);

    delay_index = 0;
    assert_eq!(
        radio_extract_receive_f32(
            radio,
            stereo.as_ptr(),
            mono.as_mut_ptr(),
            1,
            delay.as_mut_ptr(),
            1,
            &mut delay_index,
            &mut stats,
        ),
        RADIO_OK
    );
    assert_eq!(delay_index, 0);
    radio_destroy(radio);
}

fn assert_dcs_word_starts(samples: &[f32], word: u32, sample_rate_hz: u32, inverted: bool) {
    let mut accumulator = 0_u32;
    let mut index = 0_usize;
    let clock_limit = sample_rate_hz * 10;

    for phase in 0..23 {
        let mut expected_positive = (word >> phase) & 1 != 0;
        if inverted {
            expected_positive = !expected_positive;
        }
        assert!(index < samples.len());
        assert_eq!(samples[index].is_sign_positive(), expected_positive);
        loop {
            accumulator += 1_344;
            index += 1;
            if accumulator >= clock_limit {
                accumulator -= clock_limit;
                break;
            }
        }
    }
}

#[test]
fn dcs_generation_matches_legacy_words_clock_and_polarity() {
    const SAMPLE_RATE_HZ: u32 = 48_000;
    const PEAK: f32 = 2_000.0 / 32_767.0;
    const WORD_023N: u32 = 0x763813;
    const WORD_431N: u32 = 0x6c5919;
    let radio = create_radio_with_maximum_frame_count(9_000);
    let inverse = create_radio_with_maximum_frame_count(9_000);
    let mut output = vec![0.0_f32; 9_000];
    let mut inverse_output = vec![0.0_f32; 9_000];

    assert_eq!(dcs_transmit::test_wire_word(0o023), WORD_023N);
    assert_eq!(dcs_transmit::test_wire_word(0o431), WORD_431N);
    assert_eq!(radio_dcs_code_supported(-1), 0);
    assert_eq!(radio_dcs_code_supported(0o000), 1);
    assert_eq!(radio_dcs_code_supported(0o777), 1);
    assert_eq!(radio_dcs_code_supported(0o1000), 0);
    assert_eq!(radio_dcs_configure_transmit(radio, 0o023, 0), RADIO_OK);
    assert_eq!(
        radio_dcs_generate_f32(radio, output.as_mut_ptr(), 9_000, PEAK, 1, 0),
        RADIO_OK
    );
    assert_dcs_word_starts(&output, WORD_023N, SAMPLE_RATE_HZ, false);
    assert!(output.iter().all(|sample| sample.abs() == PEAK));

    assert_eq!(radio_dcs_configure_transmit(inverse, 0o431, 1), RADIO_OK);
    assert_eq!(
        radio_dcs_generate_f32(inverse, inverse_output.as_mut_ptr(), 9_000, PEAK, 1, 0),
        RADIO_OK
    );
    assert_dcs_word_starts(&inverse_output, WORD_431N, SAMPLE_RATE_HZ, true);
    radio_destroy(radio);
    radio_destroy(inverse);
}

#[test]
fn dcs_generation_is_callback_partition_invariant_and_configuration_preserves_word_phase() {
    const PEAK: f32 = 2_000.0 / 32_767.0;
    const WORD_431N: u32 = 0x6c5919;
    let contiguous = create_radio_with_maximum_frame_count(1_000);
    let fragmented = create_radio_with_maximum_frame_count(1_000);
    let reconfigured = create_radio_with_maximum_frame_count(512);
    let mut one_block = [0.0_f32; 1_000];
    let mut fragments = [0.0_f32; 1_000];
    let mut advance = [0.0_f32; 358];
    let mut after_configuration = [0.0_f32; 1];

    assert_eq!(radio_dcs_configure_transmit(contiguous, 0o023, 0), RADIO_OK);
    assert_eq!(radio_dcs_configure_transmit(fragmented, 0o023, 0), RADIO_OK);
    assert_eq!(
        radio_dcs_generate_f32(contiguous, one_block.as_mut_ptr(), 1_000, PEAK, 1, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_generate_f32(fragmented, fragments.as_mut_ptr(), 479, PEAK, 1, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_generate_f32(fragmented, fragments[479..].as_mut_ptr(), 521, PEAK, 1, 0,),
        RADIO_OK
    );
    assert_eq!(one_block, fragments);

    assert_eq!(
        radio_dcs_configure_transmit(reconfigured, 0o023, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_generate_f32(reconfigured, advance.as_mut_ptr(), 358, PEAK, 1, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_configure_transmit(reconfigured, 0o431, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_generate_f32(
            reconfigured,
            after_configuration.as_mut_ptr(),
            1,
            PEAK,
            1,
            0,
        ),
        RADIO_OK
    );
    assert_eq!(
        after_configuration[0].is_sign_positive(),
        (WORD_431N >> 1) & 1 != 0
    );
    radio_destroy(contiguous);
    radio_destroy(fragmented);
    radio_destroy(reconfigured);
}

#[test]
fn dcs_turnoff_and_disabled_output_preserve_legacy_phase_behavior() {
    const PEAK: f32 = 2_000.0 / 32_767.0;
    let radio = create_radio_with_maximum_frame_count(48_000);
    let mut tail = vec![0.0_f32; 48_000];
    let mut silent = [1.0_f32; 4];
    let mut phase = -1.0;
    let mut held_phase = -1.0;

    assert_eq!(radio_dcs_configure_transmit(radio, 0o023, 0), RADIO_OK);
    assert_eq!(
        radio_dcs_generate_f32(radio, tail.as_mut_ptr(), 48_000, PEAK, 1, 1),
        RADIO_OK
    );
    assert_eq!(tail[0], 0.0);
    assert!(tail.iter().copied().fold(0.0_f32, f32::max) > PEAK * 0.999);
    assert!(tail.iter().copied().fold(0.0_f32, f32::min) < -PEAK * 0.999);
    assert_eq!(radio_dcs_tail_phase_radians(radio, &mut phase), RADIO_OK);
    assert!((phase - 0.8 * std::f64::consts::PI).abs() < 1.0e-9);

    assert_eq!(
        radio_dcs_generate_f32(radio, silent.as_mut_ptr(), 4, PEAK, 0, 0),
        RADIO_OK
    );
    assert_eq!(silent, [0.0; 4]);
    assert_eq!(
        radio_dcs_tail_phase_radians(radio, &mut held_phase),
        RADIO_OK
    );
    assert_eq!(phase, held_phase);
    assert_eq!(
        radio_dcs_generate_f32(radio, silent.as_mut_ptr(), 4, 0.0, 1, 0),
        RADIO_OK
    );
    assert_eq!(silent, [0.0; 4]);
    radio_destroy(radio);
}

#[test]
fn dcs_generation_validates_bounds_and_preserves_invalid_configuration_outcome() {
    const PEAK: f32 = 2_000.0 / 32_767.0;
    let radio = create_radio();
    let invalid = create_radio();
    let mut output = [0.0_f32; 4];
    let mut phase = 0.0;

    assert_eq!(
        radio_dcs_configure_transmit(ptr::null_mut(), 0o023, 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_dcs_generate_f32(ptr::null_mut(), output.as_mut_ptr(), 1, PEAK, 1, 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_dcs_generate_f32(radio, ptr::null_mut(), 1, PEAK, 1, 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_dcs_generate_f32(radio, output.as_mut_ptr(), 5, PEAK, 1, 0),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(
        radio_dcs_generate_f32(radio, ptr::null_mut(), 0, PEAK, 1, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_generate_f32(radio, ptr::null_mut(), 0, PEAK, 0, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_tail_phase_radians(ptr::null(), &mut phase),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_dcs_tail_phase_radians(radio, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );

    assert_eq!(radio_dcs_configure_transmit(invalid, 0o1000, 0), RADIO_OK);
    assert_eq!(
        radio_dcs_generate_f32(invalid, output.as_mut_ptr(), 1, PEAK, 1, 0),
        RADIO_OK
    );
    assert_eq!(
        output[0].is_sign_positive(),
        dcs_transmit::test_wire_word(-1) & 1 != 0
    );
    radio_destroy(radio);
    radio_destroy(invalid);
}

#[test]
fn ctcss_calibration_matches_legacy_tables_and_q8_truncation() {
    assert_eq!(radio_ctcss_frequency_supported(67.0), 1);
    assert_eq!(radio_ctcss_frequency_supported(250.3), 1);
    assert_eq!(radio_ctcss_frequency_supported(100.0001), 0);
    assert!((radio_ctcss_legacy_frequency(114.8) - 114.746_093_75).abs() < 1.0e-12);
    assert_eq!(radio_ctcss_legacy_peak(114.8, 0), 17_083.0);
    assert_eq!(radio_ctcss_legacy_peak(114.8, 1), 18_417.0);
    assert_eq!(radio_ctcss_legacy_scaled_peak(114.8, 0, 102, 252), 6_699.0);

    let mut amplitude = 0.0;
    let mut bias = 0.0;
    assert_eq!(
        radio_ctcss_legacy_scaled_levels(85.4, 1, 256, 256, &mut amplitude, &mut bias),
        RADIO_OK
    );
    assert_eq!((amplitude, bias), (17_886.0, 1.0));
    assert_eq!(
        radio_ctcss_legacy_scaled_levels(100.0, 0, 256, 256, &mut amplitude, &mut bias),
        RADIO_OK
    );
    assert_eq!((amplitude, bias), (16_951.5, -0.5));
    assert_eq!(
        radio_ctcss_legacy_scaled_levels(100.0, 0, 256, 256, ptr::null_mut(), &mut bias),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_legacy_scaled_levels(100.0, 0, 256, 256, &mut amplitude, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
}

#[test]
fn ctcss_generation_retains_legacy_phase_and_tail_behavior() {
    let normal = create_radio_with_maximum_frame_count(8);
    let shifted = create_radio_with_maximum_frame_count(8);
    let tail = create_radio_with_maximum_frame_count(48_000);
    let mut output = [9.0_f32; 8];
    let mut shifted_output = [0.0_f32; 1];
    let mut tail_output = vec![0.0_f32; 48_000];
    let mut phase = 0.0;
    let mut shifted_phase = 0.0;

    assert_eq!(
        radio_ctcss_generate_f32(normal, output.as_mut_ptr(), 8, 114.8, 1.0, 1, 0.0),
        RADIO_OK
    );
    assert_eq!(output[0], 0.0);
    assert!((output[1] - 0.015_019_664).abs() < 1.0e-8);
    assert!(output[7] > output[6]);
    assert_eq!(radio_ctcss_phase_radians(normal, &mut phase), RADIO_OK);

    assert_eq!(
        radio_ctcss_generate_f32(normal, output.as_mut_ptr(), 8, 114.8, 1.0, 0, 180.0,),
        RADIO_OK
    );
    assert_eq!(output, [0.0; 8]);
    assert_eq!(
        radio_ctcss_phase_radians(normal, &mut shifted_phase),
        RADIO_OK
    );
    assert_eq!(phase, shifted_phase);

    assert_eq!(
        radio_ctcss_generate_f32(shifted, output.as_mut_ptr(), 8, 114.8, 1.0, 1, 0.0),
        RADIO_OK
    );

    assert_eq!(
        radio_ctcss_generate_f32(normal, output.as_mut_ptr(), 1, 114.8, 1.0, 1, 0.0),
        RADIO_OK
    );
    assert_eq!(
        radio_ctcss_generate_f32(
            shifted,
            shifted_output.as_mut_ptr(),
            1,
            114.8,
            1.0,
            1,
            360.0 * 170.0 / 256.0,
        ),
        RADIO_OK
    );
    assert_eq!(radio_ctcss_phase_radians(normal, &mut phase), RADIO_OK);
    assert_eq!(
        radio_ctcss_phase_radians(shifted, &mut shifted_phase),
        RADIO_OK
    );
    let phase_difference =
        (shifted_phase - phase + 2.0 * std::f64::consts::PI) % (2.0 * std::f64::consts::PI);
    assert!((phase_difference - 2.0 * std::f64::consts::PI * 170.0 / 256.0).abs() < 1.0e-12);

    assert_eq!(
        radio_ctcss_generate_tail_f32(tail, tail_output.as_mut_ptr(), 48_000, 55.0, 1.0, 1),
        RADIO_OK
    );
    assert_eq!(tail_output[0], 0.0);
    assert!(tail_output[1] > tail_output[0]);
    assert_eq!(radio_ctcss_phase_radians(tail, &mut phase), RADIO_OK);
    assert!(phase.abs() < 1.0e-12);
    assert_eq!(
        radio_ctcss_generate_tail_f32(tail, output.as_mut_ptr(), 1, 55.0, 1.0, 0),
        RADIO_OK
    );
    assert_eq!(output[0], 0.0);

    radio_destroy(normal);
    radio_destroy(shifted);
    radio_destroy(tail);
}

#[test]
fn ctcss_generation_validates_radio_rate_bounds_and_output() {
    let radio = create_radio();
    let mut output = [0.0_f32; 4];
    let mut phase = 0.0;

    assert_eq!(
        radio_ctcss_generate_f32(ptr::null_mut(), output.as_mut_ptr(), 1, 100.0, 1.0, 1, 0.0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_generate_f32(radio, ptr::null_mut(), 1, 100.0, 1.0, 1, 0.0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_generate_tail_f32(radio, output.as_mut_ptr(), 5, 55.0, 1.0, 1),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(
        radio_ctcss_generate_f32(radio, ptr::null_mut(), 0, 100.0, 1.0, 1, 0.0),
        RADIO_OK
    );
    assert_eq!(
        radio_ctcss_phase_radians(ptr::null(), &mut phase),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_phase_radians(radio, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    radio_destroy(radio);
}

#[test]
fn create_accepts_only_the_native_48khz_rate() {
    for rate in [1, 8_000, 16_000, 44_100, 47_999, 48_001, 96_000, 192_000] {
        let mut config = valid_config();
        config.native_sample_rate_hz = rate;
        let mut handle = 1_usize as *mut Radio;
        assert_eq!(radio_create(&config, &mut handle), RADIO_UNSUPPORTED);
        assert!(handle.is_null());
    }
    let mut handle = ptr::null_mut();
    assert_eq!(radio_create(&valid_config(), &mut handle), RADIO_OK);
    radio_destroy(handle);
}

#[test]
fn create_rejects_invalid_arguments_and_clears_output_handle() {
    let mut handle = 1_usize as *mut Radio;
    assert_eq!(
        radio_create(ptr::null(), &mut handle),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_create(&valid_config(), ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );

    let mut config = valid_config();
    config.struct_size -= 1;
    assert_eq!(radio_create(&config, &mut handle), RADIO_INVALID_ARGUMENT);
    assert!(handle.is_null());

    let mut config = valid_config();
    config.abi_version += 1;
    assert_eq!(radio_create(&config, &mut handle), RADIO_INVALID_ARGUMENT);

    let mut config = valid_config();
    config.native_sample_rate_hz = 0;
    assert_eq!(radio_create(&config, &mut handle), RADIO_INVALID_ARGUMENT);

    let mut config = valid_config();
    config.maximum_frame_count = 0;
    assert_eq!(radio_create(&config, &mut handle), RADIO_INVALID_ARGUMENT);

    let mut config = valid_config();
    config.interleaved_channels = 1;
    assert_eq!(radio_create(&config, &mut handle), RADIO_UNSUPPORTED);
}

#[test]
fn tick_validates_bounds_and_always_silences_valid_output() {
    let radio = create_radio();
    let input = [0.25_f32; 8];
    let mut output = [1.0_f32; 8];

    assert_eq!(
        radio_tick(ptr::null_mut(), input.as_ptr(), output.as_mut_ptr(), 4),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_tick(radio, input.as_ptr(), ptr::null_mut(), 4),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_tick(radio, input.as_ptr(), output.as_mut_ptr(), 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_tick(radio, input.as_ptr(), output.as_mut_ptr(), 5),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert!(output.iter().all(|sample| *sample == 1.0));

    assert_eq!(
        radio_tick(radio, ptr::null(), output.as_mut_ptr(), 4),
        RADIO_INVALID_ARGUMENT
    );
    assert!(output.iter().all(|sample| *sample == 0.0));

    output.fill(1.0);
    assert_eq!(
        radio_tick(radio, input.as_ptr(), output.as_mut_ptr(), 4),
        RADIO_OK
    );
    assert!(output.iter().all(|sample| *sample == 0.0));
    radio_destroy(radio);
    radio_destroy(ptr::null_mut());
}

#[test]
fn repeat_applies_exact_gain_or_silences_when_muted() {
    let input = [-1.0_f32, -0.25_f32, 0.0_f32, 0.5_f32, 1.0_f32];
    let mut output = [9.0_f32; 5];

    assert_eq!(
        radio_repeat_f32(input.as_ptr(), output.as_mut_ptr(), 5, 0.5, 0),
        RADIO_OK
    );
    assert_eq!(output, [-0.5, -0.125, 0.0, 0.25, 0.5]);

    output.fill(9.0);
    assert_eq!(
        radio_repeat_f32(ptr::null(), output.as_mut_ptr(), 5, 0.5, 1),
        RADIO_OK
    );
    assert_eq!(output, [0.0; 5]);

    let mut in_place = input;
    assert_eq!(
        radio_repeat_f32(in_place.as_ptr(), in_place.as_mut_ptr(), 5, -1.0, 0),
        RADIO_OK
    );
    assert_eq!(in_place, [1.0, 0.25, 0.0, -0.5, -1.0]);
}

#[test]
fn repeat_validates_buffers_and_muting_control() {
    let input = [1.0_f32; 2];
    let mut output = [1.0_f32; 2];

    assert_eq!(
        radio_repeat_f32(ptr::null(), ptr::null_mut(), 0, 1.0, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_repeat_f32(input.as_ptr(), ptr::null_mut(), 1, 1.0, 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_repeat_f32(ptr::null(), output.as_mut_ptr(), 1, 1.0, 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_repeat_f32(input.as_ptr(), output.as_mut_ptr(), 1, 1.0, 2),
        RADIO_INVALID_ARGUMENT
    );
}

#[test]
fn transmit_renderer_matches_the_existing_pcm_code_vectors() {
    let radio = create_radio();
    let scale = transmit_renderer::PCM_CODE_SCALE;
    let program = [40_000.0 / scale, -40_000.0 / scale, 1_000.0 / scale];
    let ctcss = [1.0_f32, -1.0, 0.5];
    let dcs = [0.0_f32; 3];
    let mut config = render_config(3, 2);
    config.ctcss_peak_a = 40_000.0 / scale;
    config.ctcss_peak_b = 40_000.0 / scale;
    let mut stereo = [30_000_i16, -30_000, 0, 0, 10, 20];
    let mut meter = [0_i16; 6];
    let mut rails = 99_u64;

    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            3,
            &config,
            stereo.as_mut_ptr(),
            meter.as_mut_ptr(),
            &mut rails,
        ),
        RADIO_OK
    );
    assert_eq!(rails, 2);
    assert_eq!(
        meter,
        [i16::MAX, i16::MAX, i16::MIN, i16::MIN, 1_000, 1_000]
    );
    assert_eq!(
        stereo,
        [i16::MAX, 2_767, i16::MIN, i16::MIN, 21_010, 20_020]
    );
    radio_destroy(radio);
}

#[test]
fn transmit_renderer_preserves_every_output_route_and_dcs_scaling() {
    let radio = create_radio();
    let scale = transmit_renderer::PCM_CODE_SCALE;
    let program = [1_000.0 / scale];
    let ctcss = [0.5_f32];
    let dcs = [2_000.0 / scale];
    let mut stereo = [0_i16; 2];
    let mut rails = 0_u64;

    let config = render_config(0, 1);
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            ptr::null(),
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &config,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_OK
    );
    assert_eq!(stereo, [0, 1_000]);

    stereo = [0, 0];
    let mut config = render_config(2, 3);
    config.ctcss_peak_a = 16_000.0 / scale;
    config.ctcss_peak_b = 16_000.0 / scale;
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &config,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_OK
    );
    // DCS is already a PCM-code level and is never multiplied by CTCSS peak.
    assert_eq!(stereo, [10_000, 11_000]);

    stereo = [0, 0];
    let config = render_config(4, 0);
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &config,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_OK
    );
    assert_eq!(stereo, [1_000, 0]);
    radio_destroy(radio);
}

#[test]
fn transmit_renderer_validates_control_plane_bounds_and_writes_safe_empty_stats() {
    let radio = create_radio();
    let program = [0.0_f32; 5];
    let ctcss = [0.0_f32; 5];
    let dcs = [0.0_f32; 5];
    let mut stereo = [0_i16; 10];
    let mut rails = 99_u64;
    let config = render_config(1, 2);

    assert_eq!(
        radio_render_transmit_f32(
            ptr::null(),
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &config,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &config,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            ptr::null_mut(),
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            5,
            &config,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(rails, 0);

    let mut small = config;
    small.struct_size -= 1;
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &small,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_INVALID_ARGUMENT
    );
    let invalid_route = render_config(5, 1);
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &invalid_route,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_INVALID_ARGUMENT
    );
    let invalid_route = render_config(1, 5);
    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            1,
            &invalid_route,
            stereo.as_mut_ptr(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_INVALID_ARGUMENT
    );

    assert_eq!(
        radio_render_transmit_f32(
            radio,
            ptr::null(),
            ptr::null(),
            ptr::null(),
            0,
            &config,
            ptr::null_mut(),
            ptr::null_mut(),
            &mut rails,
        ),
        RADIO_OK
    );
    assert_eq!(rails, 0);
    for null_input in [0, 1, 2, 3] {
        let result = match null_input {
            0 => radio_render_transmit_f32(
                radio,
                ptr::null(),
                ctcss.as_ptr(),
                dcs.as_ptr(),
                1,
                &config,
                stereo.as_mut_ptr(),
                ptr::null_mut(),
                &mut rails,
            ),
            1 => radio_render_transmit_f32(
                radio,
                program.as_ptr(),
                ptr::null(),
                dcs.as_ptr(),
                1,
                &config,
                stereo.as_mut_ptr(),
                ptr::null_mut(),
                &mut rails,
            ),
            2 => radio_render_transmit_f32(
                radio,
                program.as_ptr(),
                ctcss.as_ptr(),
                ptr::null(),
                1,
                &config,
                stereo.as_mut_ptr(),
                ptr::null_mut(),
                &mut rails,
            ),
            _ => radio_render_transmit_f32(
                radio,
                program.as_ptr(),
                ctcss.as_ptr(),
                dcs.as_ptr(),
                1,
                &config,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut rails,
            ),
        };
        assert_eq!(result, RADIO_INVALID_ARGUMENT);
    }
    radio_destroy(radio);
}

#[test]
fn transmitter_quantization_matches_legacy_nan_and_rail_saturation_behavior() {
    let radio = create_radio();
    let program = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5 / 32_767.0];
    let ctcss = [f32::from_bits(0x7fc0_0000), 0.0, 0.0, 0.0];
    let dcs = [0.0_f32; 4];
    let config = render_config(2, 2);
    let mut stereo = [0_i16; 8];
    let mut meter = [0_i16; 8];
    let mut rails = 0_u64;

    assert_eq!(
        radio_render_transmit_f32(
            radio,
            program.as_ptr(),
            ctcss.as_ptr(),
            dcs.as_ptr(),
            4,
            &config,
            stereo.as_mut_ptr(),
            meter.as_mut_ptr(),
            &mut rails,
        ),
        RADIO_OK
    );
    assert_eq!(rails, 2);
    assert_eq!(
        meter,
        [
            i16::MAX,
            i16::MAX,
            i16::MAX,
            i16::MAX,
            i16::MIN,
            i16::MIN,
            0,
            0
        ]
    );
    assert_eq!(stereo, [i16::MAX, i16::MAX, 0, 0, 0, 0, 0, 0]);
    radio_destroy(radio);
}

const DCS_023N_WIRE_WORD: u32 = 0x76_3813;
const DCS_431N_WIRE_WORD: u32 = 0x6c_5919;
const DCS_WORD_MASK: u32 = 0x7f_ffff;
const DCS_NATIVE_SAMPLE_RATE_HZ: u32 = 48_000;
const DCS_NATIVE_MAX_SYMBOL_SAMPLES: usize = 358;
const DCS_PHASE_CAPTURE_SAMPLES: usize = 20_000;
const DCS_CAPTURE_AMPLITUDE_PCM: i16 = 4_000;
const DCS_TURNOFF_SHORT_MS: u32 = 80;
const DCS_TURNOFF_VALID_MS: u32 = 100;
const DCS_NORMAL_MS: u32 = 500;
const DCS_TURNOFF_RESET_MS: u32 = 40;
const DCS_DC_BIASED_SIGNAL_PCM: i16 = 500;
const DCS_DC_BIAS_PCM: i16 = 800;

/// The portable ABI is always stereo; a legacy mono capture has its unused
/// right channel cleared while a stereo capture deliberately carries unrelated
/// data on that channel.
#[derive(Clone, Copy, Debug)]
enum DcsCaptureLayout {
    Mono,
    Stereo,
}

fn create_radio_at_rate(sample_rate_hz: u32, maximum_frame_count: u32) -> *mut Radio {
    let mut config = valid_config();
    let mut radio = ptr::null_mut();
    config.native_sample_rate_hz = sample_rate_hz;
    config.maximum_frame_count = maximum_frame_count;
    assert_eq!(radio_create(&config, &mut radio), RADIO_OK);
    radio
}

fn dcs_pcm_capture(
    first_word: u32,
    following_word: u32,
    frame_count: usize,
    sample_rate_hz: u32,
) -> Vec<i16> {
    let mut output = vec![0_i16; frame_count];
    let mut accumulator = 0_u32;
    let mut phase = 0_u32;
    let mut first = true;
    for sample in output.iter_mut().take(frame_count) {
        let word = if first { first_word } else { following_word };
        *sample = if word & (1_u32 << phase) != 0 {
            DCS_CAPTURE_AMPLITUDE_PCM
        } else {
            -DCS_CAPTURE_AMPLITUDE_PCM
        };
        accumulator = accumulator.wrapping_add(1_344);
        if accumulator >= sample_rate_hz * 10 {
            accumulator -= sample_rate_hz * 10;
            phase = (phase + 1) % 23;
            if phase == 0 {
                first = false;
            }
        }
    }
    output
}

fn dcs_interleave_pcm(selected: &[i16], layout: DcsCaptureLayout) -> Vec<f32> {
    let mut output = vec![0.0_f32; selected.len() * 2];
    for (index, sample) in selected.iter().copied().enumerate() {
        output[index * 2] = f32::from(sample) / 32_768.0;
        output[index * 2 + 1] = match layout {
            DcsCaptureLayout::Mono => 0.0,
            DcsCaptureLayout::Stereo => -f32::from(sample) / 32_768.0,
        };
    }
    output
}

fn dcs_interleave_f32(selected: &[f32], layout: DcsCaptureLayout) -> Vec<f32> {
    let mut output = vec![0.0_f32; selected.len() * 2];
    for (index, sample) in selected.iter().copied().enumerate() {
        output[index * 2] = sample;
        output[index * 2 + 1] = match layout {
            DcsCaptureLayout::Mono => 0.0,
            DcsCaptureLayout::Stereo => -sample,
        };
    }
    output
}

fn dcs_stereo_capture(
    first_word: u32,
    following_word: u32,
    frame_count: usize,
    sample_rate_hz: u32,
) -> Vec<f32> {
    dcs_interleave_pcm(
        &dcs_pcm_capture(first_word, following_word, frame_count, sample_rate_hz),
        DcsCaptureLayout::Stereo,
    )
}

fn dcs_turnoff_pcm(frame_count: usize, sample_rate_hz: u32, amplitude: f64) -> Vec<i16> {
    let mut output = vec![0_i16; frame_count];
    let mut phase = 0.0_f64;
    let step = std::f64::consts::TAU * 134.4 / f64::from(sample_rate_hz);
    for sample in output.iter_mut().take(frame_count) {
        *sample = (amplitude * phase.sin()).round() as i16;
        phase += step;
        if phase >= std::f64::consts::TAU {
            phase -= std::f64::consts::TAU;
        }
    }
    output
}

fn dcs_turnoff_stereo(frame_count: usize, sample_rate_hz: u32) -> Vec<f32> {
    dcs_interleave_pcm(
        &dcs_turnoff_pcm(frame_count, sample_rate_hz, 1_000.0),
        DcsCaptureLayout::Stereo,
    )
}

fn dcs_impaired_pcm_capture(
    word: u32,
    frame_count: usize,
    inverted_symbols: u32,
    dc_offset: i16,
    noise_peak: i16,
) -> Vec<i16> {
    let mut output = vec![0_i16; frame_count];
    let mut accumulator = 0_u32;
    let mut phase = 0_u32;
    let mut noise_state = 0x1357_9bdf_u32;
    for sample in &mut output {
        let mut positive = word & (1_u32 << phase) != 0;
        if inverted_symbols & (1_u32 << phase) != 0 {
            positive = !positive;
        }
        noise_state = noise_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        let noise = (((noise_state >> 16) & 0x7ff) as i32 - 1_024) * i32::from(noise_peak) / 1_024;
        *sample = (if positive {
            DCS_CAPTURE_AMPLITUDE_PCM
        } else {
            -DCS_CAPTURE_AMPLITUDE_PCM
        }) + dc_offset
            + noise as i16;
        accumulator = accumulator.wrapping_add(1_344);
        if accumulator >= DCS_NATIVE_SAMPLE_RATE_HZ * 10 {
            accumulator -= DCS_NATIVE_SAMPLE_RATE_HZ * 10;
            phase = (phase + 1) % 23;
        }
    }
    output
}

fn dcs_biased_pcm_capture(word: u32, frame_count: usize) -> Vec<i16> {
    let mut output = vec![0_i16; frame_count];
    let mut accumulator = 0_u32;
    let mut phase = 0_u32;
    for sample in &mut output {
        *sample = DCS_DC_BIAS_PCM
            + if word & (1_u32 << phase) != 0 {
                DCS_DC_BIASED_SIGNAL_PCM
            } else {
                -DCS_DC_BIASED_SIGNAL_PCM
            };
        accumulator = accumulator.wrapping_add(1_344);
        if accumulator >= DCS_NATIVE_SAMPLE_RATE_HZ * 10 {
            accumulator -= DCS_NATIVE_SAMPLE_RATE_HZ * 10;
            phase = (phase + 1) % 23;
        }
    }
    output
}

fn dcs_noise_pcm_capture(frame_count: usize) -> Vec<i16> {
    let mut output = vec![0_i16; frame_count];
    let mut state = 0x71f2_a3c5_u32;
    for sample in &mut output {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        *sample = ((state >> 16) & 0x1fff) as i16 - 4_096;
    }
    output
}

fn dcs_generated_tail_capture(
    sample_rate_hz: u32,
    frame_count: usize,
    layout: DcsCaptureLayout,
) -> Vec<f32> {
    let transmitter = create_radio_at_rate(sample_rate_hz, frame_count as u32);
    let mut tail = vec![0.0_f32; frame_count];
    assert_eq!(
        radio_dcs_configure_transmit(transmitter, 0o023, 0),
        RADIO_OK
    );
    assert_eq!(
        radio_dcs_generate_f32(
            transmitter,
            tail.as_mut_ptr(),
            frame_count as u32,
            1_000.0 / 32_767.0,
            1,
            1,
        ),
        RADIO_OK
    );
    radio_destroy(transmitter);
    dcs_interleave_f32(&tail, layout)
}

fn dcs_samples_for_ms(sample_rate_hz: u32, milliseconds: u32) -> usize {
    (sample_rate_hz as usize * milliseconds as usize) / 1_000
}

fn process_dcs_capture_with_pattern(
    radio: *mut Radio,
    stereo: &[f32],
    maximum_fragment: u32,
    pattern_offset: usize,
) -> u32 {
    assert_eq!(stereo.len() % 2, 0);
    let mut offset = 0_usize;
    let mut pattern_index = pattern_offset;
    let mut valid = 0_u32;
    let fragments = [1_u32, 17, 359, 960, 53, 711, 2, 487];
    while offset < stereo.len() / 2 {
        let requested = fragments[pattern_index % fragments.len()].min(maximum_fragment) as usize;
        let frame_count = requested.min(stereo.len() / 2 - offset);
        assert_eq!(
            radio_dcs_process_receive_f32(
                radio,
                stereo[offset * 2..].as_ptr(),
                frame_count as u32,
                &mut valid,
            ),
            RADIO_OK
        );
        offset += frame_count;
        pattern_index += 1;
    }
    valid
}

fn process_dcs_capture(radio: *mut Radio, stereo: &[f32], maximum_fragment: u32) -> u32 {
    process_dcs_capture_with_pattern(radio, stereo, maximum_fragment, 0)
}

#[test]
fn dcs_receive_preserves_legacy_wire_qualification_and_fragmentation() {
    let contiguous = create_radio_at_rate(48_000, 24_000);
    let fragmented = create_radio_at_rate(48_000, 960);
    let inverse = create_radio_at_rate(48_000, 960);
    let normal = dcs_stereo_capture(DCS_023N_WIRE_WORD, DCS_023N_WIRE_WORD, 24_000, 48_000);
    let inverse_samples = dcs_stereo_capture(
        DCS_023N_WIRE_WORD ^ DCS_WORD_MASK,
        DCS_023N_WIRE_WORD ^ DCS_WORD_MASK,
        24_000,
        48_000,
    );
    let mut valid = 0_u32;

    assert_eq!(radio_dcs_configure_receive(contiguous, 0o023, 0), RADIO_OK);
    assert_eq!(radio_dcs_configure_receive(fragmented, 0o023, 0), RADIO_OK);
    assert_eq!(radio_dcs_configure_receive(inverse, 0o023, 1), RADIO_OK);
    assert_eq!(
        radio_dcs_process_receive_f32(contiguous, normal.as_ptr(), 24_000, &mut valid),
        RADIO_OK
    );
    assert_eq!(valid, 1);
    assert_eq!(process_dcs_capture(fragmented, &normal, 960), 1);
    assert_eq!(process_dcs_capture(inverse, &inverse_samples, 960), 1);
    radio_destroy(contiguous);
    radio_destroy(fragmented);
    radio_destroy(inverse);
}

#[test]
fn dcs_receive_round_trips_every_supported_code_and_polarity_at_native_rate() {
    for code in 0..=0o777 {
        for inverted in [false, true] {
            let word =
                dcs_transmit::test_wire_word(code) ^ if inverted { DCS_WORD_MASK } else { 0 };
            let capture = dcs_interleave_pcm(
                &dcs_pcm_capture(
                    word,
                    word,
                    DCS_PHASE_CAPTURE_SAMPLES,
                    DCS_NATIVE_SAMPLE_RATE_HZ,
                ),
                DcsCaptureLayout::Mono,
            );
            let radio = create_radio_at_rate(DCS_NATIVE_SAMPLE_RATE_HZ, 960);

            assert_eq!(
                radio_dcs_configure_receive(radio, code, u32::from(inverted)),
                RADIO_OK
            );
            assert_eq!(
                process_dcs_capture(radio, &capture, 960),
                1,
                "code {code:03o}, inverted={inverted}"
            );
            assert!(!unsafe { (*radio).dcs_receive.turnoff_active() });
            radio_destroy(radio);
        }
    }
}

#[test]
fn dcs_receive_recovers_every_symbol_phase_through_fragmented_callbacks() {
    let cases = [
        (
            DCS_NATIVE_SAMPLE_RATE_HZ,
            DCS_NATIVE_MAX_SYMBOL_SAMPLES,
            DCS_PHASE_CAPTURE_SAMPLES,
            false,
            DcsCaptureLayout::Mono,
        ),
        (
            DCS_NATIVE_SAMPLE_RATE_HZ,
            DCS_NATIVE_MAX_SYMBOL_SAMPLES,
            DCS_PHASE_CAPTURE_SAMPLES,
            true,
            DcsCaptureLayout::Stereo,
        ),
    ];

    for (sample_rate_hz, maximum_symbol_samples, capture_samples, inverted, layout) in cases {
        let word = dcs_transmit::test_wire_word(0o023) ^ if inverted { DCS_WORD_MASK } else { 0 };
        let source = dcs_interleave_pcm(
            &dcs_pcm_capture(
                word,
                word,
                capture_samples + maximum_symbol_samples,
                sample_rate_hz,
            ),
            layout,
        );
        for offset in 0..maximum_symbol_samples {
            let radio = create_radio_at_rate(sample_rate_hz, 960);
            let begin = offset * 2;
            let end = (offset + capture_samples) * 2;

            assert_eq!(
                radio_dcs_configure_receive(radio, 0o023, u32::from(inverted)),
                RADIO_OK
            );
            assert_eq!(
                process_dcs_capture_with_pattern(radio, &source[begin..end], 960, offset),
                1,
                "rate={sample_rate_hz}, layout={layout:?}, inverted={inverted}, offset={offset}"
            );
            radio_destroy(radio);
        }
    }
}

#[test]
fn dcs_receive_corrects_up_to_three_symbols_through_noise() {
    let error_masks = [
        0,
        1_u32 << 2,
        (1_u32 << 2) | (1_u32 << 9),
        (1_u32 << 2) | (1_u32 << 9) | (1_u32 << 17),
    ];
    let word = dcs_transmit::test_wire_word(0o023);
    for error_mask in error_masks {
        let source = dcs_interleave_pcm(
            &dcs_impaired_pcm_capture(
                word,
                DCS_PHASE_CAPTURE_SAMPLES + DCS_NATIVE_MAX_SYMBOL_SAMPLES,
                error_mask,
                700,
                900,
            ),
            DcsCaptureLayout::Mono,
        );
        let radio = create_radio_at_rate(DCS_NATIVE_SAMPLE_RATE_HZ, 960);
        let offset = DCS_NATIVE_MAX_SYMBOL_SAMPLES / 2;
        let begin = offset * 2;
        let end = (offset + DCS_PHASE_CAPTURE_SAMPLES) * 2;

        assert_eq!(radio_dcs_configure_receive(radio, 0o023, 0), RADIO_OK);
        assert_eq!(
            process_dcs_capture_with_pattern(radio, &source[begin..end], 960, offset),
            1,
            "error mask {error_mask:#08x}"
        );
        radio_destroy(radio);
    }
}

#[test]
fn dcs_receive_tracks_dc_bias_below_the_legacy_integer_quantum() {
    let selected = dcs_biased_pcm_capture(
        dcs_transmit::test_wire_word(0o023),
        DCS_NATIVE_SAMPLE_RATE_HZ as usize * 2,
    );
    let capture = dcs_interleave_pcm(&selected, DcsCaptureLayout::Mono);
    let radio = create_radio_at_rate(DCS_NATIVE_SAMPLE_RATE_HZ, 960);

    assert_eq!(radio_dcs_configure_receive(radio, 0o023, 0), RADIO_OK);
    assert_eq!(process_dcs_capture_with_pattern(radio, &capture, 960, 6), 1);
    radio_destroy(radio);
}

fn assert_dcs_turnoff_at_rate(
    sample_rate_hz: u32,
    layout: DcsCaptureLayout,
    inverted: bool,
    pattern_offset: usize,
) {
    let normal_count = dcs_samples_for_ms(sample_rate_hz, DCS_NORMAL_MS);
    let reset_count = dcs_samples_for_ms(sample_rate_hz, DCS_TURNOFF_RESET_MS);
    let short_count = dcs_samples_for_ms(sample_rate_hz, DCS_TURNOFF_SHORT_MS);
    let tail_count = dcs_samples_for_ms(sample_rate_hz, DCS_TURNOFF_VALID_MS);
    let word = dcs_transmit::test_wire_word(0o023) ^ if inverted { DCS_WORD_MASK } else { 0 };
    let normal = dcs_interleave_pcm(
        &dcs_pcm_capture(word, word, normal_count, sample_rate_hz),
        layout,
    );
    let short_tail = dcs_interleave_pcm(
        &dcs_turnoff_pcm(short_count, sample_rate_hz, 4_000.0),
        layout,
    );
    let tail = dcs_generated_tail_capture(sample_rate_hz, tail_count, layout);
    let radio = create_radio_at_rate(sample_rate_hz, 960);

    assert_eq!(
        radio_dcs_configure_receive(radio, 0o023, u32::from(inverted)),
        RADIO_OK
    );
    assert_eq!(
        process_dcs_capture_with_pattern(radio, &normal, 960, pattern_offset),
        1
    );
    assert!(!unsafe { (*radio).dcs_receive.turnoff_active() });
    assert_eq!(
        process_dcs_capture_with_pattern(
            radio,
            &normal[..reset_count * 2],
            960,
            pattern_offset + 1,
        ),
        1
    );
    assert_eq!(
        process_dcs_capture_with_pattern(radio, &short_tail, 960, pattern_offset + 2),
        1
    );
    assert!(!unsafe { (*radio).dcs_receive.turnoff_active() });
    assert_eq!(
        process_dcs_capture_with_pattern(
            radio,
            &normal[..reset_count * 2],
            960,
            pattern_offset + 3,
        ),
        1
    );
    assert_eq!(
        process_dcs_capture_with_pattern(radio, &tail, 960, pattern_offset + 4),
        0
    );
    assert!(unsafe { (*radio).dcs_receive.turnoff_active() });
    assert_eq!(
        process_dcs_capture_with_pattern(radio, &normal, 960, pattern_offset + 5),
        1
    );
    assert!(!unsafe { (*radio).dcs_receive.turnoff_active() });
    radio_destroy(radio);
}

#[test]
fn dcs_receive_turnoff_detection_matches_native_polarity_and_capture_layout() {
    assert_dcs_turnoff_at_rate(DCS_NATIVE_SAMPLE_RATE_HZ, DcsCaptureLayout::Mono, false, 0);
    assert_dcs_turnoff_at_rate(DCS_NATIVE_SAMPLE_RATE_HZ, DcsCaptureLayout::Stereo, true, 1);
    assert_dcs_turnoff_at_rate(DCS_NATIVE_SAMPLE_RATE_HZ, DcsCaptureLayout::Mono, true, 2);
    assert_dcs_turnoff_at_rate(
        DCS_NATIVE_SAMPLE_RATE_HZ,
        DcsCaptureLayout::Stereo,
        false,
        3,
    );
}

#[test]
fn dcs_receive_rejects_broadband_noise_as_a_turnoff_tone() {
    let normal_count = dcs_samples_for_ms(DCS_NATIVE_SAMPLE_RATE_HZ, DCS_NORMAL_MS);
    let word = dcs_transmit::test_wire_word(0o023);
    let normal = dcs_interleave_pcm(
        &dcs_pcm_capture(word, word, normal_count, DCS_NATIVE_SAMPLE_RATE_HZ),
        DcsCaptureLayout::Mono,
    );
    let noise = dcs_interleave_pcm(
        &dcs_noise_pcm_capture(dcs_samples_for_ms(
            DCS_NATIVE_SAMPLE_RATE_HZ,
            DCS_TURNOFF_VALID_MS,
        )),
        DcsCaptureLayout::Mono,
    );
    let radio = create_radio_at_rate(DCS_NATIVE_SAMPLE_RATE_HZ, 960);

    assert_eq!(radio_dcs_configure_receive(radio, 0o023, 0), RADIO_OK);
    assert_eq!(process_dcs_capture_with_pattern(radio, &normal, 960, 4), 1);
    let _ = process_dcs_capture_with_pattern(radio, &noise, 960, 5);
    assert!(!unsafe { (*radio).dcs_receive.turnoff_active() });
    radio_destroy(radio);
}

#[test]
fn dcs_receive_preserves_legacy_loss_hold_and_turnoff_tail() {
    let radio = create_radio_at_rate(48_000, 960);
    let matching = dcs_stereo_capture(DCS_023N_WIRE_WORD, DCS_023N_WIRE_WORD, 24_000, 48_000);
    let nonmatching = dcs_stereo_capture(DCS_431N_WIRE_WORD, DCS_431N_WIRE_WORD, 24_000, 48_000);
    let short_tail = dcs_turnoff_stereo(3_840, 48_000);
    let tail = dcs_turnoff_stereo(4_800, 48_000);

    assert_eq!(radio_dcs_configure_receive(radio, 0o023, 0), RADIO_OK);
    assert_eq!(process_dcs_capture(radio, &matching, 960), 1);
    assert_eq!(process_dcs_capture(radio, &nonmatching[..8_000], 960), 1);
    assert_eq!(process_dcs_capture(radio, &nonmatching[8_000..], 960), 0);

    assert_eq!(radio_dcs_configure_receive(radio, 0o023, 0), RADIO_OK);
    assert_eq!(process_dcs_capture(radio, &matching, 960), 1);
    assert_eq!(process_dcs_capture(radio, &short_tail, 960), 1);
    assert!(!unsafe { (*radio).dcs_receive.turnoff_active() });
    assert_eq!(process_dcs_capture(radio, &tail, 960), 0);
    assert!(unsafe { (*radio).dcs_receive.turnoff_active() });
    radio_destroy(radio);
}

#[test]
fn dcs_receive_validates_and_disables_like_the_legacy_decoder() {
    let radio = create_radio();
    let single_stereo = [0.0_f32, 0.0];
    let mut valid = u32::MAX;

    assert_eq!(
        radio_dcs_configure_receive(ptr::null_mut(), 0o023, 0),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_dcs_process_receive_f32(ptr::null_mut(), single_stereo.as_ptr(), 1, &mut valid),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_dcs_process_receive_f32(radio, single_stereo.as_ptr(), 1, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(radio_dcs_configure_receive(radio, -1, 1), RADIO_OK);
    assert_eq!(
        radio_dcs_process_receive_f32(radio, ptr::null(), 1, &mut valid),
        RADIO_OK
    );
    assert_eq!(valid, 0);
    assert_eq!(radio_dcs_configure_receive(radio, 0o023, 0), RADIO_OK);
    assert_eq!(
        radio_dcs_process_receive_f32(radio, ptr::null(), 1, &mut valid),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(valid, 0);
    assert_eq!(
        radio_dcs_process_receive_f32(radio, single_stereo.as_ptr(), 5, &mut valid),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(valid, 0);
    assert_eq!(
        radio_dcs_process_receive_f32(radio, ptr::null(), 0, &mut valid),
        RADIO_OK
    );
    assert_eq!(valid, 0);
    assert_eq!(radio_dcs_configure_receive(radio, 0o1000, 0), RADIO_OK);
    assert_eq!(
        radio_dcs_process_receive_f32(radio, ptr::null(), 1, &mut valid),
        RADIO_OK
    );
    assert_eq!(valid, 0);
    radio_destroy(radio);
}

fn ctcss_receive_config(tone_mask: u64, relax: u32) -> CtcssReceiveConfig {
    CtcssReceiveConfig {
        struct_size: size_of::<CtcssReceiveConfig>() as u32,
        tone_mask,
        relax,
    }
}

fn prepare_portable_ctcss_detector(radio: *mut Radio, tone_index: usize) {
    unsafe { (*radio).ctcss_receive.test_prepare_detector(tone_index) };
}

#[test]
fn ctcss_receive_abi_qualifies_and_releases() {
    const TONE: usize = 11;
    let whole = create_radio_with_maximum_frame_count(160);
    let config = ctcss_receive_config(1_u64 << TONE, 0);
    let input = [0.0_f32; 4];
    let mut decoded_whole = i32::MIN;

    assert_eq!(radio_ctcss_configure_receive(whole, &config), RADIO_OK);
    prepare_portable_ctcss_detector(whole, TONE);
    assert_eq!(
        radio_ctcss_process_receive_f32(whole, input.as_ptr(), 4, 1, &mut decoded_whole),
        RADIO_OK
    );
    assert_eq!(decoded_whole, TONE as i32);

    unsafe { (*whole).ctcss_receive.test_set_detector_release(TONE) };
    assert_eq!(
        radio_ctcss_process_receive_f32(whole, input.as_ptr(), 1, 1, &mut decoded_whole),
        RADIO_OK
    );
    assert_eq!(decoded_whole, -1);
    assert_eq!(
        unsafe { (*whole).ctcss_receive.test_blanking_samples() },
        ctcss_receive::SAMPLE_RATE_HZ as i32 / 5
    );
    radio_destroy(whole);
}

#[test]
fn ctcss_receive_abi_validates_arguments_and_carrier_loss() {
    let radio = create_radio_with_maximum_frame_count(1);
    let mut decoded = i32::MIN;
    let input = [100.0_f32 / 32_768.0];
    let valid = ctcss_receive_config(1_u64 << 11, 0);
    let invalid_relax = ctcss_receive_config(1_u64 << 11, 2);

    assert_eq!(
        radio_ctcss_configure_receive(ptr::null_mut(), &valid),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_configure_receive(radio, ptr::null()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_configure_receive(radio, &invalid_relax),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(radio_ctcss_configure_receive(radio, &valid), RADIO_OK);
    prepare_portable_ctcss_detector(radio, 11);
    assert_eq!(
        radio_ctcss_process_receive_f32(radio, input.as_ptr(), 1, 0, &mut decoded),
        RADIO_OK
    );
    assert_eq!(decoded, -1);
    assert_eq!(
        unsafe { (*radio).ctcss_receive.test_detector_decode(11) },
        0
    );
    assert_eq!(
        radio_ctcss_process_receive_f32(radio, ptr::null(), 1, 1, &mut decoded),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(decoded, -1);
    assert_eq!(
        radio_ctcss_process_receive_f32(radio, input.as_ptr(), 2, 1, &mut decoded),
        RADIO_FRAME_COUNT_EXCEEDED
    );
    assert_eq!(
        radio_ctcss_process_receive_f32(radio, input.as_ptr(), 1, 2, &mut decoded),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_process_receive_f32(radio, input.as_ptr(), 1, 1, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    radio_destroy(radio);
}
