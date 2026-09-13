/**
 * @file descriptor_smoke.c
 * @brief Verify the public C descriptor and bounded silent native tick.
 */

#include <assert.h>
#include <string.h>

#include "rptadvradio/rptadvradio.h"

int main(void)
{
	const struct rptadv_radio_descriptor *descriptor = rptadv_radio_descriptor();
	struct rptadv_radio_config config = {
		.struct_size = sizeof(config),
		.abi_version = RPTADV_RADIO_ABI_VERSION,
		.native_sample_rate_hz = 48000,
		.maximum_frame_count = 48000,
		.interleaved_channels = RPTADV_RADIO_CANONICAL_CHANNELS,
	};
	struct rptadv_radio *radio = NULL;
	struct rptadv_radio *shifted_radio = NULL;
	struct rptadv_radio *tail_radio = NULL;
	const float input[8] = { 1.0F, -1.0F, 0.25F, -0.25F, 0.5F, -0.5F, 0.0F, 0.0F };
	float output[8] = { 1.0F, 1.0F, 1.0F, 1.0F, 1.0F, 1.0F, 1.0F, 1.0F };
	const float program[1] = { 1000.0F / 32767.0F };
	const float ctcss[1] = { 0.0F };
	const float dcs[1] = { 0.0F };
	const float legacy_program[3] = { 40000.0F / 32767.0F, -40000.0F / 32767.0F,
		1000.0F / 32767.0F };
	const float legacy_ctcss[3] = { 1.0F, -1.0F, 0.5F };
	const float legacy_dcs[3] = { 0.0F, 0.0F, 0.0F };
	const float dcs_program[1] = { 0.0F };
	const float dcs_ctcss[1] = { 0.0F };
	const float dcs_signal[1] = { 2000.0F / 32767.0F };
	const float dcs_peak = 2000.0F / 32767.0F;
	const float dcs_receive_stereo[2] = { 0.0F, 0.0F };
	float audio_meter_mono[6] = { 0.0F };
	struct rptadv_radio_audio_statistics audio_statistics = { 0 };
	uint32_t audio_clipping = 99U;
	struct rptadv_radio_micor_squelch_state micor_squelch = { 0 };
	uint32_t micor_closed = 0U;
	const float envelope_input[4] = { 100.0F / 32768.0F, -100.0F / 32768.0F, 0.0F, 0.0F };
	float envelope_output[4] = { 0.0F };
	struct rptadv_radio_envelope_state envelope = { 0 };
	uint32_t envelope_comparator = 0U;
	const float delay_input[4] = { 10.0F, 20.0F, 30.0F, 40.0F };
	float delay_output[4] = { 0.0F };
	float delay_storage[5] = { 0.0F };
	struct rptadv_radio_delay_line_state delay_state = { 0 };
	const float center_input[4] = { -1.0F, 32767.0F / 32768.0F, -2000.0F / 32768.0F,
		2000.0F / 32768.0F };
	float center_output[4] = { 0.0F };
	float center_limited[4] = { 0.0F };
	struct rptadv_radio_center_slicer_state center_state = { 0 };
	const float deemphasis_input[4] = { 0.0F, 20000.0F / 32768.0F,
		-10000.0F / 32768.0F, 32767.0F / 32768.0F };
	float deemphasis_output[4] = { 0.0F };
	struct rptadv_radio_deemphasis_integrator_state deemphasis_state = {
		.accumulator = -12345,
	};
	const float fir_input[3] = { -1.0F, -1234.0F / 32768.0F, 0.0F };
	float fir_output[3] = { 0.0F };
	int16_t fir_history[3] = { 1200, -700, 300 };
	const int16_t fir_coefficients[3] = { 10000, -5000, 3000 };
	struct rptadv_radio_transmit_render_config render_config = {
		.struct_size = sizeof(render_config),
		.output_a_route = RPTADV_RADIO_TX_OUTPUT_AUX_VOICE,
		.output_b_route = RPTADV_RADIO_TX_OUTPUT_TONE,
		.ctcss_peak_a = 1.0F,
		.ctcss_bias_a = 0.0F,
		.ctcss_peak_b = 1.0F,
		.ctcss_bias_b = 0.0F,
	};
	int16_t stereo[2] = { 0, 0 };
	int16_t meter[2] = { 0, 0 };
	uint64_t rails = 0;
	int16_t legacy_stereo[6] = { 30000, -30000, 0, 0, 10, 20 };
	int16_t legacy_meter[6] = { 0, 0, 0, 0, 0, 0 };
	float dcs_output[4] = { 0.0F, 0.0F, 0.0F, 0.0F };
	float test_tone_output[8] = { 0.0F };
	float parrot_storage[4] = { 0.0F };
	const float parrot_input[4] = { 0.125F, -0.25F, 0.5F, -0.75F };
	float parrot_output[4] = { -1.0F, -1.0F, -1.0F, -1.0F };
	double dcs_tail_phase = -1.0;
	double test_tone_phase = -1.0;
	uint32_t parrot_playback_started = 0;
	uint32_t parrot_recorded = 0;
	uint32_t parrot_played = 0;
	uint32_t dcs_receive_valid = 99U;
	struct rptadv_radio_native_parrot_status parrot_status = { 0 };
	struct rptadv_radio_signal_mode_config signal_mode_config = {
		.struct_size = sizeof(signal_mode_config),
		.hold_ms = 25,
		.ctcss_tx_enabled = 1U,
		.default_tx_ctcss_frequency_tenths_hz = -10,
		.mapped_tx_ctcss_frequency_tenths_hz = { [4] = 1000 },
	};
	struct rptadv_radio_signal_mode_input signal_mode_input = {
		.elapsed_ms = 0,
		.decoded_ctcss = 4,
		.dcs_valid = 0U,
		.tx_ptt_in = 0U,
	};
	struct rptadv_radio_signal_mode_state signal_mode_state = {
		.last_rx_ctcss = -1,
	};
	struct rptadv_radio_ctcss_render_state_config ctcss_render_config = {
		.struct_size = sizeof(ctcss_render_config),
		.turnoff_duration_ms = 180,
		.turnoff_phase_shift_degrees = 120.0,
		.turnoff_tail_tone_hz = 55.0,
	};
	struct rptadv_radio_ctcss_render_state_input ctcss_render_input = {
		.elapsed_ms = 20,
	};
	struct rptadv_radio_ctcss_render_state ctcss_render_state = {
		.option = RPTADV_RADIO_CTCSS_RENDER_OPTION_TURNOFF,
		.oscillator_state = RPTADV_RADIO_CTCSS_RENDER_ACTIVE,
		.enabled = 1U,
	};
	struct rptadv_radio_tx_finish_input tx_finish_input = {
		.elapsed_ms = 20,
	};
	struct rptadv_radio_tx_finish_state tx_finish_state = {
		.buffer_clear_frames = 99,
		.finish_remaining_ms = 9,
		.tx_state = 1,
	};
	struct rptadv_radio_tx_complete_config tx_complete_config = {
		.struct_size = sizeof(tx_complete_config),
		.txrx_blanking_time_ms = 125,
	};
	struct rptadv_radio_tx_complete_state tx_complete_state = {
		.tx_state = RPTADV_RADIO_TX_STATE_COMPLETE,
		.tx_ptt_out = 1U,
		.tx_ctcss_option = RPTADV_RADIO_CTCSS_RENDER_OPTION_START,
		.txrx_blanking_timer_ms = 9,
		.txrx_blanking_sample_remainder = 17U,
		.tx_ctcss_ready = 0U,
	};
	struct rptadv_radio_vox_carrier_input vox_carrier_input = {
		.detector_active = 1U,
		.hang_time_ms = 40,
		.elapsed_ms = 20,
	};
	struct rptadv_radio_vox_carrier_state vox_carrier_state = {
		.remaining_ms = 7,
		.carrier_detect = 0U,
	};
	struct rptadv_radio_tx_cpu_saver_input tx_cpu_saver_input = {
		.enabled = 1U,
		.tx_ptt_in = 0U,
		.tx_ptt_out = 0U,
		.tx_idle = 1U,
	};
	struct rptadv_radio_tx_cpu_saver_state tx_cpu_saver_state = {
		.halted = 0U,
	};
	struct rptadv_radio_rx_cpu_saver_input rx_cpu_saver_input = {
		.enabled = 1U,
		.carrier_detect = 0U,
		.signal_mode_null = 1U,
		.tx_ptt_in = 0U,
		.tx_ptt_out = 0U,
	};
	struct rptadv_radio_rx_cpu_saver_state rx_cpu_saver_state = {
		.halted = 0U,
		.action = RPTADV_RADIO_RX_CPU_SAVER_ACTION_NONE,
	};
	float ctcss_output[8] = { 0.0F };
	float shifted_ctcss[8] = { 0.0F };
	float tail_ctcss[48000] = { 0.0F };
	const float receive_stereo[6] = { 32767.0F / 32768.0F, 0.0F, -1.0F, 0.0F,
		100.0F / 32768.0F, 0.0F };
	float receive_mono[3] = { 0.0F };
	float receive_delay[2] = { 10.0F / 32768.0F, 20.0F / 32768.0F };
	uint32_t receive_delay_index = 9U;
	struct rptadv_radio_receive_extract_stats receive_stats = { 0 };
	double amplitude = 0.0;
	double bias = 0.0;
	double normal_phase = 0.0;
	double shifted_phase = 0.0;
	double tail_phase = 1.0;
	double phase_difference;

	assert(descriptor != NULL);
	assert(descriptor->struct_size == sizeof(*descriptor));
	assert(descriptor->abi_version == RPTADV_RADIO_ABI_VERSION);
	assert(strcmp(descriptor->capability_name, "rptadv.radio-core") == 0);
	assert(descriptor->radio_create != NULL);
	assert(descriptor->radio_tick != NULL);
	assert(descriptor->radio_repeat_f32 != NULL);
	assert(descriptor->radio_destroy != NULL);
	assert(descriptor->radio_render_transmit_f32 != NULL);
	assert(descriptor->radio_dcs_code_supported != NULL);
	assert(descriptor->radio_dcs_configure_transmit != NULL);
	assert(descriptor->radio_dcs_generate_f32 != NULL);
	assert(descriptor->radio_dcs_tail_phase_radians != NULL);
	assert(descriptor->radio_extract_receive_f32 != NULL);
	assert(descriptor->radio_render_calibrated_test_tone_f32 != NULL);
	assert(descriptor->radio_calibrated_test_tone_phase_radians != NULL);
	assert(descriptor->radio_native_parrot_bind_f32 != NULL);
	assert(descriptor->radio_native_parrot_reset != NULL);
	assert(descriptor->radio_native_parrot_rx_transition != NULL);
	assert(descriptor->radio_native_parrot_record_f32 != NULL);
	assert(descriptor->radio_native_parrot_play_f32 != NULL);
	assert(descriptor->radio_native_parrot_status != NULL);
	assert(descriptor->radio_ctcss_frequency_supported != NULL);
	assert(descriptor->radio_ctcss_legacy_frequency != NULL);
	assert(descriptor->radio_ctcss_legacy_peak != NULL);
	assert(descriptor->radio_ctcss_legacy_scaled_peak != NULL);
	assert(descriptor->radio_ctcss_legacy_scaled_levels != NULL);
	assert(descriptor->radio_ctcss_generate_f32 != NULL);
	assert(descriptor->radio_ctcss_generate_tail_f32 != NULL);
	assert(descriptor->radio_ctcss_phase_radians != NULL);
	assert(descriptor->radio_dcs_configure_receive != NULL);
	assert(descriptor->radio_dcs_process_receive_f32 != NULL);
	assert(descriptor->radio_measure_raw_pcm_f32 != NULL);
	assert(descriptor->radio_micor_squelch_update != NULL);
	assert(descriptor->radio_measure_envelope_f32 != NULL);
	assert(descriptor->radio_delay_line_f32 != NULL);
	assert(descriptor->radio_center_slicer_f32 != NULL);
	assert(descriptor->radio_deemphasis_integrator_f32 != NULL);
	assert(descriptor->radio_fir_mono_f32 != NULL);
	assert(descriptor->radio_rx_blanking_advance != NULL);
	assert(descriptor->radio_vox_carrier_advance != NULL);
	assert(descriptor->radio_tx_cpu_saver_advance != NULL);
	assert(descriptor->radio_rx_cpu_saver_advance != NULL);
	assert(descriptor->radio_signal_mode_advance != NULL);
	assert(descriptor->radio_signal_mode_advance(&signal_mode_config, &signal_mode_input,
						      &signal_mode_state) == RPTADV_RADIO_OK);
	assert(signal_mode_state.smode == RPTADV_RADIO_SIGNAL_MODE_CTCSS);
	assert(signal_mode_state.smode_timer_ms == 25);
	assert(signal_mode_state.tx_ctcss_frequency_tenths_hz == 1000);
	assert(signal_mode_state.tx_ctcss_option == 1U);
	assert(descriptor->radio_ctcss_render_state_advance != NULL);
	assert(descriptor->radio_ctcss_render_state_advance(
		       &ctcss_render_config, &ctcss_render_input, &ctcss_render_state) ==
	       RPTADV_RADIO_OK);
	assert(ctcss_render_state.option == RPTADV_RADIO_CTCSS_RENDER_OPTION_HOLD);
	assert(ctcss_render_state.oscillator_state == RPTADV_RADIO_CTCSS_RENDER_TURNOFF);
	assert(ctcss_render_state.turnoff_remaining_ms == 160);
	assert(ctcss_render_state.phase_shift_degrees == 120.0);
	assert(ctcss_render_state.tail_tone_hz == 55.0);
	assert(descriptor->radio_tx_finish_advance != NULL);
	assert(descriptor->radio_tx_finish_advance(&tx_finish_input, &tx_finish_state) ==
	       RPTADV_RADIO_OK);
	assert(tx_finish_state.buffer_clear_frames == 3);
	assert(tx_finish_state.finish_remaining_ms == 60);
	assert(tx_finish_state.tx_state == RPTADV_RADIO_TX_STATE_FINISHING);
	assert(descriptor->radio_tx_finish_continue != NULL);
	tx_finish_state.finish_remaining_ms = 20;
	assert(descriptor->radio_tx_finish_continue(&tx_finish_input, &tx_finish_state) ==
	       RPTADV_RADIO_OK);
	assert(tx_finish_state.buffer_clear_frames == 0);
	assert(tx_finish_state.finish_remaining_ms == 0);
	assert(tx_finish_state.tx_state == RPTADV_RADIO_TX_STATE_COMPLETE);
	assert(descriptor->radio_tx_complete != NULL);
	assert(descriptor->radio_tx_complete(&tx_complete_config, &tx_complete_state) ==
	       RPTADV_RADIO_OK);
	assert(tx_complete_state.tx_state == RPTADV_RADIO_TX_STATE_IDLE);
	assert(tx_complete_state.tx_ptt_out == 0U);
	assert(tx_complete_state.tx_ctcss_option == RPTADV_RADIO_CTCSS_RENDER_OPTION_DISABLE);
	assert(tx_complete_state.txrx_blanking_timer_ms == 125);
	assert(tx_complete_state.txrx_blanking_sample_remainder == 0U);
	assert(tx_complete_state.tx_ctcss_ready == 1U);
	assert(descriptor->radio_vox_carrier_advance(&vox_carrier_input, &vox_carrier_state) ==
	       RPTADV_RADIO_OK);
	assert(vox_carrier_state.remaining_ms == 20 && vox_carrier_state.carrier_detect == 1U);
	assert(descriptor->radio_tx_cpu_saver_advance(&tx_cpu_saver_input, &tx_cpu_saver_state) ==
	       RPTADV_RADIO_OK);
	assert(tx_cpu_saver_state.halted == 1U);
	assert(descriptor->radio_rx_cpu_saver_advance(&rx_cpu_saver_input, &rx_cpu_saver_state) ==
	       RPTADV_RADIO_OK);
	assert(rx_cpu_saver_state.halted == 1U);
	assert(rx_cpu_saver_state.action == RPTADV_RADIO_RX_CPU_SAVER_ACTION_ENTER);
	audio_meter_mono[5] = 32767.0F / 32768.0F;
	assert(descriptor->radio_measure_raw_pcm_f32(audio_meter_mono, 6, 1, &audio_statistics,
						      &audio_clipping) == RPTADV_RADIO_OK);
	assert(!audio_clipping && audio_statistics.index == 1 && audio_statistics.maxbuf[0] == 32767);
	assert(audio_statistics.pwrbuf[0] == (uint32_t)((uint64_t)32767 * 32767));
	assert(descriptor->radio_micor_squelch_update(&micor_squelch, 1U, 100000000.0, 7000U,
								    500U, &micor_closed) == RPTADV_RADIO_OK);
	assert(micor_closed == 1U && micor_squelch.settling_samples == 1U);
	assert(descriptor->radio_measure_envelope_f32(envelope_input, envelope_output, 4U, 1, 97,
								  &envelope, &envelope_comparator) == RPTADV_RADIO_OK);
	assert(envelope_comparator == 1U && envelope.maximum == 97 && envelope.minimum == -98 &&
	       envelope.peak == 97 && envelope.upper_decay_counter == 1 &&
	       envelope.lower_decay_counter == 1);
	assert(envelope_output[0] == 50.0F / 32768.0F &&
	       envelope_output[3] == 97.0F / 32768.0F);
	assert(descriptor->radio_delay_line_f32(delay_input, delay_output, 4U, delay_storage, 5U,
						 3U, &delay_state, 1U, 0U) == RPTADV_RADIO_OK);
	assert(delay_output[0] == 0.0F && delay_output[1] == 0.0F &&
	       delay_output[2] == 0.0F && delay_output[3] == 10.0F);
	assert(delay_state.input_index == 4U && delay_state.dirty == 1U);
	assert(descriptor->radio_center_slicer_f32(center_input, center_output, center_limited, 4U,
							 100, 1000, 1, &center_state) == RPTADV_RADIO_OK);
	assert(center_output[0] == -500.0F / 32768.0F &&
	       center_output[1] == 500.0F / 32768.0F &&
	       center_output[2] == -500.0F / 32768.0F &&
	       center_output[3] == 500.0F / 32768.0F);
	assert(center_limited[0] == -100.0F / 32768.0F &&
	       center_limited[1] == 100.0F / 32768.0F &&
	       center_limited[2] == -100.0F / 32768.0F &&
	       center_limited[3] == 100.0F / 32768.0F);
	assert(center_state.maximum == 1999 && center_state.minimum == 1001 &&
	       center_state.peak == 499);
	assert(descriptor->radio_deemphasis_integrator_f32(
		       deemphasis_input, deemphasis_output, 4U, 6878, 25889, 256,
		       &deemphasis_state) == RPTADV_RADIO_OK);
	assert(deemphasis_output[0] == -8188.0F / 32768.0F &&
	       deemphasis_output[1] == 10322.0F / 32768.0F &&
	       deemphasis_output[2] == -240.0F / 32768.0F &&
	       deemphasis_output[3] == 27321.0F / 32768.0F);
	assert(deemphasis_state.accumulator == 32541);
	assert(descriptor->radio_fir_mono_f32(fir_input, fir_output, 3U, fir_history, 3U,
					      fir_coefficients, 300, 280, 10000) == RPTADV_RADIO_OK);
	assert(fir_output[0] == 28794.0F / 32768.0F &&
	       fir_output[1] == -16027.0F / 32768.0F &&
	       fir_output[2] == 9693.0F / 32768.0F);
	assert(fir_history[0] == 0 && fir_history[1] == -1446 && fir_history[2] == 27136);
	assert(descriptor->radio_create(&config, &radio) == RPTADV_RADIO_OK);
	assert(radio != NULL);
	assert(descriptor->radio_create(&config, &shifted_radio) == RPTADV_RADIO_OK);
	assert(descriptor->radio_create(&config, &tail_radio) == RPTADV_RADIO_OK);
	assert(descriptor->radio_dcs_configure_receive(radio, 023, 0) == RPTADV_RADIO_OK);
	assert(descriptor->radio_dcs_process_receive_f32(radio, dcs_receive_stereo, 1,
								  &dcs_receive_valid) == RPTADV_RADIO_OK);
	assert(!dcs_receive_valid);
	assert(descriptor->radio_dcs_configure_receive(radio, -1, 0) == RPTADV_RADIO_OK);
	assert(descriptor->radio_dcs_process_receive_f32(radio, NULL, 1, &dcs_receive_valid) ==
	       RPTADV_RADIO_OK);
	assert(!dcs_receive_valid);
	assert(descriptor->radio_ctcss_frequency_supported(67.0F));
	assert(!descriptor->radio_ctcss_frequency_supported(100.0001F));
	assert(descriptor->radio_ctcss_legacy_frequency(114.8) == 114.74609375);
	assert(descriptor->radio_ctcss_legacy_peak(114.8, 0) == 17083.0);
	assert(descriptor->radio_ctcss_legacy_peak(114.8, 1) == 18417.0);
	assert(descriptor->radio_ctcss_legacy_scaled_peak(114.8, 0, 102, 252) == 6699.0);
	assert(descriptor->radio_ctcss_legacy_scaled_levels(85.4, 1, 256, 256, &amplitude, &bias) ==
	       RPTADV_RADIO_OK);
	assert(amplitude == 17886.0 && bias == 1.0);
	assert(descriptor->radio_ctcss_legacy_scaled_levels(100.0, 0, 256, 256, &amplitude, &bias) ==
	       RPTADV_RADIO_OK);
	assert(amplitude == 16951.5 && bias == -0.5);
	assert(descriptor->radio_ctcss_legacy_scaled_levels(100.0, 0, 256, 256, NULL, &bias) ==
	       RPTADV_RADIO_INVALID_ARGUMENT);

	assert(descriptor->radio_ctcss_generate_f32(radio, ctcss_output, 8, 114.8, 1.0F, 1, 0.0) ==
	       RPTADV_RADIO_OK);
	assert(ctcss_output[0] == 0.0F && ctcss_output[1] > 0.015F && ctcss_output[1] < 0.016F);
	assert(descriptor->radio_ctcss_phase_radians(radio, &normal_phase) == RPTADV_RADIO_OK);
	assert(descriptor->radio_ctcss_generate_f32(radio, ctcss_output, 8, 114.8, 1.0F, 0, 180.0) ==
	       RPTADV_RADIO_OK);
	for (unsigned int index = 0; index < 8; ++index)
		assert(ctcss_output[index] == 0.0F);
	assert(descriptor->radio_ctcss_phase_radians(radio, &shifted_phase) == RPTADV_RADIO_OK);
	assert(normal_phase == shifted_phase);

	assert(descriptor->radio_ctcss_generate_f32(shifted_radio, shifted_ctcss, 8, 114.8, 1.0F, 1,
						      0.0) == RPTADV_RADIO_OK);
	assert(descriptor->radio_ctcss_generate_f32(radio, ctcss_output, 1, 114.8, 1.0F, 1, 0.0) ==
	       RPTADV_RADIO_OK);
	assert(descriptor->radio_ctcss_generate_f32(shifted_radio, shifted_ctcss, 1, 114.8, 1.0F, 1,
						      360.0 * 170.0 / 256.0) == RPTADV_RADIO_OK);
	assert(descriptor->radio_ctcss_phase_radians(radio, &normal_phase) == RPTADV_RADIO_OK);
	assert(descriptor->radio_ctcss_phase_radians(shifted_radio, &shifted_phase) == RPTADV_RADIO_OK);
	phase_difference = shifted_phase - normal_phase;
	if (phase_difference < 0.0)
		phase_difference += 6.28318530717958647692;
	assert(phase_difference > 4.17 && phase_difference < 4.18);

	assert(descriptor->radio_ctcss_generate_tail_f32(tail_radio, tail_ctcss, 48000, 55.0, 1.0F,
							  1) == RPTADV_RADIO_OK);
	assert(tail_ctcss[0] == 0.0F && tail_ctcss[1] > tail_ctcss[0]);
	assert(descriptor->radio_ctcss_phase_radians(tail_radio, &tail_phase) == RPTADV_RADIO_OK);
	assert(tail_phase > -0.000000000001 && tail_phase < 0.000000000001);
	assert(descriptor->radio_tick(radio, input, output, 4) == RPTADV_RADIO_OK);
	for (unsigned int index = 0; index < 8; ++index) {
		assert(output[index] == 0.0F);
	}
	assert(descriptor->radio_extract_receive_f32(radio, receive_stereo, receive_mono, 3,
						     receive_delay, 2, &receive_delay_index,
						     &receive_stats) == RPTADV_RADIO_OK);
	assert(receive_stats.peak == 1.0F && receive_stats.rail_samples == 2U);
	assert(receive_delay_index == 1U);
	assert(receive_mono[0] == 10.0F / 32768.0F &&
	       receive_mono[1] == 20.0F / 32768.0F &&
	       receive_mono[2] == 32767.0F / 32768.0F);
	assert(descriptor->radio_render_calibrated_test_tone_f32(radio, test_tone_output, 8, 1) ==
	       RPTADV_RADIO_OK);
	assert(test_tone_output[0] == 0.0F && test_tone_output[1] > 0.029F &&
	       test_tone_output[1] < 0.030F);
	assert(descriptor->radio_calibrated_test_tone_phase_radians(radio, &test_tone_phase) ==
	       RPTADV_RADIO_OK);
	assert(test_tone_phase > 1.04 && test_tone_phase < 1.05);
	test_tone_output[0] = 0.25F;
	assert(descriptor->radio_render_calibrated_test_tone_f32(radio, test_tone_output, 1, 0) ==
	       RPTADV_RADIO_OK);
	assert(test_tone_output[0] == 0.25F);
	assert(descriptor->radio_calibrated_test_tone_phase_radians(radio, &test_tone_phase) ==
	       RPTADV_RADIO_OK);
	assert(test_tone_phase == 0.0);
	assert(descriptor->radio_render_calibrated_test_tone_f32(radio, test_tone_output, 1, 1) ==
	       RPTADV_RADIO_OK);
	assert(test_tone_output[0] == 0.0F);
	assert(descriptor->radio_native_parrot_bind_f32(radio, parrot_storage, 4) ==
	       RPTADV_RADIO_OK);
	assert(descriptor->radio_native_parrot_rx_transition(radio, 0, 1,
								 &parrot_playback_started) == RPTADV_RADIO_OK);
	assert(parrot_playback_started == 0);
	assert(descriptor->radio_native_parrot_record_f32(radio, parrot_input, 4, 4,
								    &parrot_recorded) == RPTADV_RADIO_OK);
	assert(parrot_recorded == 4);
	assert(descriptor->radio_native_parrot_rx_transition(radio, 1, 0,
								 &parrot_playback_started) == RPTADV_RADIO_OK);
	assert(parrot_playback_started == 1);
	assert(descriptor->radio_native_parrot_play_f32(radio, parrot_output, 2, &parrot_played) ==
	       RPTADV_RADIO_OK);
	assert(parrot_played == 2 && parrot_output[0] == parrot_input[0] &&
	       parrot_output[1] == parrot_input[1] && parrot_output[2] == -1.0F);
	assert(descriptor->radio_native_parrot_status(radio, &parrot_status) == RPTADV_RADIO_OK);
	assert(parrot_status.recorded_samples == 4 && parrot_status.playback_offset == 2 &&
	       parrot_status.playing == 1 && !parrot_status.truncated);
	assert(descriptor->radio_native_parrot_play_f32(radio, parrot_output + 2, 2,
								  &parrot_played) == RPTADV_RADIO_OK);
	assert(parrot_played == 2 && parrot_output[2] == parrot_input[2] &&
	       parrot_output[3] == parrot_input[3]);
	assert(descriptor->radio_native_parrot_reset(radio) == RPTADV_RADIO_OK);
	assert(descriptor->radio_native_parrot_status(radio, &parrot_status) == RPTADV_RADIO_OK);
	assert(!parrot_status.recorded_samples && !parrot_status.playback_offset &&
	       !parrot_status.playing && !parrot_status.truncated);
	assert(descriptor->radio_repeat_f32(input, output, 8, 0.5F, 0) == RPTADV_RADIO_OK);
	assert(output[0] == 0.5F);
	assert(output[1] == -0.5F);
	assert(descriptor->radio_repeat_f32(NULL, output, 8, 1.0F, 1) == RPTADV_RADIO_OK);
	for (unsigned int index = 0; index < 8; ++index) {
		assert(output[index] == 0.0F);
	}
	assert(descriptor->radio_render_transmit_f32(
		radio, program, ctcss, dcs, 1, &render_config, stereo, meter, &rails) == RPTADV_RADIO_OK);
	assert(rails == 0);
	assert(stereo[0] == 1000 && stereo[1] == 0);
	assert(meter[0] == 1000 && meter[1] == 1000);

	render_config.output_a_route = RPTADV_RADIO_TX_OUTPUT_COMPOSITE;
	render_config.output_b_route = RPTADV_RADIO_TX_OUTPUT_TONE;
	render_config.ctcss_peak_a = 40000.0F / 32767.0F;
	render_config.ctcss_peak_b = 40000.0F / 32767.0F;
	assert(descriptor->radio_render_transmit_f32(radio, legacy_program, legacy_ctcss,
		legacy_dcs, 3, &render_config, legacy_stereo, legacy_meter, &rails) == RPTADV_RADIO_OK);
	assert(rails == 2);
	assert(legacy_meter[0] == INT16_MAX && legacy_meter[1] == INT16_MAX);
	assert(legacy_meter[2] == INT16_MIN && legacy_meter[3] == INT16_MIN);
	assert(legacy_meter[4] == 1000 && legacy_meter[5] == 1000);
	assert(legacy_stereo[0] == INT16_MAX && legacy_stereo[1] == 2767);
	assert(legacy_stereo[2] == INT16_MIN && legacy_stereo[3] == INT16_MIN);
	assert(legacy_stereo[4] == 21010 && legacy_stereo[5] == 20020);

	stereo[0] = 0;
	stereo[1] = 0;
	render_config.output_a_route = RPTADV_RADIO_TX_OUTPUT_DISABLED;
	render_config.output_b_route = RPTADV_RADIO_TX_OUTPUT_VOICE;
	render_config.ctcss_peak_a = 1.0F;
	render_config.ctcss_peak_b = 1.0F;
	assert(descriptor->radio_render_transmit_f32(
		radio, program, ctcss, dcs, 1, &render_config, stereo, NULL, &rails) == RPTADV_RADIO_OK);
	assert(stereo[0] == 0 && stereo[1] == 1000);

	stereo[0] = 0;
	stereo[1] = 0;
	render_config.output_a_route = RPTADV_RADIO_TX_OUTPUT_TONE;
	render_config.output_b_route = RPTADV_RADIO_TX_OUTPUT_COMPOSITE;
	render_config.ctcss_peak_a = 16000.0F / 32767.0F;
	render_config.ctcss_peak_b = 16000.0F / 32767.0F;
	assert(descriptor->radio_render_transmit_f32(radio, dcs_program, dcs_ctcss,
		dcs_signal, 1, &render_config, stereo, NULL, &rails) == RPTADV_RADIO_OK);
	assert(stereo[0] == 2000 && stereo[1] == 2000);

	assert(descriptor->radio_dcs_code_supported(023));
	assert(!descriptor->radio_dcs_code_supported(01000));
	assert(descriptor->radio_dcs_configure_transmit(radio, 023, 0) == RPTADV_RADIO_OK);
	assert(descriptor->radio_dcs_generate_f32(radio, dcs_output, 4, dcs_peak, 1, 0) ==
	       RPTADV_RADIO_OK);
	assert(dcs_output[0] == dcs_peak);
	assert(descriptor->radio_dcs_configure_transmit(radio, 023, 1) == RPTADV_RADIO_OK);
	assert(descriptor->radio_dcs_generate_f32(radio, dcs_output, 1, dcs_peak, 1, 0) ==
	       RPTADV_RADIO_OK);
	assert(dcs_output[0] == -dcs_peak);
	assert(descriptor->radio_dcs_generate_f32(radio, dcs_output, 4, dcs_peak, 1, 1) ==
	       RPTADV_RADIO_OK);
	assert(dcs_output[0] == 0.0F && dcs_output[1] > 0.0F);
	assert(descriptor->radio_dcs_tail_phase_radians(radio, &dcs_tail_phase) == RPTADV_RADIO_OK);
	assert(dcs_tail_phase > 0.0);
	descriptor->radio_destroy(radio);
	descriptor->radio_destroy(shifted_radio);
	descriptor->radio_destroy(tail_radio);
	return 0;
}
