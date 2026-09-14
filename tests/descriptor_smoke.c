/**
 * @file descriptor_smoke.c
 * @brief Exercise the ABI 3 session lifecycle through the installed header.
 */

#include <assert.h>
#include <stddef.h>
#include <string.h>

#include "rptadvradio/rptadvradio.h"

struct processor_context {
	float scale;
	float bias;
	uint32_t calls;
	uint32_t warm_calls;
	uint32_t fail;
};

static int32_t process(void *opaque, const float *input, float *output,
		       uint32_t frame_count)
{
	struct processor_context *context = opaque;
	uint32_t index;
	context->calls++;
	if (context->fail != 0)
		return -1;
	for (index = 0; index < frame_count; ++index)
		output[index] = input[index] * context->scale + context->bias;
	return 0;
}

static int32_t warm(void *opaque, uint32_t frame_count)
{
	struct processor_context *context = opaque;
	assert(frame_count == 16);
	context->warm_calls++;
	return 0;
}

static int32_t ring_render(void *opaque, float *output, uint32_t frame_count,
			   struct rptadv_radio_program_ring_result *result)
{
	uint32_t index;
	(void)opaque;
	for (index = 0; index < frame_count; ++index)
		output[index] = 0.1F;
	result->observation.occupancy_frames = 120;
	result->observation.reserve_frames = 20;
	result->observation.target_frames = 60;
	result->observation.capacity_frames = 240;
	result->observation.ratio = 1.00001;
	result->ctcss_decoded_index = -1;
	return 0;
}

static struct rptadv_radio_processor_port processor_port(struct processor_context *context)
{
	struct rptadv_radio_processor_port port = {
		.context = context,
		.process_f32 = process,
		.warm = warm,
	};
	return port;
}

static struct rptadv_radio_session_config valid_config(void)
{
	struct rptadv_radio_session_config config = { 0 };

	config.struct_size = sizeof(config);
	config.abi_version = RPTADV_RADIO_ABI_VERSION;
	config.generation_id = 42;
	config.native_sample_rate_hz = RPTADV_RADIO_NATIVE_SAMPLE_RATE_HZ;
	config.interleaved_channels = RPTADV_RADIO_CANONICAL_CHANNELS;
	config.maximum_receive_frame_count = 16;
	config.maximum_transmit_frame_count = 16;
	config.publication_interval_ms = 50;
	config.receive_input_gain = 1.0F;
	config.receive.noise_filter_profile = RPTADV_RADIO_NOISE_FILTER_STANDARD;
	config.receive.ctcss_decoder_gain = 1.0F;
	config.qualification.carrier_source = RPTADV_RADIO_CARRIER_USB;
	config.qualification.subaudible_source = RPTADV_RADIO_SUBAUDIBLE_DISABLED;
	config.qualification.radio_duplex = 1;
	config.transmit.output_a_route = RPTADV_RADIO_OUTPUT_VOICE;
	config.transmit.output_b_route = RPTADV_RADIO_OUTPUT_VOICE;
	return config;
}

int main(void)
{
	const struct rptadv_radio_descriptor *descriptor = rptadv_radio_descriptor();
	struct rptadv_radio_session_config config = valid_config();
	struct processor_context deemphasis = { .scale = 1.0F, .bias = 0.1F };
	struct processor_context filter = { .scale = 2.0F };
	struct processor_context notch = { .scale = 1.0F };
	struct processor_context noise = { .scale = 1.0F, .bias = -0.1F };
	struct processor_context dynamics = { .scale = 0.5F };
	struct processor_context transmit_graph = { .scale = 1.0F, .bias = 0.2F };
	struct processor_context dcs_normal_graph = { .scale = 1.0F };
	struct processor_context dcs_turnoff_graph = { .scale = 1.0F };
	struct rptadv_radio_session_ports ports = {
		.struct_size = sizeof(ports),
		.receive_deemphasis = processor_port(&deemphasis),
		.receive_filter = processor_port(&filter),
		.receive_ctcss_notch[0] = processor_port(&notch),
		.receive_noise_reduction = processor_port(&noise),
		.receive_dynamics = processor_port(&dynamics),
		.transmit_program = processor_port(&transmit_graph),
		.transmit_dcs_normal_filter = processor_port(&dcs_normal_graph),
		.transmit_dcs_turnoff_filter = processor_port(&dcs_turnoff_graph),
		.program_ring = { .render_f32 = ring_render },
	};
	struct rptadv_radio_session *session = NULL;
	struct rptadv_radio_receive_input receive_input = { .hardware_carrier = 1 };
	struct rptadv_radio_transmit_input transmit_input = {
		.external_ptt_request = 1,
		.physical_ptt_applied = 1,
		.render_admitted = 1,
	};
	struct rptadv_radio_receive_result receive_result = { 0 };
	struct rptadv_radio_transmit_result transmit_result = { 0 };
	struct rptadv_radio_snapshot snapshot = { 0 };
	struct rptadv_radio_event event = { 0 };
	float capture[8] = { 0.25F, 0.0F, -0.25F, 0.0F,
			     0.5F, 0.0F, -0.5F, 0.0F };
	float receive[4] = { 9.0F, 9.0F, 9.0F, 9.0F };
	float transmit[8] = { 9.0F, 9.0F, 9.0F, 9.0F,
			      9.0F, 9.0F, 9.0F, 9.0F };

	assert(descriptor != NULL);
	assert(descriptor->struct_size == sizeof(*descriptor));
	assert(descriptor->abi_version == RPTADV_RADIO_ABI_VERSION);
	assert(strcmp(descriptor->capability_name, "rptadv.radio-core") == 0);
	assert(descriptor->session_create != NULL);
	assert(descriptor->session_warm != NULL);
	assert(descriptor->session_receive != NULL);
	assert(descriptor->session_transmit != NULL);
	assert(descriptor->session_snapshot != NULL);
	assert(descriptor->session_pop_receive_event != NULL);
	assert(descriptor->session_pop_transmit_event != NULL);
	assert(descriptor->session_destroy != NULL);

	assert(descriptor->session_create(&config, &ports, &session) == RPTADV_RADIO_OK);
	assert(session != NULL);
	assert(descriptor->session_receive(session, capture, receive, 4,
					    &receive_input, &receive_result) ==
	       RPTADV_RADIO_NOT_READY);
	assert(descriptor->session_warm(session) == RPTADV_RADIO_OK);
	assert(deemphasis.warm_calls == 1 && deemphasis.calls == 1);
	assert(filter.warm_calls == 1 && filter.calls == 1);
	assert(notch.warm_calls == 1 && notch.calls == 1);
	assert(noise.warm_calls == 1 && noise.calls == 1);
	assert(dynamics.warm_calls == 1 && dynamics.calls == 1);
	assert(dcs_normal_graph.warm_calls == 1 && dcs_normal_graph.calls == 1);
	assert(dcs_turnoff_graph.warm_calls == 1 && dcs_turnoff_graph.calls == 1);
	deemphasis.calls = filter.calls = noise.calls = dynamics.calls = 0;
	assert(descriptor->session_receive(session, capture, receive, 4,
					    &receive_input, &receive_result) ==
	       RPTADV_RADIO_OK);
	assert(receive_result.generation_id == 42);
	assert(receive_result.frame_count == 4);
	assert(receive_result.ctcss_decoder_peak == 0.0F);
	assert(receive[0] > 0.29F && receive[0] < 0.31F);
	assert(receive[1] > -0.21F && receive[1] < -0.19F);
	assert(deemphasis.calls == 1 && filter.calls == 1 && noise.calls == 1 &&
	       dynamics.calls == 1);
	assert(descriptor->session_transmit(session, transmit, 4, &transmit_input,
					     &transmit_result) == RPTADV_RADIO_OK);
	assert(transmit_result.generation_id == 42);
	assert(transmit_result.logical_ptt == 1);
	assert(transmit_result.program_ring.occupancy_frames == 120);
	assert(transmit[0] > 0.29F && transmit[0] < 0.31F);
	assert(transmit[0] == transmit[1]);
	assert(dcs_normal_graph.calls == 2 && dcs_turnoff_graph.calls == 2);
	assert(descriptor->session_snapshot(session, &snapshot) == RPTADV_RADIO_OK);
	assert(snapshot.receive_frames == 4);
	assert(snapshot.transmit_frames == 4);
	assert(snapshot.receive_ctcss_decoder_peak == 0.0F);
	assert(descriptor->session_pop_receive_event(session, &event) == 1);
	assert(event.generation_id == 42);
	descriptor->session_destroy(session);
	descriptor->session_destroy(NULL);
	return 0;
}
