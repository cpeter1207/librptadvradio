# librptadvradio development rules

Initial-alpha clarification (2026-09-13): rpt_advanced ADR 0040 removes any
requirement for backward compatibility with earlier project alphas. Do not
retain compatibility-only code or descriptor slots. Preserve required current
behavior and external interoperability, and reject mismatched artifacts safely.

This repository follows the shared rpt_advanced project baseline.  The library
is a portable, versioned, dynamically linked radio-core boundary.  It has no
Asterisk, OSS, PortAudio, ALSA, Hamlib, HID, FFmpeg, or direct hardware
dependency.

The native tick is real-time safe: it must not allocate, lock, block, log, or
perform I/O.  It accepts and produces canonical interleaved normalized `f32`
PCM for exactly the requested bounded frame count.  Stream rate and channel
layout are setup properties and cannot change while an object is alive.

Run format, lint, and static analysis once before a push. Rust implementation
documentation is complete, warning-free Rustdoc. Use Doxygen only for the
unavoidable C ABI header or shim and do not duplicate Rustdoc narrative there.
Use native Debian 13 amd64 coverage for production Rust source only. Keep the C
ABI, Debian packaging, documentation, archive, and staged-install checks in
sync when a public interface changes. Never static-link or vendor a duplicate
of a separately released project library.
