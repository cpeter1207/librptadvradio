# Configured source speech analysis

The local alpha12 candidate extends standalone speech configuration in ABI 5
without moving existing callback offsets. The configured prefix ends at 184
bytes on native 64-bit platforms; preprocessing extends the configuration to
32 bytes and adds the capability word, followed by the local-session retirement
query. SONAME 5 and existing package names remain unchanged. Consumers must check
the appropriate descriptor prefix and capability bits before using extensions.
An older provider cannot satisfy this opt-in capability; do not read past its
published prefix. Existing radio sessions and speech_analysis_create retain
the 0.5 threshold/one-frame behavior.

The configured function takes finite inclusive threshold 0..1 and a positive
consecutive-frame count. Each observation is one actual 256-sample 16 kHz Earshot
frame. Partial input calls retain the latest decision and never count as a frame.
Below-threshold observations, reset, RF closure and source discontinuities clear
the pending streak. Qualified observations alone update the last-voice source
coordinate. The caller still owns pre-roll, admission and hangtime.

No programme storage, new resampler, classifier thread or Earshot patch is added.
The provider does not impose a caller's retention window; consumers must bound
their onset count to their own retained-audio contract. Publication requires the
normal native gates and coordinated downstream artifact updates. This document
does not assert that alpha12 has been released.


## Measured experiment, not an onset guarantee

The unchanged Earshot/SRC path was measured on the existing yes/no/noise
fixtures at 8 kHz and 48 kHz, at full and quarter amplitude, with identical
source offsets and seven-sample calls. The 8 kHz fixture path uses test-only
decimation, not a live telephone codec or representative caller recording.

For full-level yes at 48 kHz, first qualification occurred at 223.4 ms for
0.5/1, 271.4 ms for 0.5/4, 495.4 ms for 0.75/1 and 543.4 ms for 0.75/4.
Thus threshold selection added 272 ms before the four-frame requirement added
another 48 ms. Interrupted qualifying streaks can add more than 48 ms.

At threshold 0.75, both onset counts missed the entire 8 kHz no fixture at
both levels and the quarter-level 48 kHz no fixture. The same threshold
rejected an 8 kHz noise fixture admitted at 0.5; all settings rejected the
48 kHz noise fixture. These are limited fixture observations, not general
noise rejection or quiet-speech guarantees.

An initial test hypothesized that 0.75/4 would still meet the old regression's
10080-sample onset reference plus 160 ms. It failed with first qualification
at sample 26085. That reference is not independently annotated acoustic onset.
The failed hypothesis/log were retained separately; tests instead enforce
actual frame counts, source coordinates, resets, unchanged defaults, callback
storage behavior and caller-owned retained-window semantics. No delay buffer
was enlarged to disguise the sensitivity tradeoff.


## Explicit standalone history reset (local candidate)

Check the readable capability word and
`RPTADV_RADIO_SPEECH_ANALYSIS_RESET_CAPABILITY` (bit 4) before relying on full
`speech_analysis_reset`. This bit promises in-place reset for every accepted
standalone configuration: plain, bandpass, RNNoise, or both.

Reset clears analysis converters, FIR/RNNoise history, partial 480/256-sample
frames, classifier decisions, scratch and source-map tags. It retains prepared
storage, coefficients, handles and processing delay. It performs no RNNoise
processing or Earshot prediction, so reset creates no synthetic observation.
The next genuine source span supplies the new origin. SRC fractional phase is
retained by its provider; pre-origin startup positions remain unqualified and
are excluded before classifier input, not treated as previous-source evidence.

An error invalidates the decision and rejects further input until a successful
explicit reset retry. This differs deliberately from the unchanged implicit
source-jump retirement and local carrier/replacement policies. Caller-owned
programme ring, hold, admission, source age and programme RNNoise bypass are
untouched.

RNNoise-selected setup now requires the checked appended reset operation on
`librptadv_rnnoise_adapter.so.1`. A short descriptor or missing reset is rejected,
without falling back to plain analysis or binding upstream RNNoise directly.

### Dependency identity and publication block

The isolated qualification candidate is RNNoise adapter source
`b25f187d958c9866430bdf968c2b2bb579e5d667`, with adapter DSO SHA256
`7ae93fc02b44f7c3f4d5ae7fab7e5f0cec716458e03cf7f275f7b95129ed4816`
and upstream RNNoise DSO SHA256
`4d869024a18d1c03b6a9dabac6780af0ae1ad5424322ad7204fe9448aace67c1`.
It is dynamically selected through isolated library paths, not installed over
host libraries.

No qualified release version or package floor has been assigned to the reset
contract. Existing alpha2 RNNoise and alpha12 radio metadata do not identify a
released reset-capable pair. The historical Debian build/runtime lower bounds
remain unchanged and are insufficient to claim reset support. Publication is
blocked until the actual first qualified versions are assigned and synchronized
in Cargo, Makefile, Debian build/runtime dependencies and package metadata.
Native ARM64, complete package gates and production line/branch coverage remain
required; local amd64 evidence is not a release or hardware qualification.
