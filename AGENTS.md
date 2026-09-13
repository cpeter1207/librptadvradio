# librptadvradio development rules

This repository follows the shared rpt_advanced project baseline.  The library
is a portable, versioned, dynamically linked radio-core boundary.  It has no
Asterisk, OSS, PortAudio, ALSA, Hamlib, HID, FFmpeg, or direct hardware
dependency.

The native tick is real-time safe: it must not allocate, lock, block, log, or
perform I/O.  It accepts and produces canonical interleaved normalized `f32`
PCM for exactly the requested bounded frame count.  Stream rate and channel
layout are setup properties and cannot change while an object is alive.

Run format, lint, static analysis, Doxygen, and Rustdoc once before a push.
Use native Debian 13 amd64 coverage for production Rust source only.  Keep C
ABI, Debian packaging, Doxygen, Rustdoc, archive, and staged-install checks in
sync when a public interface changes.  Never static-link or vendor a duplicate
of a separately released project library.
