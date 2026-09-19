/**
 * @file rptadvradio.h
 * @brief ABI 4 for one prepared, portable 48 kHz radio session.
 *
 * The shared object owns radio DSP and signaling state. Device, Asterisk,
 * FFmpeg, RNNoise, and elastic-ring implementations remain in independently
 * versioned adapters and are supplied as narrowly typed borrowed ports.
 */
#ifndef RPTADV_RADIO_H
#define RPTADV_RADIO_H
#include <stdint.h>
#ifdef __cplusplus
extern "C" {
#endif

#define RPTADV_RADIO_ABI_VERSION 4U /**< Breaking whole-session ABI. */
#define RPTADV_RADIO_NATIVE_SAMPLE_RATE_HZ 48000U /**< Immutable native rate. */
#define RPTADV_RADIO_CANONICAL_CHANNELS 2U /**< Interleaved device channels. */
#define RPTADV_RADIO_CTCSS_TONE_COUNT 38U /**< CTCSS mapping-table length. */
#define RPTADV_RADIO_CTCSS_CALIBRATION_TARGET (2400.0F / 32768.0F) /**< Normalized RX decoder target. */

/** @brief Opaque runtime generation owned by the shared object. */
struct rptadv_radio_session;

/** @brief Result from one session operation. */
enum rptadv_radio_result {
  RPTADV_RADIO_OK = 0, /**< Operation completed. */
  RPTADV_RADIO_INVALID_ARGUMENT = -1, /**< An argument is invalid. */
  RPTADV_RADIO_PROVIDER_FAILED = -2, /**< A borrowed port failed. */
  RPTADV_RADIO_FRAME_COUNT_EXCEEDED = -3, /**< Callback exceeds setup bound. */
  RPTADV_RADIO_UNSUPPORTED = -4, /**< Stream rate or layout is unsupported. */
  RPTADV_RADIO_NOT_READY = -5, /**< Session has not been warmed. */
  RPTADV_RADIO_BUSY = -6, /**< A serial callback has two callers. */
};

/** @brief Receive carrier-indication source. */
enum rptadv_radio_carrier_source {
  RPTADV_RADIO_CARRIER_DISABLED = 0, /**< No carrier source. */
  RPTADV_RADIO_CARRIER_DSP = 1, /**< Discriminator-noise detector. */
  RPTADV_RADIO_CARRIER_VOX = 2, /**< Audio-level detector. */
  RPTADV_RADIO_CARRIER_USB = 3, /**< Normal USB GPIO. */
  RPTADV_RADIO_CARRIER_USB_INVERTED = 4, /**< Inverted USB GPIO. */
  RPTADV_RADIO_CARRIER_PARALLEL = 5, /**< Normal parallel input. */
  RPTADV_RADIO_CARRIER_PARALLEL_INVERTED = 6, /**< Inverted parallel input. */
};

/** @brief Native discriminator-noise detector response. */
enum rptadv_radio_noise_filter_profile {
  RPTADV_RADIO_NOISE_FILTER_STANDARD = 0, /**< Established default response. */
  RPTADV_RADIO_NOISE_FILTER_ALTERNATE = 1, /**< Alternate established response. */
};

/** @brief Receive subaudible-indication source. */
enum rptadv_radio_subaudible_source {
  RPTADV_RADIO_SUBAUDIBLE_DISABLED = 0, /**< No required indication. */
  RPTADV_RADIO_SUBAUDIBLE_USB = 1, /**< Normal USB GPIO. */
  RPTADV_RADIO_SUBAUDIBLE_USB_INVERTED = 2, /**< Inverted USB GPIO. */
  RPTADV_RADIO_SUBAUDIBLE_DSP = 3, /**< Native CTCSS or DCS decoder. */
  RPTADV_RADIO_SUBAUDIBLE_PARALLEL = 4, /**< Normal parallel input. */
  RPTADV_RADIO_SUBAUDIBLE_PARALLEL_INVERTED = 5, /**< Inverted parallel input. */
};

/** @brief Transmit CTCSS/DCS turn-off policy. */
enum rptadv_radio_tone_off_mode {
  RPTADV_RADIO_TONE_OFF_NONE = 0, /**< Release normally. */
  RPTADV_RADIO_TONE_OFF_PHASE_SHIFT = 1, /**< Reverse burst. */
  RPTADV_RADIO_TONE_OFF_TONE_REMOVE = 2, /**< Remove tone before PTT. */
  RPTADV_RADIO_TONE_OFF_TAIL_TONE = 3, /**< Send configured tail tone. */
};

/** @brief One hardware output assignment. */
enum rptadv_radio_output_route {
  RPTADV_RADIO_OUTPUT_DISABLED = 0, /**< Silence. */
  RPTADV_RADIO_OUTPUT_VOICE = 1, /**< Program audio. */
  RPTADV_RADIO_OUTPUT_TONE = 2, /**< CTCSS/DCS only. */
  RPTADV_RADIO_OUTPUT_COMPOSITE = 3, /**< Program plus CTCSS/DCS. */
  RPTADV_RADIO_OUTPUT_AUXILIARY_VOICE = 4, /**< Auxiliary program route. */
};

/** @brief Current transmitter-signaling phase. */
enum rptadv_radio_transmitter_state {
  RPTADV_RADIO_TRANSMITTER_IDLE = 0, /**< No transmit request is active. */
  RPTADV_RADIO_TRANSMITTER_ACTIVE = 1, /**< Normal program transmission. */
  RPTADV_RADIO_TRANSMITTER_TONE_OFF = 2, /**< CTCSS or DCS turn-off phase. */
  RPTADV_RADIO_TRANSMITTER_FINISHING = 4, /**< Final drain before release. */
  RPTADV_RADIO_TRANSMITTER_COMPLETE = 5, /**< Release transition completed. */
};

/** @brief Owner-ordered event kinds. */
enum rptadv_radio_event_kind {
  RPTADV_RADIO_EVENT_NONE = 0, /**< No event. */
  RPTADV_RADIO_EVENT_CARRIER = 1, /**< Carrier edge. */
  RPTADV_RADIO_EVENT_SUBAUDIBLE = 2, /**< CTCSS/DCS qualification edge. */
  RPTADV_RADIO_EVENT_RECEIVER_KEYED = 3, /**< Qualified receiver edge. */
  RPTADV_RADIO_EVENT_CTCSS_DECODE = 4, /**< Decoded CTCSS index changed. */
  RPTADV_RADIO_EVENT_DCS_DECODE = 5, /**< DCS validity changed. */
  RPTADV_RADIO_EVENT_PTT = 6, /**< Logical PTT edge. */
  RPTADV_RADIO_EVENT_CTCSS_TRANSMIT = 7, /**< Transmit tone changed. */
  RPTADV_RADIO_EVENT_RECEIVER_BLANKING = 8, /**< RX blanking armed. */
  RPTADV_RADIO_EVENT_PROVIDER_FAILURE = 9, /**< External port failed. */
};

/** @brief Process one exact mono F32 span without allocation or blocking. */
typedef int32_t (*rptadv_radio_process_f32_fn)(void *context,
                                               const float *input,
                                               float *output,
                                               uint32_t frame_count);
/** @brief Exercise a prepared object without publishing live output. */
typedef int32_t (*rptadv_radio_warm_fn)(void *context, uint32_t frame_count);
/** @brief Advance a prepared bypass path without producing PCM. */
typedef int32_t (*rptadv_radio_bypass_fn)(void *context,
                                         uint32_t frame_count);

/** @brief Latest diagnostics returned by a rate-adjusting ring provider. */
struct rptadv_radio_ring_observation {
  uint32_t occupancy_frames; /**< PCM frames retained. */
  uint32_t reserve_frames; /**< Protected minimum occupancy. */
  uint32_t target_frames; /**< Occupancy servo target. */
  uint32_t capacity_frames; /**< Allocated capacity. */
  double ratio; /**< Current input-to-output ratio. */
  uint64_t underrun_samples; /**< Cumulative absent output. */
  uint64_t overrun_samples; /**< Cumulative discarded input. */
  uint64_t concealment_samples; /**< Cumulative concealment output. */
};

/** @brief Program-ring diagnostics and sample-associated RX qualification. */
struct rptadv_radio_program_ring_result {
  struct rptadv_radio_ring_observation observation; /**< Ring diagnostics. */
  uint32_t receiver_keyed; /**< Rendered span contains qualified local RX. */
  int32_t ctcss_decoded_index; /**< Aligned CTCSS table index or negative. */
  uint32_t dcs_valid; /**< Aligned configured DCS state. */
};

/** @brief Render exact mono F32 from the prepared program ring. */
typedef int32_t (*rptadv_radio_ring_render_f32_fn)(
    void *context, float *output, uint32_t frame_count,
    struct rptadv_radio_program_ring_result *result);

/** @brief Borrowed prepared processor; null callbacks make it a bypass. */
struct rptadv_radio_processor_port {
  void *context; /**< Provider state valid through session destruction. */
  rptadv_radio_process_f32_fn process_f32; /**< Exact-frame callback. */
  rptadv_radio_bypass_fn bypass; /**< Advance disabled/bypassed state. */
  rptadv_radio_warm_fn warm; /**< Setup warm-up callback. */
};

/** @brief Borrowed program-ring consumer; null render produces silence. */
struct rptadv_radio_program_ring_port {
  void *context; /**< Provider state valid through session destruction. */
  rptadv_radio_ring_render_f32_fn render_f32; /**< Exact-frame render. */
  rptadv_radio_warm_fn warm; /**< Non-consuming setup warm-up. */
};

/** @brief Immutable discriminator and subaudible detector policy. */
struct rptadv_radio_receive_config {
  uint32_t noise_filter_profile; /**< enum rptadv_radio_noise_filter_profile. */
  uint32_t squelch_open_level; /**< MICOR opening threshold. */
  uint32_t squelch_hysteresis; /**< MICOR closing hysteresis. */
  float ctcss_decoder_gain; /**< Linear decoder-only input gain. */
  int32_t vox_threshold; /**< VOX threshold, 0 through 32767 PCM codes. */
  int32_t vox_hang_ms; /**< VOX hang, 0 through 32767 milliseconds. */
  uint32_t ctcss_enabled; /**< Enable native CTCSS decode. */
  uint64_t ctcss_tone_mask; /**< Enabled CTCSS table entries. */
  uint32_t ctcss_relax; /**< Select relaxed decoder tolerance. */
  uint32_t dcs_enabled; /**< Enable native DCS decode. */
  int32_t dcs_code; /**< Three-octal-digit DCS code. */
  uint32_t dcs_inverted; /**< Invert receive DCS polarity. */
  uint32_t cpu_saver_enabled; /**< Freeze idle voice/calibration work. */
  uint32_t native_squelch_delay_frames; /**< Native pre-deemphasis delay. */
};

/** @brief Immutable receiver qualification and duplex policy. */
struct rptadv_radio_qualification_config {
  uint32_t carrier_source; /**< enum rptadv_radio_carrier_source. */
  uint32_t subaudible_source; /**< enum rptadv_radio_subaudible_source. */
  uint32_t subaudible_override; /**< Admit carrier without tone. */
  uint32_t advanced_transport; /**< Advanced full-rate transport active. */
  uint32_t radio_duplex; /**< Permit receive during transmit. */
  uint32_t rx_on_delay_blocks; /**< Required 20 ms admission blocks. */
  uint32_t tx_off_delay_blocks; /**< Post-transmit 20 ms guard blocks. */
};

/** @brief Immutable transmitter signaling, level, and routing. */
struct rptadv_radio_transmit_config {
  uint32_t ctcss_transmit_enabled; /**< Enable transmit CTCSS selection. */
  int32_t default_ctcss_frequency_tenths_hz; /**< Default transmit tone. */
  int32_t mapped_ctcss_frequency_tenths_hz[RPTADV_RADIO_CTCSS_TONE_COUNT]; /**< RX tone map. */
  uint32_t tone_off_mode; /**< enum rptadv_radio_tone_off_mode. */
  int32_t ctcss_turnoff_duration_ms; /**< CTCSS tail duration. */
  double ctcss_turnoff_phase_shift_degrees; /**< Reverse-burst phase. */
  double ctcss_turnoff_tail_tone_hz; /**< Replacement tail tone. */
  uint32_t dcs_transmit_enabled; /**< Enable transmit DCS. */
  uint32_t dcs_turnoff_enabled; /**< Enable DCS turn-off. */
  int32_t dcs_turnoff_duration_ms; /**< DCS turn-off duration. */
  int32_t receiver_blanking_ms; /**< Post-PTT receive blanking. */
  int32_t tx_settle_time_ms; /**< Audio delay after physical PTT. */
  uint32_t cpu_saver_enabled; /**< Halt idle signaling work. */
  int32_t dcs_code; /**< Transmit DCS code. */
  uint32_t dcs_inverted; /**< Invert transmit DCS polarity. */
  float dcs_peak; /**< Normalized DCS peak. */
  float ctcss_peak; /**< Normalized CTCSS oscillator peak. */
  uint32_t output_a_route; /**< enum rptadv_radio_output_route. */
  uint32_t output_b_route; /**< enum rptadv_radio_output_route. */
  float output_a_tone_gain; /**< Output A tone calibration. */
  float output_a_tone_bias; /**< Output A tone bias. */
  float output_b_tone_gain; /**< Output B tone calibration. */
  float output_b_tone_bias; /**< Output B tone bias. */
};

/** @brief Complete immutable setup for one runtime generation. */
struct rptadv_radio_session_config {
  uint32_t struct_size; /**< Size supplied by caller. */
  uint32_t abi_version; /**< Must equal RPTADV_RADIO_ABI_VERSION. */
  uint64_t generation_id; /**< Tag copied to observations. */
  uint32_t native_sample_rate_hz; /**< Must be 48000. */
  uint32_t interleaved_channels; /**< Must be two. */
  uint32_t maximum_receive_frame_count; /**< Preallocated RX bound. */
  uint32_t maximum_transmit_frame_count; /**< Preallocated TX bound. */
  uint32_t publication_interval_ms; /**< Periodic status cadence. */
  uint32_t receive_channel; /**< Capture channel, zero or one. */
  float receive_input_gain; /**< Linear post-deemphasis gain. */
  struct rptadv_radio_receive_config receive; /**< Detector policy. */
  struct rptadv_radio_qualification_config qualification; /**< RX gating. */
  struct rptadv_radio_transmit_config transmit; /**< TX policy. */
};

/**
 * @brief Borrowed prepared runtime ports grouped by exact pipeline role.
 *
 * Receive order is deemphasis, squelch gate/input gain, fixed filter graph,
 * the prepared decoded-tone or 55 Hz tail notch, RNNoise, then dynamics.
 * Transmit processing precedes native CTCSS/DCS mix. Providers must be
 * allocation-free, lock-free, nonblocking, and exact-frame.
 */
struct rptadv_radio_session_ports {
  uint32_t struct_size; /**< Size supplied by caller. */
  struct rptadv_radio_processor_port receive_deemphasis; /**< RX deemphasis. */
  struct rptadv_radio_processor_port receive_filter; /**< Fixed RX filtering. */
  struct rptadv_radio_processor_port
      receive_ctcss_notch[RPTADV_RADIO_CTCSS_TONE_COUNT]; /**< Prepared one-tone notches. */
  struct rptadv_radio_processor_port receive_noise_reduction; /**< RNNoise. */
  struct rptadv_radio_processor_port receive_dynamics; /**< RX dynamics. */
  struct rptadv_radio_processor_port transmit_program; /**< Final TX graph. */
  struct rptadv_radio_processor_port transmit_dcs_normal_filter; /**< Normal DCS NRZ shaper. */
  struct rptadv_radio_processor_port transmit_dcs_turnoff_filter; /**< DCS turn-off sine shaper. */
  struct rptadv_radio_program_ring_port program_ring; /**< TX program source. */
  struct rptadv_radio_processor_port receive_ctcss_tail_notch; /**< Prepared 55 Hz tail notch. */
};

/** @brief Hardware snapshots supplied to one receive callback. */
struct rptadv_radio_receive_input {
  uint32_t hardware_carrier; /**< Normal GPIO COR. */
  uint32_t parallel_carrier; /**< Normal parallel COR. */
  uint32_t hardware_subaudible; /**< Normal GPIO tone input. */
  uint32_t parallel_subaudible; /**< Normal parallel tone input. */
  uint32_t subaudible_override; /**< Admit carrier without tone for this span. */
};

/** @brief Control/hardware snapshots supplied to one transmit callback. */
struct rptadv_radio_transmit_input {
  uint32_t external_ptt_request; /**< Program requests transmit. */
  uint32_t physical_ptt_applied; /**< Hardware reports keyed. */
  uint32_t render_admitted; /**< A DAC span will be delivered. */
  uint32_t ctcss_inhibit; /**< Suppress selected transmit CTCSS. */
  uint32_t calibrated_test_tone; /**< Replace program with 1 kHz reference. */
  int32_t forced_ctcss_tenths_hz; /**< Forced tone, or zero for normal selection. */
};

/** @brief Immediate observation from one receive callback. */
struct rptadv_radio_receive_result {
  uint64_t generation_id; /**< Runtime generation tag. */
  uint64_t first_sample_index; /**< First RX stream sample. */
  uint32_t frame_count; /**< Consumed native frames. */
  uint32_t carrier_active; /**< Resolved carrier state. */
  uint32_t subaudible_active; /**< Resolved tone state. */
  uint32_t receiver_keyed; /**< Qualified receiver state. */
  int32_t ctcss_decoded_index; /**< Table index or negative. */
  uint32_t dcs_valid; /**< Configured DCS is decoded. */
  int16_t rssi_peak; /**< Compatibility RSSI value. */
  uint32_t rssi_updated; /**< RSSI window completed. */
  float ctcss_decoder_peak; /**< Normalized post-gain CTCSS half peak-to-peak. */
  float input_peak; /**< Raw selected-channel normalized peak. */
  float input_rms; /**< Raw selected-channel normalized RMS. */
  uint64_t input_rail_samples; /**< Raw samples at a hardware rail. */
  float output_peak; /**< Processed normalized peak. */
  float output_rms; /**< Processed normalized RMS. */
  uint64_t output_rail_samples; /**< Processed samples at a hardware rail. */
  uint32_t periodic_status_due; /**< Publication deadline crossed. */
};

/** @brief Immediate observation from one transmit callback. */
struct rptadv_radio_transmit_result {
  uint64_t generation_id; /**< Runtime generation tag. */
  uint64_t first_sample_index; /**< First TX stream sample. */
  uint32_t frame_count; /**< Produced native frames. */
  uint32_t logical_ptt; /**< Current PTT intent. */
  int32_t transmitter_state; /**< enum rptadv_radio_transmitter_state. */
  int32_t selected_ctcss_tenths_hz; /**< Selected tone. */
  float program_peak; /**< Post-graph mono program peak. */
  float program_rms; /**< Post-graph mono program RMS. */
  uint64_t program_rail_samples; /**< Post-graph program rail samples. */
  float output_peak; /**< Routed stereo normalized peak. */
  float output_rms; /**< Routed stereo normalized RMS. */
  uint64_t output_rail_samples; /**< Routed stereo rail samples. */
  uint32_t periodic_status_due; /**< Publication deadline crossed. */
  struct rptadv_radio_ring_observation program_ring; /**< Ring diagnostics. */
};

/** @brief One owner-ordered, generation-tagged edge. */
struct rptadv_radio_event {
  uint64_t generation_id; /**< Runtime generation tag. */
  uint64_t sample_index; /**< Owner stream position after detection. */
  uint32_t kind; /**< enum rptadv_radio_event_kind. */
  int32_t value; /**< New state or selected value. */
};

/** @brief Lock-free latest-value session diagnostics. */
struct rptadv_radio_snapshot {
  uint64_t generation_id; /**< Runtime generation tag. */
  uint64_t receive_frames; /**< Cumulative RX frames. */
  uint64_t transmit_frames; /**< Cumulative TX frames. */
  float receive_input_peak; /**< Latest raw selected-channel RX peak. */
  float receive_input_rms; /**< Latest raw selected-channel RX RMS. */
  float receive_ctcss_decoder_peak; /**< Latest normalized CTCSS decoder peak. */
  float receive_output_peak; /**< Latest processed RX peak. */
  float receive_output_rms; /**< Latest processed RX RMS. */
  float transmit_program_peak; /**< Latest post-graph program peak. */
  float transmit_program_rms; /**< Latest post-graph program RMS. */
  float transmit_output_peak; /**< Latest routed stereo TX peak. */
  float transmit_output_rms; /**< Latest routed stereo TX RMS. */
  uint64_t receive_input_rail_samples; /**< Cumulative raw RX rail samples. */
  uint64_t receive_output_rail_samples; /**< Cumulative processed RX rails. */
  uint64_t transmit_program_rail_samples; /**< Cumulative program rails. */
  uint64_t transmit_output_rail_samples; /**< Cumulative output rails. */
  uint64_t provider_failures; /**< External callback failures. */
  uint64_t receive_event_drops; /**< Full RX queue events. */
  uint64_t transmit_event_drops; /**< Full TX queue events. */
  uint32_t carrier_active; /**< Latest carrier state. */
  uint32_t subaudible_active; /**< Latest tone state. */
  uint32_t receiver_keyed; /**< Latest receiver state. */
  uint32_t logical_ptt; /**< Latest PTT intent. */
  int32_t ctcss_decoded_index; /**< Latest CTCSS index. */
  uint32_t dcs_valid; /**< Latest DCS state. */
  struct rptadv_radio_ring_observation program_ring; /**< Ring diagnostics. */
};

/** @brief Immutable whole-session ABI 4 function table. */
struct rptadv_radio_descriptor {
  uint32_t struct_size; /**< Descriptor size. */
  uint32_t abi_version; /**< Descriptor ABI. */
  const char *capability_name; /**< Stable capability name. */
  enum rptadv_radio_result (*session_create)(
      const struct rptadv_radio_session_config *,
      const struct rptadv_radio_session_ports *,
      struct rptadv_radio_session **); /**< Prepare a generation. */
  enum rptadv_radio_result (*session_warm)(
      struct rptadv_radio_session *); /**< Warm processors before start. */
  enum rptadv_radio_result (*session_receive)(
      struct rptadv_radio_session *, const float *, float *, uint32_t,
      const struct rptadv_radio_receive_input *,
      struct rptadv_radio_receive_result *); /**< Exact RX callback. */
  enum rptadv_radio_result (*session_transmit)(
      struct rptadv_radio_session *, float *, uint32_t,
      const struct rptadv_radio_transmit_input *,
      struct rptadv_radio_transmit_result *); /**< Exact TX callback. */
  enum rptadv_radio_result (*session_snapshot)(
      const struct rptadv_radio_session *,
      struct rptadv_radio_snapshot *); /**< Read lock-free status. */
  uint32_t (*session_pop_receive_event)(
      const struct rptadv_radio_session *,
      struct rptadv_radio_event *); /**< Pop one RX event. */
  uint32_t (*session_pop_transmit_event)(
      const struct rptadv_radio_session *,
      struct rptadv_radio_event *); /**< Pop one TX event. */
  void (*session_destroy)(struct rptadv_radio_session *); /**< Destroy stopped session. */
};

/**
 * @brief Return the process-lifetime ABI 4 descriptor.
 *
 * Validate `abi_version` and `struct_size` before use. ABI 2 granular
 * operations were removed rather than retained as compatibility slots.
 */
const struct rptadv_radio_descriptor *rptadv_radio_descriptor(void);

#ifdef __cplusplus
}
#endif
#endif
