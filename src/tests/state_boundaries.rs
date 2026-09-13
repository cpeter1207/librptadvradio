use super::*;

#[test]
fn independent_receive_sources_do_not_admit_wrong_signaling_modes() {
    use receive_qualification::{
        CarrierSource, Config, Inputs, SignalMode, State, SubaudibleSource,
    };
    for source in [
        SubaudibleSource::Dsp,
        SubaudibleSource::HardwareInverted,
        SubaudibleSource::Hardware,
        SubaudibleSource::ParallelInverted,
    ] {
        let config = Config {
            carrier_source: CarrierSource::Ignore,
            subaudible_source: source,
            ..Config::default()
        };
        let result =
            receive_qualification::advance(&config, &Inputs::default(), 1, &mut State::default());
        assert!(!result.carrier_active);
    }
    let config = Config {
        carrier_source: CarrierSource::Hardware,
        subaudible_source: SubaudibleSource::Dsp,
        ..Config::default()
    };
    for mode in [SignalMode::None, SignalMode::Ctcss, SignalMode::Dcs] {
        let mut inputs = Inputs {
            hardware_carrier: true,
            dcs_receive_enabled: true,
            dcs_valid: true,
            signal_mode: mode,
            ..Inputs::default()
        };
        assert_eq!(
            receive_qualification::advance(&config, &inputs, 1, &mut State::default())
                .subaudible_active,
            mode == SignalMode::Dcs
        );
        inputs.dcs_valid = false;
        assert!(
            !receive_qualification::advance(&config, &inputs, 1, &mut State::default())
                .subaudible_active
        );
        inputs.dcs_receive_enabled = false;
        inputs.ctcss_receive_enabled = true;
        inputs.ctcss_decoder_available = false;
        assert!(
            !receive_qualification::advance(&config, &inputs, 1, &mut State::default())
                .subaudible_active
        );
        inputs.ctcss_decoder_available = true;
        inputs.ctcss_decoded = true;
        assert_eq!(
            receive_qualification::advance(&config, &inputs, 1, &mut State::default())
                .subaudible_active,
            mode == SignalMode::Ctcss
        );
    }
    let mut config = SignalModeConfig::disabled();
    let mut state = SignalModeState {
        smode: signal_mode::MODE_CTCSS,
        ..SignalModeState::default()
    };
    let input = SignalModeInput {
        decoded_ctcss: 1,
        ..SignalModeInput::default()
    };
    assert_eq!(
        signal_mode::radio_signal_mode_advance(&config, &input, &mut state),
        RADIO_OK
    );
    assert_eq!(state.last_rx_ctcss, signal_mode::CTCSS_NONE);
    config.ctcss_tx_enabled = 1;
    state.smode = signal_mode::MODE_DCS;
    let input = SignalModeInput {
        decoded_ctcss: -1,
        dcs_valid: 1,
        ..input
    };
    assert_eq!(
        signal_mode::radio_signal_mode_advance(&config, &input, &mut state),
        RADIO_OK
    );
    assert_eq!(state.smode, signal_mode::MODE_DCS);
}

#[test]
fn every_cpu_saver_boolean_is_validated_independently() {
    for field in 0..5 {
        let mut input = RxCpuSaverInput::default();
        match field {
            0 => input.enabled = 2,
            1 => input.carrier_detect = 2,
            2 => input.signal_mode_null = 2,
            3 => input.tx_ptt_in = 2,
            _ => input.tx_ptt_out = 2,
        }
        let mut state = RxCpuSaverState {
            halted: 0,
            action: 99,
        };
        let before = state;
        assert_eq!(
            rx_cpu_saver::radio_rx_cpu_saver_advance(&input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, before);
    }
    for field in 0..4 {
        let mut input = TxCpuSaverInput::default();
        match field {
            0 => input.enabled = 2,
            1 => input.tx_ptt_in = 2,
            2 => input.tx_ptt_out = 2,
            _ => input.tx_idle = 2,
        }
        let mut state = TxCpuSaverState { halted: 1 };
        assert_eq!(
            tx_cpu_saver::radio_tx_cpu_saver_advance(&input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state.halted, 1);
    }
    assert_eq!(
        rx_blanking::radio_rx_blanking_advance(
            &RxBlankingInput::default(),
            &mut RxBlankingState::default()
        ),
        RADIO_INVALID_ARGUMENT
    );
}

#[test]
fn ctcss_render_rejects_each_invalid_state_without_committing() {
    let cfg = CtcssRenderStateConfig::disabled();
    let input = CtcssRenderStateInput::default();
    for field in 0..6 {
        let mut state = CtcssRenderState::default();
        match field {
            0 => state.option = 4,
            1 => state.oscillator_state = 3,
            2 => state.enabled = 2,
            3 => state.turnoff_remaining_ms = -1,
            4 => state.phase_shift_degrees = f64::INFINITY,
            _ => state.tail_tone_hz = f64::INFINITY,
        }
        let before = state;
        assert_eq!(
            ctcss_render_state::radio_ctcss_render_state_advance(&cfg, &input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, before);
    }
    for field in 0..2 {
        let mut invalid = cfg;
        if field == 0 {
            invalid.struct_size = 0;
        } else {
            invalid.turnoff_duration_ms = -1;
        }
        assert_eq!(
            ctcss_render_state::radio_ctcss_render_state_advance(
                &invalid,
                &input,
                &mut CtcssRenderState::default()
            ),
            RADIO_INVALID_ARGUMENT
        );
    }
    let mut idle = CtcssRenderState::default();
    assert_eq!(
        ctcss_render_state::radio_ctcss_render_state_advance(&cfg, &input, &mut idle),
        RADIO_OK
    );
    assert_eq!(idle, CtcssRenderState::default());
    let mut tail = CtcssRenderState {
        oscillator_state: ctcss_render_state::STATE_TURNOFF,
        turnoff_remaining_ms: 0,
        ..CtcssRenderState::default()
    };
    assert_eq!(
        ctcss_render_state::radio_ctcss_render_state_advance(&cfg, &input, &mut tail),
        RADIO_OK
    );
    assert_eq!(tail.option, ctcss_render_state::OPTION_DISABLE);
}

#[test]
fn dcs_tail_rejects_invalid_transitions_and_handles_immediate_expiry() {
    let config = DcsTurnoffConfig {
        turnoff_duration_ms: 1,
    };
    let active = DcsTurnoffState {
        tx_state: dcs_turnoff::STATE_ACTIVE,
        ..DcsTurnoffState::default()
    };
    let input = DcsTurnoffInput {
        elapsed_ms: 0,
        tx_ptt_in: 0,
        begin_turnoff: 1,
    };
    for field in 0..4 {
        let mut invalid = input;
        match field {
            0 => invalid.elapsed_ms = -1,
            1 => invalid.begin_turnoff = 2,
            2 => invalid.tx_ptt_in = 1,
            _ => invalid.begin_turnoff = 0,
        }
        let mut state = active;
        assert_eq!(
            dcs_turnoff::radio_dcs_turnoff_advance(&config, &invalid, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, active);
    }
    let mut state = active;
    state.tx_state = 99;
    assert_eq!(
        dcs_turnoff::radio_dcs_turnoff_advance(&config, &input, &mut state),
        RADIO_INVALID_ARGUMENT
    );
    let mut state = active;
    assert_eq!(
        dcs_turnoff::radio_dcs_turnoff_advance(
            &config,
            &DcsTurnoffInput {
                elapsed_ms: 2,
                ..input
            },
            &mut state
        ),
        RADIO_OK
    );
    assert_eq!(state.finish_requested, 1);
    assert_eq!(state.finish_elapsed_ms, 1);
    for (begin, remaining) in [(1, 1), (0, 0)] {
        let mut state = DcsTurnoffState {
            tx_state: dcs_turnoff::STATE_TOC,
            dcs_turnoff_remaining_ms: remaining,
            ..active
        };
        let before = state;
        assert_eq!(
            dcs_turnoff::radio_dcs_turnoff_advance(
                &config,
                &DcsTurnoffInput {
                    begin_turnoff: begin,
                    ..input
                },
                &mut state
            ),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, before);
    }
}

#[test]
fn finish_continuation_rejects_invalid_state_and_nulls_atomically() {
    let input = TxFinishInput::default();
    let valid = TxFinishState {
        tx_state: tx_finish::STATE_FINISHING,
        buffer_clear_frames: 1,
        finish_remaining_ms: 1,
    };
    assert_eq!(
        tx_finish::radio_tx_finish_continue(ptr::null(), &mut { valid }),
        RADIO_INVALID_ARGUMENT
    );
    assert_eq!(
        tx_finish::radio_tx_finish_continue(&input, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    for field in 0..3 {
        let mut state = valid;
        match field {
            0 => state.tx_state = 0,
            1 => state.buffer_clear_frames = -1,
            _ => state.finish_remaining_ms = -1,
        }
        let before = state;
        assert_eq!(
            tx_finish::radio_tx_finish_continue(&input, &mut state),
            RADIO_INVALID_ARGUMENT
        );
        assert_eq!(state, before);
    }
}

#[test]
fn legacy_generators_and_meter_cover_zero_span_wrap_and_corrupt_ring_index() {
    let radio = create_radio_with_maximum_frame_count(100);
    let mut output = [99.0_f32; 100];
    assert_eq!(
        radio_render_calibrated_test_tone_f32(radio, output.as_mut_ptr(), 100, 1),
        RADIO_OK
    );
    let mut phase = 0.0;
    assert_eq!(
        radio_calibrated_test_tone_phase_radians(radio, &mut phase),
        RADIO_OK
    );
    assert!(phase > 0.0 && phase < std::f64::consts::TAU);
    assert_eq!(
        radio_native_parrot_record_f32(radio, ptr::null(), 0, 0, ptr::null_mut()),
        RADIO_INVALID_ARGUMENT
    );
    for (frequency, peak, count) in [(0.0, 1.0, 1), (100.0, 0.0, 1), (100.0, 1.0, 0)] {
        unsafe {
            ctcss_transmit::generate(
                &mut phase,
                output.as_mut_ptr(),
                count,
                frequency,
                peak,
                u32::from(count != 0),
                0.0,
            )
        };
    }
    for index in [i16::MAX, 0] {
        let mut stats = AudioStatistics {
            maxbuf: [0; AUDIO_STATS_LEN],
            clipbuf: [0; AUDIO_STATS_LEN],
            pwrbuf: [0; AUDIO_STATS_LEN],
            index,
        };
        let samples = [0.01_f32; 6];
        assert_eq!(
            audio_meter::measure(samples.as_ptr(), 6, 1, &mut stats),
            Ok(false)
        );
        assert_eq!(stats.index, 1);
        assert_eq!(stats.clipbuf[0], 0);
    }
    let mut stats = AudioStatistics {
        maxbuf: [0; AUDIO_STATS_LEN],
        clipbuf: [0; AUDIO_STATS_LEN],
        pwrbuf: [0; AUDIO_STATS_LEN],
        index: i16::MAX,
    };
    assert_eq!(
        audio_meter::measure(ptr::null(), 0, 1, &mut stats),
        Ok(false)
    );
    assert_eq!(stats.index, 1);
    radio_destroy(radio);
}
