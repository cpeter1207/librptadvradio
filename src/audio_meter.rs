//! Legacy-compatible raw PCM meter implemented on canonical F32 input.
//!
//! This module deliberately keeps the historical 48 kHz-to-8 kHz sample
//! selection and integer-power units used by ASL3's `ast_radio_check_audio`.
//! That lets compatibility adapters replace only the measurement primitive
//! without changing tune displays or receive-level calibration.

use super::{AudioStatistics, RADIO_INVALID_ARGUMENT};

/// Number of historical 20 ms meter slots retained by one meter state.
pub const STATS_LEN: usize = 50;

const CLIP_SAMPLE_THRESHOLD: u32 = 0x7eb0;
const CLIP_EVENT_MIN_SAMPLES: u16 = 3;
const PCM_SCALE: f32 = 32_768.0;
const MONO_SAMPLE_LIMIT: usize = 6 * 160;
const STEREO_SAMPLE_LIMIT: usize = 12 * 160;

/// Quantize a canonical sample to the signed-16 raw-PCM code domain.
///
/// Direct `i16 / 32768.0` mappings round-trip exactly.  Other finite F32
/// input is rounded to its nearest raw PCM code and saturated, which makes
/// the primitive useful at a genuine F32 hardware boundary without changing
/// the established signed-16 compatibility path.
pub(crate) fn f32_to_pcm_code(sample: f32) -> Result<i32, i32> {
    if !sample.is_finite() {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok((sample * PCM_SCALE)
        .round()
        .clamp(i32::from(i16::MIN) as f32, i32::from(i16::MAX) as f32) as i32)
}

/// Return the historical index/stride/maximum scalar count for a layout.
fn layout_parameters(channels: u32) -> Result<(usize, usize, usize), i32> {
    match channels {
        1 => Ok((5, 6, MONO_SAMPLE_LIMIT)),
        2 => Ok((10, 12, STEREO_SAMPLE_LIMIT)),
        _ => Err(RADIO_INVALID_ARGUMENT),
    }
}

/// Advance a legacy meter index after normalizing corrupt external state.
fn advance_index(index: i16) -> i16 {
    let index = if index < 0 || index as usize >= STATS_LEN {
        0
    } else {
        index as usize
    };
    ((index + 1) % STATS_LEN) as i16
}

/// Measure one raw 48 kHz canonical-F32 span in ASL3-compatible units.
///
/// `sample_count` counts scalar F32 elements, not frames.  `channels` must
/// be one for interleaved 48 kHz mono or two for interleaved 48 kHz stereo.
/// The selected samples use the established fifth native frame phase: index
/// five for mono and index ten for stereo.  This intentionally retains the
/// legacy clip-pair counter, including its non-resetting behavior across a
/// non-clipped gap.
pub fn measure(
    samples: *const f32,
    sample_count: u32,
    channels: u32,
    statistics: &mut AudioStatistics,
) -> Result<bool, i32> {
    let (first, stride, maximum) = layout_parameters(channels)?;
    let sample_count = usize::try_from(sample_count).map_err(|_| RADIO_INVALID_ARGUMENT)?;
    let sample_count = sample_count.min(maximum);
    let selected_count = sample_count / stride;

    if selected_count != 0 && samples.is_null() {
        return Err(RADIO_INVALID_ARGUMENT);
    }

    // Verify every selected input before changing the caller-owned ring so a
    // non-finite F32 callback input cannot leave a partial meter update.
    for offset in 0..selected_count {
        let sample = unsafe { samples.add(first + offset * stride).read() };
        f32_to_pcm_code(sample)?;
    }

    let mut next = *statistics;
    let index = if next.index < 0 || next.index as usize >= STATS_LEN {
        0
    } else {
        next.index as usize
    };
    if selected_count == 0 {
        next.index = advance_index(next.index);
        *statistics = next;
        return Ok(false);
    }

    let mut maximum = 0_u32;
    let mut power = 0.0_f64;
    let mut sequential_clips = 0_u16;
    let mut last_clip: Option<usize> = None;
    for offset in 0..selected_count {
        let sample = unsafe { samples.add(first + offset * stride).read() };
        let value = f32_to_pcm_code(sample)?.unsigned_abs();
        if value != 0 {
            maximum = maximum.max(value);
            power += f64::from(value * value);
            if value > CLIP_SAMPLE_THRESHOLD {
                if last_clip.is_some_and(|previous| previous + 1 == offset) {
                    sequential_clips += 1;
                }
                last_clip = Some(offset);
            }
        }
    }
    next.maxbuf[index] = maximum as u16;
    next.pwrbuf[index] = (power / selected_count as f64) as u32;
    next.clipbuf[index] = sequential_clips;
    next.index = advance_index(index as i16);
    *statistics = next;
    Ok(sequential_clips >= CLIP_EVENT_MIN_SAMPLES)
}
