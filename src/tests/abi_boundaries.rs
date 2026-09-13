//! Public descriptor boundary regressions: rejection must not corrupt state.
use super::*;

#[test]
fn parrot_and_receive_boundaries_keep_status_destinations_deterministic() {
    let radio = create_radio();
    let mut result = 99;
    assert_eq!(
        radio_native_parrot_reset(ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_rx_transition(radio, 1, 0, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_native_parrot_rx_transition(ptr::null_mut(), 1, 0, &mut result),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(result, 0);
    assert_eq!(
        radio_ctcss_process_receive_f32(ptr::null_mut(), ptr::null(), 0, 0, &mut -1),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_ctcss_process_receive_f32(radio, ptr::null(), 0, 0, &mut -1),
        RADIO_OK
    );
    let mut cfg = ctcss_receive_config(1, 0);
    cfg.struct_size = 0;
    assert_eq!(
        radio_ctcss_configure_receive(radio, &cfg),
        RADIO_INVALID_ARGUMENT
    );
    cfg.struct_size = size_of::<CtcssReceiveConfig>() as u32;
    assert_eq!(radio_ctcss_configure_receive(radio, &cfg), RADIO_OK);
    assert_eq!(
        radio_ctcss_process_receive_f32(radio, ptr::null(), 0, 1, &mut -1),
        RADIO_OK
    );
    radio_destroy(radio);
}

#[test]
fn scalar_descriptor_wrappers_reject_null_outputs_and_run_valid_operations() {
    let mut elapsed = 99;
    assert_eq!(
        radio_elapsed_ms(&mut 0, 1, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_timer_consume(ptr::null_mut(), 1, &mut elapsed),
        RADIO_INVALID_ARGUMENT
    );
    let mut sq = micor_squelch::State::default();
    let mut closed = 99;
    assert_eq!(
        radio_micor_squelch_update(ptr::null_mut(), 0, 0.0, 1, 1, &mut closed),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_micor_squelch_update(&mut sq, 0, 0.0, 1, 1, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_micor_squelch_update(&mut sq, 0, 0.0, 1, 1, &mut closed),
        RADIO_OK
    );
    assert!(closed <= 1);
    let mut stats = AudioStatistics {
        maxbuf: [0; AUDIO_STATS_LEN],
        clipbuf: [0; AUDIO_STATS_LEN],
        pwrbuf: [0; AUDIO_STATS_LEN],
        index: 0,
    };
    assert_eq!(
        radio_measure_raw_pcm_f32(ptr::null(), 0, 1, ptr::null_mut(), &mut closed),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_measure_raw_pcm_f32(ptr::null(), 0, 1, &mut stats, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_measure_raw_pcm_f32(ptr::null(), 6, 1, &mut stats, &mut closed),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(closed, 0);
}

#[test]
fn envelope_wrapper_preserves_state_on_rejection_and_matches_valid_measurement() {
    let mut state = envelope_meter::State::default();
    let input = [0.25, -0.25];
    let mut output = [9.0; 2];
    let mut comparator = 99;
    assert_eq!(
        radio_measure_envelope_f32(
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            1,
            1,
            ptr::null_mut(),
            &mut comparator
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_measure_envelope_f32(
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            1,
            1,
            &mut state,
            ptr::null_mut()
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_measure_envelope_f32(
            ptr::null(),
            output.as_mut_ptr(),
            2,
            1,
            1,
            &mut state,
            &mut comparator
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(comparator, 0);
    assert_eq!(output, [9.0; 2]);
    assert_eq!(
        radio_measure_envelope_f32(
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            1,
            1,
            &mut state,
            &mut comparator
        ),
        RADIO_OK
    );
    assert_eq!(comparator, 1);
    assert_ne!(output, [9.0; 2]);
    assert_eq!(
        radio_measure_envelope_f32(
            input.as_ptr(),
            ptr::null_mut(),
            2,
            1,
            1,
            &mut state,
            &mut comparator
        ),
        RADIO_OK
    );
    assert_eq!(
        radio_measure_envelope_f32(
            ptr::null(),
            ptr::null_mut(),
            0,
            1,
            1,
            &mut state,
            &mut comparator
        ),
        RADIO_OK
    );
}

#[test]
fn delay_descriptor_validates_each_pointer_and_boolean_without_modifying_outputs() {
    let input = [0.25, -0.25];
    let mut output = [9.0; 2];
    let mut storage = [0.0; 2];
    let mut state = delay_line::State::default();
    assert_eq!(
        radio_delay_line_f32(
            ptr::null(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            2,
            3,
            &mut state,
            0,
            0
        ),
        RADIO_INVALID_ARGUMENT
    );
    for (enabled, outzero) in [(2, 0), (0, 2)] {
        assert_eq!(
            radio_delay_line_f32(
                input.as_ptr(),
                output.as_mut_ptr(),
                2,
                storage.as_mut_ptr(),
                2,
                0,
                &mut state,
                enabled,
                outzero
            ),
            RADIO_INVALID_ARGUMENT
        );
    }
    assert_eq!(
        radio_delay_line_f32(
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            storage.as_mut_ptr(),
            2,
            0,
            ptr::null_mut(),
            1,
            0
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_delay_line_f32(
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            storage.as_mut_ptr(),
            0,
            0,
            &mut state,
            1,
            0
        ),
        RADIO_INVALID_ARGUMENT
    );
    for which in 0..3 {
        assert_eq!(
            radio_delay_line_f32(
                if which == 0 {
                    ptr::null()
                } else {
                    input.as_ptr()
                },
                if which == 1 {
                    ptr::null_mut()
                } else {
                    output.as_mut_ptr()
                },
                2,
                if which == 2 {
                    ptr::null_mut()
                } else {
                    storage.as_mut_ptr()
                },
                2,
                0,
                &mut state,
                1,
                0
            ),
            RADIO_INVALID_ARGUMENT
        );
    }
    assert_eq!(output, [9.0; 2]);
    assert_eq!(
        radio_delay_line_f32(
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            storage.as_mut_ptr(),
            2,
            0,
            &mut state,
            1,
            0
        ),
        RADIO_OK
    );
    assert_eq!(output, input);
    assert_eq!(
        radio_delay_line_f32(
            ptr::null(),
            output.as_mut_ptr(),
            2,
            ptr::null_mut(),
            2,
            0,
            &mut state,
            0,
            0
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_delay_line_f32(
            ptr::null(),
            ptr::null_mut(),
            2,
            storage.as_mut_ptr(),
            2,
            0,
            &mut state,
            0,
            0
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_delay_line_f32(
            ptr::null(),
            ptr::null_mut(),
            0,
            storage.as_mut_ptr(),
            2,
            0,
            &mut state,
            0,
            0
        ),
        RADIO_OK
    );
    assert_eq!(storage, [0.0; 2]);
    assert_eq!(
        radio_delay_line_f32(
            ptr::null(),
            ptr::null_mut(),
            0,
            ptr::null_mut(),
            2,
            0,
            &mut state,
            1,
            0
        ),
        RADIO_OK
    );
}

#[test]
fn slicer_and_deemphasis_descriptors_validate_and_preserve_pcm_behavior() {
    let input = [0.25, -0.25];
    let mut centered = [9.0; 2];
    let mut limited = [9.0; 2];
    let mut slicer = center_slicer::State::default();
    assert_eq!(
        radio_center_slicer_f32(
            input.as_ptr(),
            centered.as_mut_ptr(),
            limited.as_mut_ptr(),
            2,
            2000,
            1000,
            1,
            ptr::null_mut()
        ),
        RADIO_INVALID_ARGUMENT
    );
    for which in 0..3 {
        assert_eq!(
            radio_center_slicer_f32(
                if which == 0 {
                    ptr::null()
                } else {
                    input.as_ptr()
                },
                if which == 1 {
                    ptr::null_mut()
                } else {
                    centered.as_mut_ptr()
                },
                if which == 2 {
                    ptr::null_mut()
                } else {
                    limited.as_mut_ptr()
                },
                2,
                2000,
                1000,
                1,
                &mut slicer
            ),
            RADIO_INVALID_ARGUMENT
        );
    }
    assert_eq!(centered, [9.0; 2]);
    assert_eq!(
        radio_center_slicer_f32(
            input.as_ptr(),
            centered.as_mut_ptr(),
            limited.as_mut_ptr(),
            2,
            2000,
            1000,
            1,
            &mut slicer
        ),
        RADIO_OK
    );
    assert!(limited.iter().all(|value| value.abs() <= 2000.0 / 32768.0));
    assert_eq!(
        radio_center_slicer_f32(
            ptr::null(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
            2000,
            1000,
            1,
            &mut slicer
        ),
        RADIO_OK
    );
    let mut deemphasis = deemphasis_integrator::State::default();
    assert_eq!(
        radio_deemphasis_integrator_f32(
            input.as_ptr(),
            centered.as_mut_ptr(),
            2,
            8192,
            0,
            256,
            ptr::null_mut()
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_deemphasis_integrator_f32(
            ptr::null(),
            centered.as_mut_ptr(),
            2,
            8192,
            0,
            256,
            &mut deemphasis
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_deemphasis_integrator_f32(
            input.as_ptr(),
            ptr::null_mut(),
            2,
            8192,
            0,
            256,
            &mut deemphasis
        ),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        radio_deemphasis_integrator_f32(
            input.as_ptr(),
            centered.as_mut_ptr(),
            2,
            8192,
            0,
            256,
            &mut deemphasis
        ),
        RADIO_OK
    );
    assert_eq!(centered, input);
    assert_eq!(
        radio_deemphasis_integrator_f32(
            ptr::null(),
            ptr::null_mut(),
            0,
            8192,
            0,
            256,
            &mut deemphasis
        ),
        RADIO_OK
    );
}

#[test]
fn fir_and_frontend_descriptors_exercise_success_and_validation() {
    let input = [0.25, -0.25];
    let coeffs = [1_i16];
    let mut history = [0_i16];
    let mut output = [9.0; 2];
    assert_eq!(
        radio_fir_mono_f32(
            input.as_ptr(),
            output.as_mut_ptr(),
            2,
            history.as_mut_ptr(),
            1,
            coeffs.as_ptr(),
            256,
            256,
            1
        ),
        RADIO_OK
    );
    assert_eq!(output, input);
    assert_eq!(
        radio_fir_mono_f32(
            ptr::null(),
            output.as_mut_ptr(),
            2,
            history.as_mut_ptr(),
            1,
            coeffs.as_ptr(),
            256,
            256,
            1
        ),
        RADIO_INVALID_ARGUMENT
    );
    let mut gates = [99_u8; 1];
    let mut state = receive_frontend::State::default();
    let mut count = 99;
    let mut updated = 99;
    for which in 0..4 {
        let expected = if which == 3 {
            RADIO_OK
        } else {
            RADIO_INVALID_ARGUMENT
        };
        assert_eq!(
            radio_receive_frontend_f32(
                input.as_ptr(),
                output.as_mut_ptr(),
                1,
                gates.as_mut_ptr(),
                1,
                1,
                history.as_mut_ptr(),
                1,
                coeffs.as_ptr(),
                1,
                256,
                coeffs.as_ptr(),
                1,
                1,
                1,
                1,
                2000,
                200,
                if which == 2 {
                    ptr::null_mut()
                } else {
                    &mut state
                },
                if which == 0 {
                    ptr::null_mut()
                } else {
                    &mut count
                },
                if which == 1 {
                    ptr::null_mut()
                } else {
                    &mut updated
                }
            ),
            expected
        );
    }
    assert_eq!(count, 1);
    assert_eq!(updated, 1);
    assert_eq!(output[0], input[0]);
    assert!(gates[0] <= 1);
}
