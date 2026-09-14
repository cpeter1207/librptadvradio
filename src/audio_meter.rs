//! Canonical-F32 sample conversion used by the radio-session meters.

use super::RADIO_INVALID_ARGUMENT;

const PCM_SCALE: f32 = 32_768.0;

/// Quantize a canonical sample to the signed-16 hardware-code domain.
///
/// Direct `i16 / 32768.0` mappings round-trip exactly. Other finite F32 input
/// is rounded to its nearest hardware code and saturated.
pub(crate) fn f32_to_pcm_code(sample: f32) -> Result<i32, i32> {
    if !sample.is_finite() {
        return Err(RADIO_INVALID_ARGUMENT);
    }
    Ok((sample * PCM_SCALE)
        .round()
        .clamp(i32::from(i16::MIN) as f32, i32::from(i16::MAX) as f32) as i32)
}
