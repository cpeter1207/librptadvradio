/**
 * @file rptadvradio.h
 * @brief Stable C ABI for the portable rpt_advanced radio core.
 *
 * Native ticks exchange interleaved normalized IEEE-754 binary32 PCM.  Device
 * I/O, Asterisk conversion, external DSP, and radio-control policy belong to
 * separately versioned adapters and are intentionally absent from this ABI.
 */

#ifndef RPTADV_RADIO_H
#define RPTADV_RADIO_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/** @brief ABI implemented by this descriptor. */
#define RPTADV_RADIO_ABI_VERSION 1U

/** @brief Canonical interleaved PCM channel count for ABI version one. */
#define RPTADV_RADIO_CANONICAL_CHANNELS 2U

/** @brief Fixed legacy CTCSS oscillator rate supported by ABI version one. */
#define RPTADV_RADIO_CTCSS_NATIVE_SAMPLE_RATE_HZ 48000U

/** @brief Fixed post-filter sample rate used by the legacy CTCSS decoder. */
#define RPTADV_RADIO_CTCSS_RECEIVE_SAMPLE_RATE_HZ 8000U

/** @brief Number of table entries accepted by the legacy CTCSS decoder. */
#define RPTADV_RADIO_CTCSS_RECEIVE_TONE_COUNT 38U

/** @brief Fixed calibrated-transmit-tone frequency in hertz. */
#define RPTADV_RADIO_CALIBRATED_TEST_TONE_FREQUENCY_HZ 1000.0

/** @brief Calibrated-transmit-tone peak in legacy signed-16 PCM codes. */
#define RPTADV_RADIO_CALIBRATED_TEST_TONE_PEAK_PCM_CODES 7518.0F

/** @brief Number of retained 20 ms raw-PCM meter slots. */
#define RPTADV_RADIO_AUDIO_STATS_LEN 50U

/** @brief Opaque native-tick state owned by this shared object. */
struct rptadv_radio;

/** @brief Result returned by one public radio-core operation. */
enum rptadv_radio_result {
  /** Operation completed. */
  RPTADV_RADIO_OK = 0,
  /** A pointer, structure version, or scalar argument was invalid. */
  RPTADV_RADIO_INVALID_ARGUMENT = -1,
  /** A tick requested more frames than setup declared. */
  RPTADV_RADIO_FRAME_COUNT_EXCEEDED = -3,
  /** ABI version one does not support the requested stream layout. */
  RPTADV_RADIO_UNSUPPORTED = -4,
};

/** @brief Receiver audio-source assignments retained for USBRadioPlus
 * compatibility. */
enum rptadv_radio_rx_audio_mode {
  /** Do not admit receiver audio. Accepted symbolic value: `no`. */
  RPTADV_RADIO_RX_AUDIO_DISABLED = 0,
  /** Radio speaker audio with radio-supplied deemphasis. Accepted value:
     `speaker`. */
  RPTADV_RADIO_RX_AUDIO_SPEAKER = 1,
  /** Flat discriminator audio requiring deemphasis. Accepted value: `flat`. */
  RPTADV_RADIO_RX_AUDIO_FLAT = 2,
};

/** @brief Carrier-indication assignments retained for USBRadioPlus
 * compatibility. */
enum rptadv_radio_carrier_source {
  /** No carrier indication. Accepted symbolic value: `no`. */
  RPTADV_RADIO_CARRIER_DISABLED = 0,
  /** Native discriminator-noise squelch. Accepted value: `dsp`. */
  RPTADV_RADIO_CARRIER_DSP = 1,
  /** Audio-level carrier detector. Accepted value: `vox`. */
  RPTADV_RADIO_CARRIER_VOX = 2,
  /** USB GPIO carrier indication. Accepted value: `usb`. */
  RPTADV_RADIO_CARRIER_USB = 3,
  /** Inverted USB GPIO carrier indication. Accepted value: `usbinvert`. */
  RPTADV_RADIO_CARRIER_USB_INVERTED = 4,
  /** Parallel-port carrier indication. Accepted value: `pp`. */
  RPTADV_RADIO_CARRIER_PARALLEL = 5,
  /** Inverted parallel-port carrier indication. Accepted value: `ppinvert`. */
  RPTADV_RADIO_CARRIER_PARALLEL_INVERTED = 6,
};

/** @brief CTCSS-indication assignments retained for USBRadioPlus compatibility.
 */
enum rptadv_radio_ctcss_source {
  /** Do not require a CTCSS indication. Accepted symbolic value: `no`. */
  RPTADV_RADIO_CTCSS_DISABLED = 0,
  /** USB GPIO CTCSS indication. Accepted value: `usb`. */
  RPTADV_RADIO_CTCSS_USB = 1,
  /** Inverted USB GPIO CTCSS indication. Accepted value: `usbinvert`. */
  RPTADV_RADIO_CTCSS_USB_INVERTED = 2,
  /** Native CTCSS decoder. Accepted value: `dsp`. */
  RPTADV_RADIO_CTCSS_DSP = 3,
  /** Parallel-port CTCSS indication. Accepted value: `pp`. */
  RPTADV_RADIO_CTCSS_PARALLEL = 4,
  /** Inverted parallel-port CTCSS indication. Accepted value: `ppinvert`. */
  RPTADV_RADIO_CTCSS_PARALLEL_INVERTED = 5,
};

/** @brief Transmit-signaling turn-off assignments retained for USBRadioPlus
 * compatibility. */
enum rptadv_radio_tone_off_mode {
  /** End CTCSS with PTT release. Accepted symbolic value: `no`. */
  RPTADV_RADIO_TONE_OFF_NONE = 0,
  /** Shift CTCSS phase before PTT release. Accepted value: `ctcss_phase_shift`.
   */
  RPTADV_RADIO_TONE_OFF_PHASE_SHIFT = 1,
  /** Remove CTCSS before PTT release. Accepted value: `ctcss_tone_remove`. */
  RPTADV_RADIO_TONE_OFF_TONE_REMOVE = 2,
  /** Substitute a low-frequency tail tone. Accepted value: `ctcss_tail_tone`.
   */
  RPTADV_RADIO_TONE_OFF_TAIL_TONE = 3,
};

/**
 * @brief Fixed native-stream properties used to create one radio object.
 *
 * The object copies these values during creation.  They cannot change until
 * the caller destroys the object and opens a replacement stream.  Adapters
 * convert their hardware or Asterisk layout to the canonical stereo layout
 * before calling @ref rptadv_radio_descriptor::radio_tick.
 */
struct rptadv_radio_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Required descriptor ABI version. */
  uint32_t abi_version;
  /** Fixed native sample rate in hertz; only 48000 is supported. */
  uint32_t native_sample_rate_hz;
  /** Largest native tick that the caller will issue; it must be nonzero. */
  uint32_t maximum_frame_count;
  /** Interleaved channels; ABI version one requires canonical stereo. */
  uint32_t interleaved_channels;
};

/**
 * @brief Legacy-compatible raw PCM meter state.
 *
 * This layout deliberately matches the established ASL3 measurement units:
 * maximum absolute signed-16 code, mean square signed-16 codes, and the
 * historical adjacent-clip count for each retained 20 ms span.  It is
 * caller-owned so the real-time primitive performs no allocation or locking.
 */
struct rptadv_radio_audio_statistics {
  /** Peak absolute PCM code for each retained span. */
  uint16_t maxbuf[RPTADV_RADIO_AUDIO_STATS_LEN];
  /** Historical adjacent clipped-sample count for each retained span. */
  uint16_t clipbuf[RPTADV_RADIO_AUDIO_STATS_LEN];
  /** Mean square PCM-code power for each retained span. */
  uint32_t pwrbuf[RPTADV_RADIO_AUDIO_STATS_LEN];
  /** Next retained span slot. */
  int16_t index;
};

/**
 * @brief Caller-owned sample-clocked MICOR noise-squelch state.
 *
 * This layout contains only portable detector state.  A compatibility adapter
 * retains one instance beside its existing receive stage and passes it to
 * @ref rptadv_radio_descriptor::radio_micor_squelch_update once per native
 * noise-filter sample.  It owns no storage and is valid when zero initialized.
 */
struct rptadv_radio_micor_squelch_state {
  /** Smoothed discriminator-noise power in established squared units. */
  double noise_power;
  /** Slowly tracked no-carrier reference for strong-signal defeat. */
  double idle_power;
  /** Long-tail capacitor charge, normalized to a full charge of one. */
  double hold_charge;
  /** Native samples consumed during one-time detector settling. */
  uint32_t settling_samples;
};

/**
 * @brief Caller-owned state for the native discriminator receive frontend.
 *
 * The frontend retains its decimator phase, the fixed 960-native-sample RSSI
 * accumulator, and the sample-clocked MICOR comparator state here.  Filter
 * coefficients and signed-16 history remain caller-owned so a compatibility
 * adapter can retain its existing live configuration and fall back without a
 * stream restart.  The structure contains no allocation or synchronization.
 */
struct rptadv_radio_receive_frontend_state {
  /** Current native-to-baseband decimator phase. */
  int16_t decimator;
  /** Current MICOR comparator result; nonzero means receiver closed. */
  int16_t comparator_output;
  /** Most recently completed RSSI calibration value in legacy PCM units. */
  int16_t rssi_peak;
  /** Reserved for future ABI-compatible scalar alignment. */
  uint16_t reserved;
  /** Sum of squared filtered-noise samples in the current RSSI window. */
  int64_t rssi_power;
  /** Native samples accumulated in @ref rssi_power. */
  uint32_t rssi_samples;
  /** Retained sample-clocked MICOR noise-squelch state. */
  struct rptadv_radio_micor_squelch_state micor_squelch;
};

/**
 * @brief Caller-owned legacy envelope-measurement state.
 *
 * This state retains the extrema and decay counters used by the USBRadioPlus
 * VOX and calibration meter.  It contains no storage and is valid when zero
 * initialized.  The accompanying primitive accepts canonical F32 PCM, but
 * preserves these signed-16-compatible measurement units exactly.
 */
struct rptadv_radio_envelope_state {
  /** Current positive envelope extremum in legacy PCM codes. */
  int16_t maximum;
  /** Current negative envelope extremum in legacy PCM codes. */
  int16_t minimum;
  /** Most recent half peak-to-peak envelope in legacy PCM codes. */
  int16_t peak;
  /** Samples remaining before positive-envelope decay. */
  int32_t upper_decay_counter;
  /** Samples remaining before negative-envelope decay. */
  int32_t lower_decay_counter;
};

/**
 * @brief Caller-owned circular delay-line cursor and dirty state.
 *
 * The sample storage remains caller-owned canonical F32 PCM.  A compatibility
 * adapter can retain its historical storage beside this compact state and
 * fall back without transferring ownership or allocating in its audio path.
 */
struct rptadv_radio_delay_line_state {
  /** Next storage position to receive an input sample. */
  uint32_t input_index;
  /** Nonzero after storage contains delayed samples that require clearing. */
  uint32_t dirty;
};

/**
 * @brief Caller-owned legacy center-slicer detector state.
 *
 * USBRadioPlus uses this state while centering the low-speed-data signal
 * before CTCSS qualification.  The primitive operates on canonical F32 PCM,
 * but retains these signed-16-compatible extrema and detector counters
 * exactly.  It contains no storage and is valid when zero initialized.
 */
struct rptadv_radio_center_slicer_state {
  /** Current positive input extremum in legacy PCM codes. */
  int16_t maximum;
  /** Current negative input extremum in legacy PCM codes. */
  int16_t minimum;
  /** Most recent half peak-to-peak envelope in legacy PCM codes. */
  int16_t peak;
  /** Retained compatibility upper detector counter. */
  int32_t upper_decay_counter;
  /** Retained compatibility lower detector counter. */
  int32_t lower_decay_counter;
};

/**
 * @brief Caller-owned fixed-point state for receiver deemphasis.
 *
 * The compatibility receiver applies this recursive signed-32 accumulator to
 * flat discriminator audio after high-pass filtering. The primitive accepts
 * canonical F32 PCM but retains this exact fixed-point state for parity with
 * the established de-emphasis stage.
 */
struct rptadv_radio_deemphasis_integrator_state {
  /** Recursive signed-32 accumulator in historical fixed-point units. */
  int32_t accumulator;
};

/**
 * @brief Measurements collected while extracting one native receive span.
 *
 * @ref peak is expressed on the canonical normalized f32 scale.  A negative
 * full-scale signed-16 hardware input therefore reports `1.0F`, while a
 * positive signed-16 rail reports `32767.0F / 32768.0F`.  This retains the
 * distinction used by the existing signed-16 receiver meter without making
 * the portable core depend on a hardware sample representation.
 */
struct rptadv_radio_receive_extract_stats {
  /** Largest absolute left-channel receiver sample. */
  float peak;
  /** Number of left-channel samples at either signed-16 PCM rail. */
  uint64_t rail_samples;
};

/**
 * @brief Callback-boundary configuration for the legacy-compatible CTCSS
 * receive detector.
 *
 * @ref tone_mask selects CTCSS-table indexes 0 through
 * @ref RPTADV_RADIO_CTCSS_RECEIVE_TONE_COUNT minus one.  The input supplied
 * to the matching process operation is already the compatibility adapter's
 * filtered, center-sliced, mono 8 kHz PCM mapped to canonical F32.  Filtering,
 * device input, and signaling-mode selection remain outside this portable
 * primitive.
 */
struct rptadv_radio_ctcss_receive_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Bit mask of configured legacy CTCSS receive table indexes. */
  uint64_t tone_mask;
  /** Nonzero selects the legacy relaxed talk-off/release behavior. */
  uint32_t relax;
};

/** @brief Legacy receive signaling-mode assignments. */
enum rptadv_radio_signal_mode {
  /** No selected receive signaling mode. */
  RPTADV_RADIO_SIGNAL_MODE_NONE = 0,
  /** Legacy carrier-only signaling mode retained by compatibility callers. */
  RPTADV_RADIO_SIGNAL_MODE_CARRIER = 1,
  /** Receive CTCSS selected the active signaling mode. */
  RPTADV_RADIO_SIGNAL_MODE_CTCSS = 2,
  /** Receive DCS selected the active signaling mode. */
  RPTADV_RADIO_SIGNAL_MODE_DCS = 3,
  /** Legacy low-speed-data signaling mode retained by compatibility callers. */
  RPTADV_RADIO_SIGNAL_MODE_LOW_SPEED_DATA = 4,
};

/**
 * @brief Immutable configuration for one legacy signaling-mode advance.
 *
 * The compatibility control plane parses CTCSS strings and translates every
 * receive-to-transmit assignment to its exact historical tenths-of-a-hertz
 * selection before calling the portable primitive.  A mapping value of zero
 * means receive-only.  The default is intentionally signed: a nonzero legacy
 * no-default sentinel remains observable because the deployed C state machine
 * treated it as a requested selection after a decoded tone disappeared.
 */
struct rptadv_radio_signal_mode_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Configured signaling-mode hold duration in milliseconds. */
  int32_t hold_ms;
  /** One enables CTCSS-selected transmit-tone requests; zero disables them. */
  uint32_t ctcss_tx_enabled;
  /** Exact default transmit selection in signed tenths of hertz. */
  int32_t default_tx_ctcss_frequency_tenths_hz;
  /** Exact mapped selections indexed by decoded CTCSS table entry. */
  int32_t mapped_tx_ctcss_frequency_tenths_hz[
      RPTADV_RADIO_CTCSS_RECEIVE_TONE_COUNT];
};

/**
 * @brief One elapsed-time and decoder snapshot for a signaling-mode advance.
 *
 * `decoded_ctcss` is -1 when no CTCSS tone qualifies, otherwise it is an
 * index from zero through @ref RPTADV_RADIO_CTCSS_RECEIVE_TONE_COUNT minus
 * one.  Boolean fields must be zero or one.
 */
struct rptadv_radio_signal_mode_input {
  /** Whole elapsed milliseconds for this callback span. */
  int32_t elapsed_ms;
  /** Current CTCSS decode index, or -1 when no tone qualifies. */
  int32_t decoded_ctcss;
  /** One when the configured DCS receiver currently qualifies. */
  uint32_t dcs_valid;
  /** One while PTT input is asserted and the mode timer is frozen. */
  uint32_t tx_ptt_in;
};

/**
 * @brief Caller-owned legacy signaling-mode state.
 *
 * This contains only fields touched by @ref
 * rptadv_radio_descriptor::radio_signal_mode_advance.  Transmit startup and
 * turn-off state remain owned by the compatibility adapter until their
 * separate migration slice moves them together.  On failure this structure is
 * not modified.
 */
struct rptadv_radio_signal_mode_state {
  /** Current value from @ref rptadv_radio_signal_mode. */
  int32_t smode;
  /** Most recently selected signaling mode. */
  int32_t smode_was;
  /** Remaining signaling-mode hold duration in milliseconds. */
  int32_t smode_timer_ms;
  /** Previously acted-on decoded CTCSS table index. */
  int32_t last_rx_ctcss;
  /** Selected transmit CTCSS frequency in signed tenths of hertz. */
  int32_t tx_ctcss_frequency_tenths_hz;
  /** Historical renderer request; one asks it to select a new tone. */
  uint32_t tx_ctcss_option;
  /** Sticky indication that a signaling-mode timer expired. */
  uint32_t smode_turnoff;
};

/** @brief One-shot CTCSS renderer-control request. */
enum rptadv_radio_ctcss_render_option {
  /** Keep the current CTCSS renderer state. */
  RPTADV_RADIO_CTCSS_RENDER_OPTION_HOLD = 0,
  /** Start the selected normal CTCSS tone. */
  RPTADV_RADIO_CTCSS_RENDER_OPTION_START = 1,
  /** Start the configured CTCSS turn-off sequence. */
  RPTADV_RADIO_CTCSS_RENDER_OPTION_TURNOFF = 2,
  /** Disable CTCSS output on this renderer callback. */
  RPTADV_RADIO_CTCSS_RENDER_OPTION_DISABLE = 3,
};

/** @brief Persistent CTCSS oscillator render state. */
enum rptadv_radio_ctcss_render_oscillator_state {
  /** The CTCSS oscillator is disabled. */
  RPTADV_RADIO_CTCSS_RENDER_DISABLED = 0,
  /** The CTCSS oscillator emits its selected normal tone. */
  RPTADV_RADIO_CTCSS_RENDER_ACTIVE = 1,
  /** The CTCSS oscillator emits the configured turn-off sequence. */
  RPTADV_RADIO_CTCSS_RENDER_TURNOFF = 2,
};

/**
 * @brief Immutable configuration for one CTCSS render-state advance.
 *
 * The compatibility adapter supplies live values after its transmitter state
 * machine has selected phase-shift or tail-tone behavior.  This operation
 * neither selects a frequency nor retains configuration storage.
 */
struct rptadv_radio_ctcss_render_state_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Configured CTCSS turn-off duration in milliseconds. */
  int32_t turnoff_duration_ms;
  /** Phase adjustment applied to the first turn-off callback in degrees. */
  double turnoff_phase_shift_degrees;
  /** Replacement tone frequency used for a tail-tone turn-off in hertz. */
  double turnoff_tail_tone_hz;
};

/** @brief Elapsed native PCM duration for one CTCSS render-state advance. */
struct rptadv_radio_ctcss_render_state_input {
  /** Whole elapsed milliseconds for this native callback span. */
  int32_t elapsed_ms;
};

/**
 * @brief Caller-owned CTCSS state consumed by the renderer.
 *
 * This compact POD contains only fields touched by @ref
 * rptadv_radio_descriptor::radio_ctcss_render_state_advance.  PTT, CTCSS
 * frequency selection, DCS state, and oscillator phase accumulation remain
 * owned by their existing caller.  On failure this structure is not modified.
 */
struct rptadv_radio_ctcss_render_state {
  /** One value from @ref rptadv_radio_ctcss_render_option. */
  uint32_t option;
  /** One value from @ref rptadv_radio_ctcss_render_oscillator_state. */
  uint32_t oscillator_state;
  /** One while the selected CTCSS source should be emitted. */
  uint32_t enabled;
  /** Remaining active turn-off duration in milliseconds. */
  int32_t turnoff_remaining_ms;
  /** Phase adjustment for this renderer callback in degrees. */
  double phase_shift_degrees;
  /** Replacement tail-tone frequency for this renderer callback in hertz. */
  double tail_tone_hz;
};

/** @brief Legacy transmitter states exposed by portable transmitter transitions. */
enum rptadv_radio_tx_finish_state_code {
  /** The transmitter is idle after final completion cleanup. */
  RPTADV_RADIO_TX_STATE_IDLE = 0,
  /** The transmitter remains keyed while normal output history drains. */
  RPTADV_RADIO_TX_STATE_FINISHING = 4,
  /** The normal output drain completed in the current callback. */
  RPTADV_RADIO_TX_STATE_COMPLETE = 5,
};

/** @brief Elapsed native PCM duration for one normal transmitter drain entry. */
struct rptadv_radio_tx_finish_input {
  /** Whole elapsed milliseconds in the callback that begins the drain. */
  int32_t elapsed_ms;
};

/**
 * @brief Caller-owned state written by one normal transmitter drain entry.
 *
 * This compact POD contains only fields touched by the historical normal
 * finishing helper. PTT, CTCSS/DCS, hang, blanking, and device output remain
 * the compatibility adapter's responsibility. On failure it is not modified.
 */
struct rptadv_radio_tx_finish_state {
  /** Remaining historical output buffers to clear after this callback. */
  int32_t buffer_clear_frames;
  /** Remaining normal finishing duration in milliseconds. */
  int32_t finish_remaining_ms;
  /** One value from @ref rptadv_radio_tx_finish_state_code after success. */
  int32_t tx_state;
};

/** @brief Transmitter states accepted by the DCS turn-off transition. */
enum rptadv_radio_dcs_turnoff_state_code {
  /** Normal DCS program audio is active. */
  RPTADV_RADIO_DCS_TURNOFF_STATE_ACTIVE = 1,
  /** A selected DCS turn-off tail remains active. */
  RPTADV_RADIO_DCS_TURNOFF_STATE_TOC = 2,
};

/** @brief Immutable DCS turn-off duration selected by the compatibility caller. */
struct rptadv_radio_dcs_turnoff_config {
  /** DCS turn-off duration in whole milliseconds. */
  int32_t turnoff_duration_ms;
};

/** @brief Native-callback input for one selected DCS turn-off transition. */
struct rptadv_radio_dcs_turnoff_input {
  /** Whole native PCM milliseconds elapsed in this callback span. */
  int32_t elapsed_ms;
  /** One when the compatibility transmitter input PTT is asserted. */
  uint32_t tx_ptt_in;
  /** One only when the compatibility caller has selected a new DCS tail. */
  uint32_t begin_turnoff;
};

/**
 * @brief Caller-owned scalar state for one DCS transmitter turn-off transition.
 *
 * The portable operation reports a finishing request but does not perform that
 * transition. The compatibility caller retains PTT, DCS waveform phase,
 * hardware, and its established finishing helper. On failure this POD is not
 * modified.
 */
struct rptadv_radio_dcs_turnoff_state {
  /** One value from @ref rptadv_radio_dcs_turnoff_state_code. */
  int32_t tx_state;
  /** Remaining DCS turn-off duration in whole milliseconds. */
  int32_t dcs_turnoff_remaining_ms;
  /** Compatibility transmit-hang time cleared when a tail begins. */
  int32_t tx_hang_remaining_ms;
  /** One when the compatibility caller must enter its finishing helper. */
  uint32_t finish_requested;
  /** Native PCM milliseconds remaining after DCS-tail expiry. */
  int32_t finish_elapsed_ms;
};

/**
 * @brief Immutable configuration for transmitter completion cleanup.
 *
 * The compatibility adapter supplies its configured receiver-blanking time
 * unchanged.  The portable operation intentionally does not normalize that
 * legacy scalar so it preserves the established C state exactly.
 */
struct rptadv_radio_tx_complete_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** Receiver blanking duration armed after transmitter release. */
  int32_t txrx_blanking_time_ms;
};

/**
 * @brief Caller-owned scalar state changed after transmitter drain completion.
 *
 * This POD excludes device PTT, CTCSS display strings, waveform state, and
 * all I/O.  The compatibility adapter retains those responsibilities and may
 * execute its exact C cleanup if this append-only descriptor member is absent
 * or rejects a snapshot.  On failure this structure is not modified.
 */
struct rptadv_radio_tx_complete_state {
  /** Legacy transmitter state after cleanup. */
  int32_t tx_state;
  /** Logical transmitter PTT output after cleanup. */
  uint32_t tx_ptt_out;
  /** Historical one-shot CTCSS renderer option after cleanup. */
  uint32_t tx_ctcss_option;
  /** Receiver blanking time armed after cleanup. */
  int32_t txrx_blanking_timer_ms;
  /** Fractional native-sample remainder for the blanking timer. */
  uint32_t txrx_blanking_sample_remainder;
  /** Historical indication that CTCSS output is ready for its renderer. */
  uint32_t tx_ctcss_ready;
};

/**
 * @brief Input snapshot for one post-transmit receive-blanking advance.
 *
 * The compatibility adapter advances its fractional sample remainder before
 * this pure operation.  It retains ownership of the signed-16 capture buffer
 * and mutes the returned leading-frame count at the existing hardware
 * boundary.
 */
struct rptadv_radio_rx_blanking_input {
  /** Whole native PCM milliseconds elapsed during this callback span. */
  int32_t elapsed_ms;
  /** Number of native PCM frames supplied in this callback span. */
  uint32_t native_frame_count;
  /** Fractional native-frame remainder before elapsed-time advancement. */
  uint32_t sample_remainder_before;
};

/**
 * @brief Caller-owned state updated after one receive-blanking span.
 *
 * On error the shared object leaves this state unchanged so the compatibility
 * adapter can execute its exact C fallback.  This operation allocates
 * nothing, locks nothing, and performs no I/O.
 */
struct rptadv_radio_rx_blanking_state {
  /** Remaining receiver blanking duration in whole milliseconds. */
  int32_t remaining_ms;
  /** Leading native frames that the adapter must mute. */
  uint32_t blanked_frame_count;
};

/**
 * @brief One VOX detector and elapsed-time snapshot.
 *
 * The compatibility caller obtains @ref detector_active from its retained
 * 8 kHz envelope stage.  This pure scalar operation neither reads PCM nor
 * retains any pointer supplied by the caller.
 */
struct rptadv_radio_vox_carrier_input {
  /** One when the legacy VOX envelope comparator is currently active. */
  uint32_t detector_active;
  /** Configured VOX hang duration in whole milliseconds. */
  int32_t hang_time_ms;
  /** Whole native PCM milliseconds elapsed in this callback span. */
  int32_t elapsed_ms;
};

/**
 * @brief Caller-owned legacy VOX carrier state.
 *
 * The operation preserves the historical update order: an active detector
 * first reloads @ref remaining_ms, then the elapsed duration is consumed, and
 * the carrier remains asserted for that callback even if the consume reaches
 * zero.  On error this POD remains unchanged for the C fallback.
 */
struct rptadv_radio_vox_carrier_state {
  /** Remaining VOX carrier hold interval in whole milliseconds. */
  int32_t remaining_ms;
  /** One while the legacy carrier detector reports receive activity. */
  uint32_t carrier_detect;
};

/**
 * @brief Actions the compatibility adapter applies around RX CPU saving.
 *
 * The portable core selects the action but never touches a DSP stage.  The
 * adapter retains ownership of the legacy high-pass and deemphasis enables.
 */
enum rptadv_radio_rx_cpu_saver_action {
  /** No receiver-DSP enable state changed. */
  RPTADV_RADIO_RX_CPU_SAVER_ACTION_NONE = 0,
  /** Disable the receive DSP stages before the native frontend. */
  RPTADV_RADIO_RX_CPU_SAVER_ACTION_ENTER = 1,
  /** Re-enable the receive DSP stages before the native frontend. */
  RPTADV_RADIO_RX_CPU_SAVER_ACTION_LEAVE = 2,
};

/**
 * @brief Immutable legacy receiver CPU-saver activity snapshot.
 *
 * All members are normalized booleans.  The native adapter derives them from
 * the existing carrier, signaling, and PTT state without transferring DSP or
 * PCM ownership to the portable core.
 */
struct rptadv_radio_rx_cpu_saver_input {
  /** One when receiver CPU saving is configured. */
  uint32_t enabled;
  /** One when the existing carrier decision is asserted. */
  uint32_t carrier_detect;
  /** One when the retained signaling mode is idle. */
  uint32_t signal_mode_null;
  /** One when logical transmitter input PTT is asserted. */
  uint32_t tx_ptt_in;
  /** One when logical transmitter output PTT is asserted. */
  uint32_t tx_ptt_out;
};

/**
 * @brief Caller-owned receiver CPU-saver transition result.
 *
 * The operation updates this POD only on success.  A rejected result leaves
 * it unchanged so the caller can execute its exact C compatibility branch.
 */
struct rptadv_radio_rx_cpu_saver_state {
  /** One while receive DSP processing is halted. */
  uint32_t halted;
  /** One of @ref rptadv_radio_rx_cpu_saver_action. */
  uint32_t action;
};

/**
 * @brief Immutable legacy transmitter CPU-saver activity snapshot.
 *
 * This pure scalar input contains the existing compatibility booleans only.
 * The portable operation has no PCM, renderer, device, or signaling
 * ownership.
 */
struct rptadv_radio_tx_cpu_saver_input {
  /** One when transmitter CPU saving is configured. */
  uint32_t enabled;
  /** One when logical transmitter input PTT is asserted. */
  uint32_t tx_ptt_in;
  /** One when logical transmitter output PTT is asserted. */
  uint32_t tx_ptt_out;
  /** One when the retained compatibility transmitter is idle. */
  uint32_t tx_idle;
};

/**
 * @brief Caller-owned legacy transmitter CPU-saver result.
 *
 * On error the shared object leaves this POD unchanged so the compatibility
 * caller may execute its exact C branch.
 */
struct rptadv_radio_tx_cpu_saver_state {
  /** One when the compatibility caller must skip transmitter rendering. */
  uint32_t halted;
};

/**
 * @brief Callback-owned state of one native-parrot recording.
 *
 * The portable core owns these cursors while the caller owns the bound
 * canonical-f32 recording storage.  Every field is a current snapshot, not a
 * cumulative measurement.
 */
struct rptadv_radio_native_parrot_status {
  /** Number of samples currently retained in the recording. */
  uint64_t recorded_samples;
  /** Next retained sample to play when @ref playing is nonzero. */
  uint64_t playback_offset;
  /** Nonzero while the recording is being copied to transmitter audio. */
  uint32_t playing;
  /** Nonzero after a record call reached its configured sample limit. */
  uint32_t truncated;
};

/** @brief Hardware-output routing used by the bounded transmitter renderer. */
enum rptadv_radio_transmit_output_route {
  /** Do not add program audio or transmit signaling to this output. */
  RPTADV_RADIO_TX_OUTPUT_DISABLED = 0,
  /** Add processed program audio without transmit signaling. */
  RPTADV_RADIO_TX_OUTPUT_VOICE = 1,
  /** Add transmit signaling without program audio. */
  RPTADV_RADIO_TX_OUTPUT_TONE = 2,
  /** Add both processed program audio and transmit signaling. */
  RPTADV_RADIO_TX_OUTPUT_COMPOSITE = 3,
  /** Add auxiliary program audio without transmit signaling. */
  RPTADV_RADIO_TX_OUTPUT_AUX_VOICE = 4,
};

/**
 * @brief Per-output signaling calibration and routing for one render call.
 *
 * All calibration values are canonical f32 values expressed against 32767 PCM
 * codes: `1.0F` represents positive full-scale output.  This lets a temporary
 * compatibility adapter normalize its existing PCM-code settings without
 * giving the portable core a non-f32 signal interface.
 */
struct rptadv_radio_transmit_render_config {
  /** Size of this structure supplied by the caller. */
  uint32_t struct_size;
  /** One value from @ref rptadv_radio_transmit_output_route for output A. */
  uint32_t output_a_route;
  /** One value from @ref rptadv_radio_transmit_output_route for output B. */
  uint32_t output_b_route;
  /** Output-A CTCSS amplitude in normalized PCM-code units. */
  float ctcss_peak_a;
  /** Output-A CTCSS calibration bias in normalized PCM-code units. */
  float ctcss_bias_a;
  /** Output-B CTCSS amplitude in normalized PCM-code units. */
  float ctcss_peak_b;
  /** Output-B CTCSS calibration bias in normalized PCM-code units. */
  float ctcss_bias_b;
};

/**
 * @brief Versioned function table exported by the radio-core shared object.
 *
 * Lifecycle functions are control-plane operations.  Only @ref radio_tick is
 * real-time capable.  Its caller provides readable input and writable output
 * storage for exactly @p frame_count interleaved frames.  The tick never
 * allocates, locks, blocks, logs, performs I/O, or changes stream properties.
 */
struct rptadv_radio_descriptor {
  /** Size of this descriptor. */
  uint32_t struct_size;
  /** ABI implemented by every function in this table. */
  uint32_t abi_version;
  /** Stable capability name. */
  const char *capability_name;
  /** Create one object after validating the fixed stream configuration. */
  enum rptadv_radio_result (*radio_create)(
      const struct rptadv_radio_config *config, struct rptadv_radio **radio);
  /**
   * @brief Consume and produce exactly @p frame_count canonical native frames.
   *
   * This minimal bootstrap core validates both PCM buffers and fills a valid
   * output block with canonical silence.  Future portable radio behavior will
   * retain the same bounded, allocation-free tick contract.
   */
  enum rptadv_radio_result (*radio_tick)(struct rptadv_radio *radio,
                                         const float *input, float *output,
                                         uint32_t frame_count);
  /**
   * @brief Repeat canonical f32 samples with optional gain and muting.
   *
   * @param input Readable input containing @p sample_count samples when not
   * muted.
   * @param output Writable output containing @p sample_count samples.
   * @param sample_count Number of interleaved canonical samples, not frames.
   * @param gain Linear gain applied to every unmuted sample.
   * @param muted Zero to copy input times @p gain; one to write canonical
   * silence.
   * @return @ref RPTADV_RADIO_OK, or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The output may alias the input exactly for in-place gain adjustment; other
   * partial overlap is invalid.  A muted call does not read @p input, so it
   * may be NULL.  A zero-sample call is a valid no-op with NULL buffers.
   */
  enum rptadv_radio_result (*radio_repeat_f32)(const float *input,
                                               float *output,
                                               uint32_t sample_count,
                                               float gain, uint32_t muted);
  /** Destroy an object; passing NULL is safe. */
  void (*radio_destroy)(struct rptadv_radio *radio);
  /**
   * @brief Quantize and route one bounded transmitter block to stereo i16 PCM.
   *
   * @param radio Radio object that declares the maximum valid frame count.
   * @param program Canonical program audio with @p frame_count samples.
   * @param ctcss Unit-amplitude canonical CTCSS samples with @p frame_count
   * samples.
   * @param dcs Canonical DCS samples already normalized from PCM-code levels.
   * @param frame_count Number of native mono program frames to render.
   * @param config Routing and per-output CTCSS calibration.
   * @param stereo In/out interleaved signed-16 stereo PCM with @p frame_count
   * frames.
   * @param meter_stereo Optional stereo meter output for unrouted program
   * audio.
   * @param program_rail_samples Receives program samples beyond i16 PCM range.
   * @return @ref RPTADV_RADIO_OK or an argument/frame-count failure.
   *
   * Program, DCS, and calibration values use 32767 PCM codes per normalized
   * unit.  CTCSS is unit amplitude before `ctcss_peak_a` or
   * `ctcss_peak_b` is applied.  The renderer preserves the legacy order:
   * quantize program for metering, saturating-add routed program, then
   * quantize and saturating-add routed CTCSS/DCS.  It allocates nothing and
   * does not access hardware.  `meter_stereo` may be NULL; every other buffer
   * is required when @p frame_count is nonzero.
   */
  enum rptadv_radio_result (*radio_render_transmit_f32)(
      const struct rptadv_radio *radio, const float *program,
      const float *ctcss, const float *dcs, uint32_t frame_count,
      const struct rptadv_radio_transmit_render_config *config, int16_t *stereo,
      int16_t *meter_stereo, uint64_t *program_rail_samples);
  /**
   * @brief Check whether a frequency exactly names a supported CTCSS tone.
   * @param frequency_hz Requested CTCSS frequency in hertz.
   * @return Nonzero for an exact signaling-table entry; zero otherwise.
   */
  uint32_t (*radio_ctcss_frequency_supported)(float frequency_hz);
  /**
   * @brief Map a requested tone to the legacy 8 kHz table-increment frequency.
   * @param frequency_hz Requested CTCSS frequency in hertz.
   * @return The compatibility oscillator frequency in hertz.
   */
  double (*radio_ctcss_legacy_frequency)(double frequency_hz);
  /**
   * @brief Return the reference CTCSS peak for one legacy detector filter.
   * @param frequency_hz Requested CTCSS frequency in hertz.
   * @param filter_250 Nonzero selects the 250 Hz table; zero selects 215 Hz.
   * @return Peak level in signed-16 PCM codes.
   */
  double (*radio_ctcss_legacy_peak)(double frequency_hz, uint32_t filter_250);
  /**
   * @brief Calculate the maximum calibrated CTCSS reference level.
   * @param frequency_hz Requested CTCSS frequency in hertz.
   * @param filter_250 Nonzero selects the 250 Hz table; zero selects 215 Hz.
   * @param tone_gain_q8 Legacy tone multiplier with eight fractional bits.
   * @param output_gain_q8 Legacy output multiplier with eight fractional bits.
   * @return The maximum absolute reference level in signed-16 PCM codes.
   */
  double (*radio_ctcss_legacy_scaled_peak)(double frequency_hz,
                                           uint32_t filter_250,
                                           int32_t tone_gain_q8,
                                           int32_t output_gain_q8);
  /**
   * @brief Calculate calibrated legacy CTCSS amplitude and DC bias.
   * @param frequency_hz Requested CTCSS frequency in hertz.
   * @param filter_250 Nonzero selects the 250 Hz table; zero selects 215 Hz.
   * @param tone_gain_q8 Legacy tone multiplier with eight fractional bits.
   * @param output_gain_q8 Legacy output multiplier with eight fractional bits.
   * @param amplitude_pcm_codes Receives oscillator amplitude in PCM codes.
   * @param bias_pcm_codes Receives DC bias in PCM codes.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   */
  enum rptadv_radio_result (*radio_ctcss_legacy_scaled_levels)(
      double frequency_hz, uint32_t filter_250, int32_t tone_gain_q8,
      int32_t output_gain_q8, double *amplitude_pcm_codes,
      double *bias_pcm_codes);
  /**
   * @brief Render a bounded, phase-continuous legacy CTCSS f32 block.
   * @param radio Radio object created for @ref
   * RPTADV_RADIO_CTCSS_NATIVE_SAMPLE_RATE_HZ.
   * @param output Writable canonical mono f32 output with @p frame_count
   * samples.
   * @param frame_count Native samples to render; it must not exceed setup
   * maximum.
   * @param frequency_hz Requested CTCSS frequency before legacy quantization.
   * @param peak Canonical f32 oscillator amplitude.
   * @param enabled Nonzero emits CTCSS; zero emits canonical silence.
   * @param phase_shift_degrees One-shot shift applied before an enabled block.
   * @return @ref RPTADV_RADIO_OK, an argument/bounds error, or
   *         @ref RPTADV_RADIO_UNSUPPORTED for another native rate.
   *
   * The radio retains oscillator phase across blocks.  A disabled or invalid
   * signal clears output without advancing phase, matching the compatibility
   * generator.  The function allocates nothing and performs no I/O.
   */
  enum rptadv_radio_result (*radio_ctcss_generate_f32)(
      struct rptadv_radio *radio, float *output, uint32_t frame_count,
      double frequency_hz, float peak, uint32_t enabled,
      double phase_shift_degrees);
  /**
   * @brief Render a bounded, phase-continuous exact-frequency CTCSS tail f32
   * block.
   * @param radio Radio object created for @ref
   * RPTADV_RADIO_CTCSS_NATIVE_SAMPLE_RATE_HZ.
   * @param output Writable canonical mono f32 output with @p frame_count
   * samples.
   * @param frame_count Native samples to render; it must not exceed setup
   * maximum.
   * @param frequency_hz Exact tail-tone frequency in hertz.
   * @param peak Canonical f32 oscillator amplitude.
   * @param enabled Nonzero emits the tail tone; zero emits canonical silence.
   * @return @ref RPTADV_RADIO_OK, an argument/bounds error, or
   *         @ref RPTADV_RADIO_UNSUPPORTED for another native rate.
   */
  enum rptadv_radio_result (*radio_ctcss_generate_tail_f32)(
      struct rptadv_radio *radio, float *output, uint32_t frame_count,
      double frequency_hz, float peak, uint32_t enabled);
  /**
   * @brief Read the CTCSS oscillator phase retained by one radio object.
   * @param radio Radio object whose transmitter oscillator is queried.
   * @param phase_radians Receives the phase in radians.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   */
  enum rptadv_radio_result (*radio_ctcss_phase_radians)(
      const struct rptadv_radio *radio, double *phase_radians);
  /**
   * @brief Check whether a numeric value fits the three-octal-digit DCS field.
   * @param code Candidate DCS value.
   * @return Nonzero for values from 000 through 777; zero otherwise.
   */
  uint32_t (*radio_dcs_code_supported)(int32_t code);
  /**
   * @brief Configure native DCS transmit word and polarity.
   * @param radio Radio object whose DCS transmitter is configured.
   * @param code DCS code expressed as an integer from 000 through 777.
   * @param inverted Nonzero selects inverse DCS polarity.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * An unsupported code retains the legacy disabled configuration outcome. The
   * call resets the fractional symbol accumulator, while preserving the 23-bit
   * repeated-word phase and the independent DCS turn-off oscillator phase.
   */
  enum rptadv_radio_result (*radio_dcs_configure_transmit)(
      struct rptadv_radio *radio, int32_t code, uint32_t inverted);
  /**
   * @brief Render bounded DCS data or its 134.4 Hz turn-off signal in f32 PCM.
   * @param radio Radio object created for the fixed stream sample rate.
   * @param output Writable canonical mono f32 output with @p frame_count
   * samples.
   * @param frame_count Native samples to render; it must not exceed setup
   * maximum.
   * @param peak Canonical f32 peak amplitude.
   * @param enabled Nonzero emits DCS; zero emits canonical silence.
   * @param turnoff Nonzero selects the 134.4 Hz DCS turn-off signal.
   * @return @ref RPTADV_RADIO_OK or an argument/frame-count failure.
   *
   * Normal output uses the configured TIA/ETSI 23-bit word, least-significant
   * bit first, at 134.4 baud. A disabled or nonpositive-peak call clears output
   * without advancing either DCS clock. The function allocates nothing and
   * performs no I/O.
   */
  enum rptadv_radio_result (*radio_dcs_generate_f32)(
      struct rptadv_radio *radio, float *output, uint32_t frame_count,
      float peak, uint32_t enabled, uint32_t turnoff);
  /**
   * @brief Read the DCS turn-off oscillator phase retained by one radio object.
   * @param radio Radio object whose DCS tail oscillator is queried.
   * @param phase_radians Receives the phase in radians.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   */
  enum rptadv_radio_result (*radio_dcs_tail_phase_radians)(
      const struct rptadv_radio *radio, double *phase_radians);
  /**
   * @brief Extract delayed left-channel receiver PCM from canonical stereo f32.
   *
   * @param radio Fixed stream object that bounds @p frame_count.
   * @param stereo Readable canonical interleaved stereo input.
   * @param mono Writable delayed normalized left-channel output.
   * @param frame_count Number of native frames to extract.
   * @param delay Writable circular normalized-f32 delay storage, or NULL when
   *              @p delay_frame_count is zero.
   * @param delay_frame_count Number of samples in @p delay.
   * @param delay_index In/out circular offset; an out-of-range input is reset
   *                    to zero before the first delayed sample.
   * @param stats Receives the raw left-channel peak and rail count.
   * @return @ref RPTADV_RADIO_OK or an argument/frame-count failure.
   *
   * This is a bounded, allocation-free native-tick primitive.  It preserves
   * the established receiver ordering: measure the undelayed left channel,
   * then replace it with the oldest delay sample before publishing the mono
   * output.  It does not apply gain, filtering, qualification, or DSP.
   */
  enum rptadv_radio_result (*radio_extract_receive_f32)(
      const struct rptadv_radio *radio, const float *stereo, float *mono,
      uint32_t frame_count, float *delay, uint32_t delay_frame_count,
      uint32_t *delay_index, struct rptadv_radio_receive_extract_stats *stats);
  /**
   * @brief Replace a native program block with the calibrated one-kilohertz
   * tone.
   * @param radio Radio object created for the 48 kHz native compatibility
   * stream.
   * @param program Writable canonical mono f32 program block.
   * @param frame_count Native frames to process; it must not exceed setup
   * maximum.
   * @param enabled Nonzero replaces the block; zero preserves it and resets
   * phase.
   * @return @ref RPTADV_RADIO_OK, an argument/bounds error, or
   *         @ref RPTADV_RADIO_UNSUPPORTED for another native rate.
   *
   * An enabled call writes the established 1 kHz, 7518-code-peak calibration
   * source after all transmitter processing, just before hardware routing.  A
   * disabled call intentionally does not alter @p program; it resets the
   * oscillator so the next enabled block starts at a zero crossing.  The
   * function allocates nothing and performs no I/O.
   */
  enum rptadv_radio_result (*radio_render_calibrated_test_tone_f32)(
      struct rptadv_radio *radio, float *program, uint32_t frame_count,
      uint32_t enabled);
  /**
   * @brief Read the calibrated test-tone oscillator phase for diagnostics.
   * @param radio Radio object whose transmitter oscillator is queried.
   * @param phase_radians Receives the phase in radians.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   */
  enum rptadv_radio_result (*radio_calibrated_test_tone_phase_radians)(
      const struct rptadv_radio *radio, double *phase_radians);
  /**
   * @brief Bind idle native-parrot state to caller-owned canonical F32 storage.
   * @param radio Radio object that owns the native-parrot cursors.
   * @param storage Stable writable canonical F32 recording storage.
   * @param storage_capacity Number of samples allocated at @p storage.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The caller allocates and frees @p storage outside real-time processing and
   * keeps it valid until the radio object is destroyed or rebound after a
   * reset.  Binding is accepted only while no recording or playback state is
   * retained.  The function performs no allocation or I/O.
   */
  enum rptadv_radio_result (*radio_native_parrot_bind_f32)(
      struct rptadv_radio *radio, float *storage, uint32_t storage_capacity);
  /**
   * @brief Clear native-parrot recording and playback cursors.
   * @param radio Radio object whose bound native-parrot state is cleared.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * Bound storage remains allocated and attached.  This is safe at a callback
   * boundary and does not read or overwrite stored audio.
   */
  enum rptadv_radio_result (*radio_native_parrot_reset)(
      struct rptadv_radio *radio);
  /**
   * @brief Apply one receiver-key transition to native-parrot state.
   * @param radio Radio object that owns the native-parrot cursors.
   * @param was_keyed Previous receiver-key state.
   * @param is_keyed Current receiver-key state.
   * @param playback_started Receives nonzero when unkeying began playback.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * A key-up clears the current recording.  An unkey starts playback only
   * when a recording contains samples.  These are the existing native echo
   * transition rules; nonzero key values are treated as keyed.
   */
  enum rptadv_radio_result (*radio_native_parrot_rx_transition)(
      struct rptadv_radio *radio, uint32_t was_keyed, uint32_t is_keyed,
      uint32_t *playback_started);
  /**
   * @brief Append one bounded native F32 block to a native-parrot recording.
   * @param radio Radio object that owns the bound recording storage.
   * @param input Readable canonical F32 input with @p frame_count samples.
   * @param frame_count Native samples to record; it must not exceed setup
   * maximum.
   * @param recording_limit Maximum retained samples for the active recording.
   * @param recorded Receives the number of samples copied from @p input.
   * @return @ref RPTADV_RADIO_OK, an argument error, or
   *         @ref RPTADV_RADIO_FRAME_COUNT_EXCEEDED.
   *
   * The function records a prefix when a block crosses @p recording_limit and
   * marks the retained state truncated.  The limit must not exceed the bound
   * storage capacity.  It allocates nothing and leaves playback state intact.
   */
  enum rptadv_radio_result (*radio_native_parrot_record_f32)(
      struct rptadv_radio *radio, const float *input, uint32_t frame_count,
      uint32_t recording_limit, uint32_t *recorded);
  /**
   * @brief Copy the next native-parrot playback span to canonical F32 output.
   * @param radio Radio object that owns the bound recording storage.
   * @param output Writable canonical F32 output with @p frame_count samples.
   * @param frame_count Native samples available at @p output.
   * @param played Receives the number of samples copied to @p output.
   * @return @ref RPTADV_RADIO_OK, an argument error, or
   *         @ref RPTADV_RADIO_FRAME_COUNT_EXCEEDED.
   *
   * A call copies at most the retained tail, changes no unwritten samples, and
   * clears the playing state after the last sample.  It allocates nothing and
   * does not access hardware.
   */
  enum rptadv_radio_result (*radio_native_parrot_play_f32)(
      struct rptadv_radio *radio, float *output, uint32_t frame_count,
      uint32_t *played);
  /**
   * @brief Read a current native-parrot state snapshot.
   * @param radio Radio object whose native-parrot state is queried.
   * @param status Receives retained counts and flags.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   */
  enum rptadv_radio_result (*radio_native_parrot_status)(
      const struct rptadv_radio *radio,
      struct rptadv_radio_native_parrot_status *status);
  /**
   * @brief Parse a case-insensitive `rxdemod` symbolic assignment.
   * @param text NUL-terminated symbolic value; no trimming or aliases apply.
   * @param mode Receives one @ref rptadv_radio_rx_audio_mode numeric value.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * On failure, this function does not alter @p mode.  It accepts exactly
   * `no`, `speaker`, and `flat`, case-insensitively.
   */
  enum rptadv_radio_result (*radio_parse_rx_audio_mode)(const char *text,
                                                        uint32_t *mode);
  /**
   * @brief Parse a case-insensitive `carrierfrom` symbolic assignment.
   * @param text NUL-terminated symbolic value; no trimming or aliases apply.
   * @param source Receives one @ref rptadv_radio_carrier_source numeric value.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * Accepted values are `no`, `dsp`, `vox`, `usb`, `usbinvert`, `pp`, and
   * `ppinvert`.  On failure, this function does not alter @p source.
   */
  enum rptadv_radio_result (*radio_parse_carrier_source)(const char *text,
                                                         uint32_t *source);
  /**
   * @brief Parse a case-insensitive `ctcssfrom` symbolic assignment.
   * @param text NUL-terminated symbolic value; no trimming or aliases apply.
   * @param source Receives one @ref rptadv_radio_ctcss_source numeric value.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * Accepted values are `no`, `usb`, `usbinvert`, `dsp`, `pp`, and
   * `ppinvert`.  On failure, this function does not alter @p source.
   */
  enum rptadv_radio_result (*radio_parse_ctcss_source)(const char *text,
                                                       uint32_t *source);
  /**
   * @brief Parse a case-insensitive CTCSS turn-off symbolic assignment.
   * @param text NUL-terminated symbolic value; no trimming or aliases apply.
   * @param mode Receives one @ref rptadv_radio_tone_off_mode numeric value.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * Accepted values are `no`, `ctcss_phase_shift`, `ctcss_tone_remove`, and
   * `ctcss_tail_tone`.  On failure, this function does not alter @p mode.
   */
  enum rptadv_radio_result (*radio_parse_tone_off_mode)(const char *text,
                                                        uint32_t *mode);
  /**
   * @brief Configure one native DCS receive decoder.
   * @param radio Fixed native-stream object that owns the decoder state.
   * @param code Nine-bit three-octal-digit DCS value, or a negative/invalid
   *             value to disable receive decoding.
   * @param inverted Nonzero selects inverse DCS polarity.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * Configuration clears the 16 symbol-timing hypotheses, Golay qualification,
   * DC tracker, and turn-off detector without disturbing transmitter state.
   * This operation belongs to setup or a callback boundary, not a concurrent
   * control thread.
   */
  enum rptadv_radio_result (*radio_dcs_configure_receive)(
      struct rptadv_radio *radio, int32_t code, uint32_t inverted);
  /**
   * @brief Process left-channel native F32 discriminator PCM for DCS.
   * @param radio Fixed native-stream object that owns the decoder state.
   * @param stereo Readable canonical interleaved stereo F32 PCM.
   * @param frame_count Native frames to process; it must not exceed setup
   * maximum.
   * @param valid Receives nonzero only while the configured DCS code qualifies.
   * @return @ref RPTADV_RADIO_OK, an argument error, or
   *         @ref RPTADV_RADIO_FRAME_COUNT_EXCEEDED.
   *
   * The input must be the raw hardware-bound mapping of signed 16-bit
   * discriminator PCM divided by 32768.0; this reconstructs legacy sample
   * codes exactly.  The primitive preserves the established 16-phase symbol
   * bank, Golay correction, one-word loss hold, Q15 DC tracker, and coherent
   * 134.4 Hz turn-off-tail clearing.  It allocates nothing and performs no I/O.
   */
  enum rptadv_radio_result (*radio_dcs_process_receive_f32)(
      struct rptadv_radio *radio, const float *stereo, uint32_t frame_count,
      uint32_t *valid);
  /**
   * @brief Configure the portable legacy-compatible CTCSS receive decoder.
   * @param radio Fixed native-stream object that owns the decoder state.
   * @param config Selected CTCSS table indexes and relaxed-talk-off setting.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * Calling this resets CTCSS qualification and release blanking without
   * changing the independent transmitter oscillator or DCS state.  It is a
   * callback-boundary operation; callers retain ownership of the 8 kHz
   * frontend filter and configured receive-to-transmit mapping.
   */
  enum rptadv_radio_result (*radio_ctcss_configure_receive)(
      struct rptadv_radio *radio,
      const struct rptadv_radio_ctcss_receive_config *config);
  /**
   * @brief Decode post-filter mono 8 kHz CTCSS PCM.
   * @param radio Fixed native-stream object that owns detector state.
   * @param samples Canonical F32 mono samples mapped exactly from legacy i16
   * post-center-slicer PCM.
   * @param sample_count Number of 8 kHz samples, bounded by the stream maximum.
   * @param carrier_detect Nonzero while the existing receiver carrier detector
   * is asserted.
   * @param decoded Receives the selected CTCSS table index, or -1 when no
   * configured tone currently qualifies.
   * @return @ref RPTADV_RADIO_OK, an argument error, or
   * @ref RPTADV_RADIO_FRAME_COUNT_EXCEEDED.
   *
   * The detector retains XPMR-compatible frequency divisors, fixed-point
   * correlator qualification, relaxed talk-off behavior, and 200 ms release
   * blanking.  It allocates nothing and performs no I/O.
   */
  enum rptadv_radio_result (*radio_ctcss_process_receive_f32)(
      struct rptadv_radio *radio, const float *samples, uint32_t sample_count,
      uint32_t carrier_detect, int32_t *decoded);
  /**
   * @brief Measure raw 48 kHz canonical-F32 PCM in ASL3-compatible units.
   * @param samples Readable raw canonical F32 samples, or NULL for zero samples.
   * @param sample_count Scalar samples in @p samples, not audio frames.
   * @param channels One for 48 kHz mono or two for interleaved 48 kHz stereo.
   * @param statistics Caller-owned retained meter state.
   * @param clipping Receives nonzero when the historical clip condition occurs.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The primitive retains the exact historical downsample phase (index five
   * of each 48 kHz mono group, or index ten of each stereo group), 960/1920
   * scalar-sample bounds, 50-slot ring units, and adjacent-clip accounting.
   * Canonical values mapped from signed 16-bit PCM by dividing by 32768.0F
   * round-trip exactly. Other finite F32 values are rounded to nearest PCM
   * code and saturated before measurement; non-finite input is rejected
   * without changing @p statistics. This operation allocates nothing, locks
   * nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_measure_raw_pcm_f32)(
      const float *samples, uint32_t sample_count, uint32_t channels,
      struct rptadv_radio_audio_statistics *statistics, uint32_t *clipping);
  /**
   * @brief Advance one sample-clocked MICOR noise-squelch comparator.
   * @param state Caller-owned persistent detector state.
   * @param squelched Previous closed output; any nonzero value is closed.
   * @param sample_power Squared noise-filter sample in established units.
   * @param open_level Calibrated noise threshold for the initial comparator.
   * @param hysteresis Additional noise margin while the receiver is open.
   * @param closed Receives the new closed output as zero or one.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The primitive retains the established detector settling, fast defeat,
   * weak-signal charge, long release, and subnormal-state clamp behavior. It
   * allocates nothing, locks nothing, and performs no hardware or Asterisk I/O.
   */
  enum rptadv_radio_result (*radio_micor_squelch_update)(
      struct rptadv_radio_micor_squelch_state *state, uint32_t squelched,
      double sample_power, uint32_t open_level, uint32_t hysteresis,
      uint32_t *closed);
  /**
   * @brief Track a legacy-compatible decaying peak envelope from F32 PCM.
   * @param input Readable canonical F32 samples, or NULL for zero samples.
   * @param output Optional writable canonical F32 half-peak samples.
   * @param sample_count Number of scalar samples to process.
   * @param decay_factor Legacy envelope decay interval in samples.
   * @param threshold Legacy PCM-code comparator threshold.
   * @param state Caller-owned persistent envelope state.
   * @param comparator Receives one when the final peak meets @p threshold.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * Input is quantized to the historical signed-16 PCM domain before each
   * state update.  An optional output receives the exact half peak-to-peak
   * result after each corresponding update, mapped back to canonical F32.
   * The primitive allocates nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_measure_envelope_f32)(
      const float *input, float *output, uint32_t sample_count,
      int32_t decay_factor, int16_t threshold,
      struct rptadv_radio_envelope_state *state, uint32_t *comparator);
  /**
   * @brief Process one canonical-F32 circular delay-line span.
   * @param input Readable canonical F32 PCM, required while enabled.
   * @param output Writable canonical F32 PCM, required for nonzero processing
   * or a dirty reset.
   * @param sample_count Number of scalar samples in this call.
   * @param storage Caller-owned writable canonical F32 circular storage.
   * @param storage_capacity Number of samples in @p storage.
   * @param lead Input-to-output delay in samples; it must not exceed capacity.
   * @param state Caller-owned cursor and dirty state.
   * @param enabled Zero selects the historical disabled reset behavior.
   * @param outzero Nonzero selects the historical forced-silent reset behavior.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * With an enabled, non-silent stage, the primitive writes each input sample
   * to @p storage then emits the sample exactly @p lead positions behind it.
   * Disabled or forced-silent calls clear storage and output only while
   * @ref rptadv_radio_delay_line_state::dirty is nonzero; otherwise they leave
   * all PCM and cursor storage unchanged.  This preserves the established
   * squelch-tail reset behavior across arbitrary callback partitions.  The
   * primitive allocates nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_delay_line_f32)(
      const float *input, float *output, uint32_t sample_count, float *storage,
      uint32_t storage_capacity, uint32_t lead,
      struct rptadv_radio_delay_line_state *state, uint32_t enabled,
      uint32_t outzero);
  /**
   * @brief Center and limit one low-speed-data F32 PCM span.
   * @param input Readable canonical F32 samples, or NULL for zero samples.
   * @param centered_output Writable centered canonical F32 PCM.
   * @param limited_output Writable centered-and-limited canonical F32 PCM.
   * @param sample_count Number of scalar samples to process.
   * @param limit Legacy signed-PCM limiter magnitude.
   * @param setpoint Legacy peak-to-peak tracking set point.
   * @param decay_factor Legacy per-sample extremum discharge amount.
   * @param state Caller-owned extrema and retained detector counters.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The primitive quantizes every input sample into the historical signed-16
   * PCM domain before applying the exact min/max, center, and limiter steps.
   * It emits the centered and limited values after their historical signed-16
   * assignments, including their fixed-point wrap behavior.  It allocates
   * nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_center_slicer_f32)(
      const float *input, float *centered_output, float *limited_output,
      uint32_t sample_count, int32_t limit, int16_t setpoint,
      int32_t decay_factor, struct rptadv_radio_center_slicer_state *state);
  /**
   * @brief Apply the legacy receiver-deemphasis recursive filter to F32 PCM.
   * @param input Readable canonical F32 PCM, or NULL for zero samples.
   * @param output Writable canonical F32 PCM after historical i16 narrowing.
   * @param sample_count Number of scalar samples to process.
   * @param output_coefficient Historical feed-forward coefficient.
   * @param feedback_coefficient Historical recursive feedback coefficient.
   * @param output_gain Historical Q8 output gain.
   * @param state Caller-owned recursive signed-32 accumulator.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The primitive quantizes input into legacy signed-16 PCM, uses wrapping
   * signed-32 multiplication and addition with truncation-toward-zero
   * division, then narrows every output to signed-16 before returning
   * canonical F32. It allocates nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_deemphasis_integrator_f32)(
      const float *input, float *output, uint32_t sample_count,
      int16_t output_coefficient, int16_t feedback_coefficient,
      int32_t output_gain,
      struct rptadv_radio_deemphasis_integrator_state *state);
  /**
   * @brief Apply the legacy mono, unit-rate FIR to canonical F32 PCM.
   * @param input Readable canonical F32 PCM, or NULL for zero samples.
   * @param output Writable canonical F32 PCM after historical rail clipping.
   * @param sample_count Number of scalar samples to process.
   * @param history Caller-owned signed-16 FIR history with @p history_count entries.
   * @param history_count Active history and coefficient count; it must be nonzero.
   * @param coefficients Readable signed-16 fixed-point FIR coefficient table.
   * @param input_gain Historical Q8 input gain.
   * @param output_gain Historical post-accumulator Q8 output gain.
   * @param calc_adjust Historical FIR accumulation normalization divisor.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * This narrow primitive preserves the deployed mono, unit-rate FIR arithmetic:
   * every F32 input sample is quantized to signed-16 PCM, history is shifted,
   * fixed-point products accumulate in signed-64 space, and the historical
   * division/multiply/division order precedes the asymmetric `[-32767,32767]`
   * clamp. Interpolation, routed output mixing, and amplitude detection remain
   * compatibility-adapter responsibilities. The caller owns all state and
   * storage; this primitive allocates nothing, locks nothing, and performs no
   * I/O.
   */
  enum rptadv_radio_result (*radio_fir_mono_f32)(
      const float *input, float *output, uint32_t sample_count,
      int16_t *history, uint32_t history_count,
      const int16_t *coefficients, int32_t input_gain,
      int32_t output_gain, int32_t calc_adjust);
  /**
   * @brief Run the legacy-compatible native discriminator receive frontend.
   * @param input Readable interleaved canonical-F32 stereo native PCM.
   * @param baseband_output Writable canonical-F32 mono decimated PCM.
   * @param baseband_output_capacity Capacity of @p baseband_output samples.
   * @param carrier_gate Writable native-frame gate; one means receiver open.
   * @param carrier_gate_capacity Capacity of @p carrier_gate entries.
   * @param native_frame_count Native stereo frames to consume.
   * @param history Caller-owned signed-16 frontend history.
   * @param history_count Entries in @p history and @p baseband_coefficients.
   * @param baseband_coefficients Readable signed-16 baseband FIR coefficients.
   * @param baseband_calc_adjust Historical baseband FIR divisor.
   * @param baseband_output_gain Historical Q8 baseband gain.
   * @param noise_coefficients Readable signed-16 noise-bandpass coefficients.
   * @param noise_coefficient_count Entries in @p noise_coefficients.
   * @param noise_divisor Historical noise-filter normalization divisor.
   * @param decimate Native frames per baseband sample.
   * @param calibration_window Native samples in one fixed RSSI window.
   * @param open_level Legacy DSP-squelch threshold.
   * @param hysteresis Legacy DSP-squelch hysteresis.
   * @param state Caller-owned frontend state.
   * @param baseband_output_count Receives emitted baseband sample count.
   * @param rssi_updated Receives one when a complete RSSI window ended.
   * @return @ref RPTADV_RADIO_OK or an argument/frame-capacity failure.
   *
   * The primitive quantizes F32 input to the historical signed-16 domain,
   * applies the deployed byte-count history shift, computes native
   * discriminator noise and decimated baseband PCM, and updates MICOR once
   * per native frame.  It allocates nothing, locks nothing, and performs no
   * I/O.  VOX, diagnostic trace, and other compatibility-only frontend modes
   * remain adapter responsibilities.
   */
  enum rptadv_radio_result (*radio_receive_frontend_f32)(
      const float *input, float *baseband_output,
      uint32_t baseband_output_capacity, uint8_t *carrier_gate,
      uint32_t carrier_gate_capacity, uint32_t native_frame_count,
      int16_t *history, uint32_t history_count,
      const int16_t *baseband_coefficients, int32_t baseband_calc_adjust,
      int32_t baseband_output_gain, const int16_t *noise_coefficients,
      uint32_t noise_coefficient_count, int32_t noise_divisor,
      uint32_t decimate, uint32_t calibration_window, uint32_t open_level,
      uint32_t hysteresis, struct rptadv_radio_receive_frontend_state *state,
      uint32_t *baseband_output_count, uint32_t *rssi_updated);
  /**
   * @brief Convert one native PCM duration to elapsed whole milliseconds.
   * @param remainder In/out sub-millisecond native-frame remainder.
   * @param native_frame_count Native frames in this callback span.
   * @param milliseconds Receives the whole elapsed milliseconds.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * This preserves the legacy 48 kHz sample-clocked timer arithmetic across
   * arbitrary native callback partitions. It allocates nothing, locks
   * nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_elapsed_ms)(
      uint32_t *remainder, uint32_t native_frame_count, int32_t *milliseconds);
  /**
   * @brief Consume elapsed milliseconds from one compatibility timer.
   * @param timer In/out timer value in milliseconds.
   * @param milliseconds Elapsed duration to consume.
   * @param remaining Receives the duration after expiration, if any.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * A nonpositive timer or elapsed duration is intentionally a no-op. An
   * expiry clears @p timer and returns the residual duration so a successor
   * signaling state begins at the same native PCM sample time.
   */
  enum rptadv_radio_result (*radio_timer_consume)(
      int32_t *timer, int32_t milliseconds, int32_t *remaining);
  /**
   * @brief Advance the legacy CTCSS/DCS signaling-mode resolver.
   * @param config Immutable parsed signaling configuration.
   * @param input Elapsed-time, decoder, and PTT snapshot.
   * @param state Caller-owned state to update only on success.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The resolver preserves the historical order: a CTCSS decode can claim an
   * idle/CTCSS mode first, then a DCS decode can claim only an idle/DCS mode.
   * It freezes the hold timer while PTT is asserted, leaves the historical
   * turn-off indication sticky, and requests CTCSS renderer option one only
   * when a nonzero selected frequency changes.  It allocates nothing, locks
   * nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_signal_mode_advance)(
      const struct rptadv_radio_signal_mode_config *config,
      const struct rptadv_radio_signal_mode_input *input,
      struct rptadv_radio_signal_mode_state *state);
  /**
   * @brief Advance the final legacy CTCSS renderer-control transition.
   * @param config Immutable live turn-off configuration.
   * @param input Elapsed native PCM duration.
   * @param state Caller-owned renderer state updated only on success.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The operation consumes start, turn-off, and disable requests produced by
   * the compatibility transmitter state machine.  It resets phase for each
   * callback, applies a phase shift only to the first turn-off callback, and
   * delays disable by one callback after an active turn-off timer expires. It
   * allocates nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_ctcss_render_state_advance)(
      const struct rptadv_radio_ctcss_render_state_config *config,
      const struct rptadv_radio_ctcss_render_state_input *input,
      struct rptadv_radio_ctcss_render_state *state);
  /**
   * @brief Enter the fixed normal legacy transmitter finishing drain.
   * @param input Elapsed native PCM duration in the entry callback.
   * @param state Caller-owned finishing state updated only on success.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The operation clears three historical output buffers and holds the
   * transmitter through the entry callback plus those three 20 ms spans. It
   * consumes @p input immediately, so an entry callback of 80 ms or longer
   * reaches @ref RPTADV_RADIO_TX_STATE_COMPLETE in that same callback. It
   * allocates nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_tx_finish_advance)(
      const struct rptadv_radio_tx_finish_input *input,
      struct rptadv_radio_tx_finish_state *state);
  /**
   * @brief Advance an already-entered legacy transmitter finishing drain.
   * @param input Elapsed native PCM duration for this callback.
   * @param state Caller-owned finishing state updated only on success.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The operation preserves the historical restored-timer rule: a positive
   * buffer-clear count with a zero timer re-seeds the timer at 20 ms per
   * buffer. It then consumes the callback duration and marks completion at
   * expiry. It allocates nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_tx_finish_continue)(
      const struct rptadv_radio_tx_finish_input *input,
      struct rptadv_radio_tx_finish_state *state);
  /**
   * @brief Apply the post-drain legacy transmitter completion cleanup.
   * @param config Immutable receiver-blanking configuration.
   * @param state Caller-owned scalar transmitter state updated only on success.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The operation clears the logical PTT output, requests CTCSS disable,
   * resets blanking's sample remainder, arms receiver blanking, returns the
   * state to idle, and marks CTCSS output ready. It does not perform hardware
   * I/O, clear display strings, select signaling, or generate PCM. It
   * allocates nothing and locks nothing.
   */
  enum rptadv_radio_result (*radio_tx_complete)(
      const struct rptadv_radio_tx_complete_config *config,
      struct rptadv_radio_tx_complete_state *state);
  /**
   * @brief Advance pure post-transmit receiver-blanking timing.
   * \param input Elapsed duration, native span, and pre-call remainder.
   * \param state Caller-owned blanking state updated only on success.
   * \return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * The adapter retains signed-16 PCM ownership and mutes the returned prefix
   * immediately before its receive frontend.  This operation preserves the
   * established fixed-48-kHz timing arithmetic, allocates nothing, locks
   * nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_rx_blanking_advance)(
      const struct rptadv_radio_rx_blanking_input *input,
      struct rptadv_radio_rx_blanking_state *state);
  /**
   * @brief Advance the legacy VOX carrier-hang decision.
   * @param input Detector, configured hang, and elapsed-time snapshot.
   * @param state Caller-owned timer and carrier output updated on success.
   * @return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * This append-only scalar primitive preserves legacy same-callback expiry:
   * the carrier is asserted when the pre-consume timer was positive, even if
   * consuming @p input leaves the timer at zero. It allocates nothing, locks
   * nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_vox_carrier_advance)(
      const struct rptadv_radio_vox_carrier_input *input,
      struct rptadv_radio_vox_carrier_state *state);
  /**
   * @brief Advance the legacy transmitter CPU-saver halt decision.
   * \param input Existing saver, PTT, and idle-state snapshot.
   * \param state Caller-owned halt result updated only on success.
   * \return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * This append-only scalar operation returns the historical halt decision:
   * CPU saving is active only while saving is enabled, both logical PTT values
   * are clear, and the transmitter is idle. It allocates nothing, locks
   * nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_tx_cpu_saver_advance)(
      const struct rptadv_radio_tx_cpu_saver_input *input,
      struct rptadv_radio_tx_cpu_saver_state *state);
  /**
   * @brief Advance the legacy receiver CPU-saver transition decision.
   * \param input Existing saver, carrier, signaling, and PTT snapshot.
   * \param state Caller-owned halt/action result updated only on success.
   * \return @ref RPTADV_RADIO_OK or @ref RPTADV_RADIO_INVALID_ARGUMENT.
   *
   * This append-only scalar operation preserves the historical receiver halt
   * predicate and reports enter/leave transitions without modifying DSP
   * stages.  It allocates nothing, locks nothing, and performs no I/O.
   */
  enum rptadv_radio_result (*radio_rx_cpu_saver_advance)(
      const struct rptadv_radio_rx_cpu_saver_input *input,
      struct rptadv_radio_rx_cpu_saver_state *state);
  /**
   * @brief Advance one selected DCS transmitter turn-off transition.
   * @param config Immutable selected DCS turn-off duration.
   * @param input Elapsed duration, PTT, and new-tail selection snapshot.
   * @param state Caller-owned ACTIVE/TOC state and completion request.
   * @return @ref RPTADV_RADIO_OK on a transactional update, otherwise an error.
   *
   * This operation owns only scalar state/timer arithmetic. It does not select
   * DCS behavior, render its waveform, key PTT, touch hardware, or enter the
   * compatibility finishing state itself.
   */
  enum rptadv_radio_result (*radio_dcs_turnoff_advance)(
      const struct rptadv_radio_dcs_turnoff_config *config,
      const struct rptadv_radio_dcs_turnoff_input *input,
      struct rptadv_radio_dcs_turnoff_state *state);
};

/**
 * @brief Return the immutable descriptor for this shared-object ABI.
 * @return Never-null pointer valid while the shared object remains loaded.
 */
const struct rptadv_radio_descriptor *rptadv_radio_descriptor(void);

#ifdef __cplusplus
}
#endif

#endif
