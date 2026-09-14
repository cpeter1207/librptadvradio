# librptadvradio

`librptadvradio` is the portable, versioned radio engine shared by
rpt_advanced and USBRadioPlus adapters. It has no dependency on Asterisk,
ASL3, OSS, ALSA, PortAudio, Hamlib, HID, FFmpeg, or RNNoise.

ABI 3 owns one prepared runtime generation with independent serial receive and
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
Receive processing is returned to the adapter with qualification; ABI 3
intentionally has no direct software local repeat path. Transmit consumes only
the prepared program-ring port before native signaling and output routing.
Normal DCS NRZ and the DCS turn-off sine use distinct prepared shaping ports so
their established levels remain independent.

ABI 3 replaces ABI 2 rather than retaining its granular migration operations.
Consumers must validate the descriptor and SONAME before creating a session.
The complete C contract is documented in
`include/rptadvradio/rptadvradio.h`.

Build the shared object with `make`. Run `make ci` for the complete local
quality gate.
