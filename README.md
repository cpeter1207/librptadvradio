# librptadvradio

`librptadvradio` is the portable, versioned radio engine shared by
rpt_advanced and USBRadioPlus adapters. It has no direct dependency on Asterisk,
ASL3, OSS, ALSA, PortAudio, Hamlib, HID, FFmpeg, or RNNoise.

ABI 5 owns one prepared runtime generation with independent serial receive and
transmit callbacks. Both callbacks accept bounded variable frame counts and
operate on normalized `f32` PCM at the fixed 48 kHz native rate. Setup copies
high-level radio policy, selects the engine's fixed detector profiles,
preallocates every workspace and event queue, and warms borrowed processing
objects before real-time work starts. Callback work performs no allocation,
locking, blocking, logging, or device I/O.

External FFmpeg graphs, RNNoise, and the rate-adjusting program ring are
represented by narrow borrowed ports. The session owns their ordering but not
their implementation or lifetime. Prepared per-tone CTCSS notch ports let the
session select the decoded tone without rebuilding a graph in real time.
Carrier sources include GPIO, parallel inputs, DSP noise squelch, VOX, and an
independent external snapshot. External snapshots must be marked valid.
Open carrier keeps receive available while still requiring the selected CTCSS
or DCS qualifier. CTCSS qualification follows the configured carrier source.
Receive processing is returned to the adapter with qualification; ABI 5
intentionally has no direct software local repeat path. Transmit consumes only
the prepared program-ring port before native signaling and output routing.
Normal DCS NRZ and the DCS turn-off sine use distinct prepared shaping ports so
their established levels remain independent.

ABI 5 replaces earlier granular migration operations.
Consumers must validate the descriptor and SONAME before creating a session.

An optional sized ABI 5 descriptor tail provides prepared Earshot speech-analysis
handles without creating a radio session. Each handle accepts mono source PCM
at a fixed 8–192 kHz rate and converts only its detector input to 16 kHz. Results
report instantaneous voice/probability and the last positive input-source sample
position. Callers retain their own VAD hangtime, manual key state, qualification,
pre-roll and programme conversion. Check
`RPTADV_RADIO_DESCRIPTOR_SPEECH_ANALYSIS_MIN_SIZE` before using the four operations;
prepare/destroy off audio and serialize process/reset on the source owner.
Radio links dynamically to `librptadvsquelch.so.2` for MICOR noise squelch and
VOX hold timing. Building requires `librptadvsquelch-dev` 0.1.0~alpha2-1 or
newer, and running requires `librptadvsquelch2` of the same minimum version.
The radio session creates its squelch handle during setup; audio callbacks only
advance the already prepared detector.
Radio also links dynamically to `librptadvctcss.so.2` for conditioned 8 kHz
CTCSS receive detection and 48 kHz tone generation. Building requires
`librptadvctcss-dev` 0.1.0~alpha2-1 or newer, and running requires
`librptadvctcss2` of the same minimum version. Separate receive and transmit
handles and operations are prepared during session setup; callback conditioning,
render policy, and timers remain in radio.
Radio links dynamically to `librptadvdcs.so.1` for 48 kHz stereo-left receive
decoding and mono transmit generation. Building requires `librptadvdcs-dev`
0.1.0~alpha1-1 or newer, and running requires `librptadvdcs1` of the same
minimum version. Receive and transmit handles are separate. Prepared transmit
updates copy the new code and polarity while preserving live phase and tail
timing; turn-off scheduling and output routing remain in radio.
The complete C contract is documented in
`include/rptadvradio/rptadvradio.h`.

Refresh the signed USBRadioPlus APT index and install `librptadvradio5`,
`librptadvsquelch2`, `librptadvctcss2`, and `librptadvdcs1` together.
Restart the controller or Asterisk after upgrading to load the installed
libraries.

Build the shared object with `make`. Run `make ci` for the complete local
quality gate.

## Speech qualification

- `off`: preserve the current receiver behavior, delay and cost; no SRC or neural prediction.
- `kerchunk`: retain every configured COS/CTCSS/DCS requirement. Block audio and receive admission until the first fresh speech decision in the RF-qualified burst, then latch admission until that qualification closes. A subsequent burst must detect its own speech.
- `on`: require speech activity or its hang, together with every enabled RF qualification. When RF requirements are absent, speech is the receive qualification; disabled audio, stale required snapshots, duplex, blanking and post-transmit guards still apply.
- Configurable `vad_hang_ms`, default 1000, measured on the native sample clock. RF loss closes admission immediately. With no RF requirement capable of ending a burst, Kerchunk uses On semantics and emits a configuration diagnostic.
- Existing configurations default to Off.

Earshot 1.2.2 belongs to the portable, released `librptadvradio` receive owner immediately after squelch. It consumes exact 256-sample 16 kHz analysis frames at threshold 0.5. Keep programme PCM at 48 kHz. Reuse the released samplerate ABI2 through the same neutral descriptor-client pattern as the existing squelch/CTCSS/DCS clients. No duplicate resampler, detector, hardware owner, PCM delay queue, or background inference thread.

Use a minimum 160 ms reserve only when VAD is active; preserve any larger configured squelch delay. The existing local PCM ring/delay remains the only audible delay line. Keep capture/render flowing while speech admission is pending; mute delivery until admission, retaining the bounded lookback. Never infer admission from PCM energy or the ring's real-sample count.

ASL analyzes undelayed squelch-qualified speech and uses its existing delay storage. When VAD is enabled, run the existing de-emphasis once before analysis/storage; invalidate delay contents on RF loss and mode/storage transitions. Preserve the exact original Off path. Standalone continues to disable the radio's internal delay and uses its controller-owned PCM ring.

Building requires Rust 1.87 or newer and `librptadv-samplerate-adapter-dev`
0.2.0~alpha2-1 or newer. Radio links to `librptadv_samplerate_adapter.so.2`;
the existing converter performs analysis conversion while programme PCM stays
at 48 kHz. The C receive configuration uses `vad_mode`, `vad_hang_ms`
(0 through 32767), and `vad_external_delay` (zero internal, one external).
