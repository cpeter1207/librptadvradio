# librptadvradio

`librptadvradio` is the portable, versioned radio-core boundary shared by
rpt_advanced and USBRadioPlus adapters. It owns no device, Asterisk, OSS,
PortAudio, ALSA, Hamlib, HID, FFmpeg, or external-library dependency.

This initial ABI establishes the bounded variable-frame native-tick contract:
canonical interleaved normalized `f32` PCM at 48 kHz, fixed stream setup, opaque state,
and a real-time tick that does no allocation, I/O, logging, locking, or
configuration work. CTCSS and DCS decoding, tone generation, parrot storage,
transmit routing, metering, and receiver/signaling primitives are implemented
in Rust. USBRadioPlus currently invokes these operations through its C
compatibility boundary.

Only 48 kHz native streams are supported. Creation rejects other rates; codec
and app_rpt conversion belongs to their adapters, not this core. RNNoise and
native processing run at 48 kHz without rate conversion. The shared PCM ring
still corrects drift between independent clocks at equal nominal rates.
See rpt_advanced ADR 0035.

The top-level tick remains a silence bootstrap; it is not yet the complete
radio engine. Channel orchestration and processing are still being migrated.
The Rust-only `signaling_engine` now owns the composed receive detectors,
source qualification, transmitter signaling, and status-publication timing.
It retains sample-offset renderer intents and preallocated diagnostics;
adapters do not yet select it. Legacy receive voice output is a calibration
tap only, not a replacement for the delivered FFmpeg-processed audio.
The transitional transmitter-render interface converts canonical mono f32
program/signaling input to signed-16 stereo PCM at the existing hardware
boundary. It performs no device I/O.

Build a release library with `make`. Run `make ci` for formatting, strict
analysis, Doxygen, Rustdoc, tests, archive, staged-install, and Debian-package
checks. The public ABI is documented in
`include/rptadvradio/rptadvradio.h`.
