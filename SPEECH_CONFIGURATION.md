# Configured source speech analysis

Candidate alpha12 appends speech_analysis_create_configured to ABI 5 without
changing any earlier field or callback offset. On native 64-bit platforms the
old descriptor prefix is 176 bytes and the new descriptor is 184 bytes; its sized
configuration is 20 bytes. SONAME 5 and the existing package names remain unchanged.
Consumers must check the configured tail's minimum readable size before use.
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
