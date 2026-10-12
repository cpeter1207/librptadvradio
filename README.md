# librptadvradio

`librptadvradio` is the portable, versioned radio engine shared by
rpt_advanced and USBRadioPlus adapters. It has no dependency on Asterisk,
ASL3, OSS, ALSA, PortAudio, Hamlib, HID, FFmpeg, or RNNoise.

ABI 4 owns one prepared runtime generation with independent serial receive and
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
CTCSS qualification follows the configured carrier source: published hardware
COS for GPIO or parallel inputs, otherwise the native noise or VOX detector.
Receive processing is returned to the adapter with qualification; ABI 4
intentionally has no direct software local repeat path. Transmit consumes only
the prepared program-ring port before native signaling and output routing.
Normal DCS NRZ and the DCS turn-off sine use distinct prepared shaping ports so
their established levels remain independent.

ABI 4 replaces earlier granular migration operations.
Consumers must validate the descriptor and SONAME before creating a session.
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

For an alpha.7 to alpha.8 package upgrade, refresh the signed USBRadioPlus
APT index and install `librptadvradio4`, `librptadvsquelch2`, and
`librptadvctcss2` together; `librptadvdcs1` remains at ABI 1. A process using
radio alpha.8 needs the new provider SONAMEs available before it starts. The
alpha.7 squelch/CTCSS ABI-1 runtimes are not substitutes for ABI 2, and an
already running consumer must be restarted to load the upgraded libraries.

Build the shared object with `make`. Run `make ci` for the complete local
quality gate.
