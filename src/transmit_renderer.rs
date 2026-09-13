//! Bounded transmitter routing and signed-16 hardware-edge quantization.

use std::ffi::c_int;

pub(crate) const PCM_CODE_SCALE: f32 = 32_767.0;
const PCM_MAXIMUM: f32 = i16::MAX as f32;
const PCM_MINIMUM: f32 = i16::MIN as f32;

/// One output assignment accepted by the transmitter renderer.
#[derive(Clone, Copy)]
pub(crate) enum OutputRoute {
    Disabled,
    Voice,
    Tone,
    Composite,
    AuxiliaryVoice,
}

impl OutputRoute {
    pub(crate) fn from_ffi(value: u32, invalid_argument: c_int) -> Result<Self, c_int> {
        match value {
            0 => Ok(Self::Disabled),
            1 => Ok(Self::Voice),
            2 => Ok(Self::Tone),
            3 => Ok(Self::Composite),
            4 => Ok(Self::AuxiliaryVoice),
            _ => Err(invalid_argument),
        }
    }

    fn has_program(self) -> bool {
        matches!(self, Self::Voice | Self::Composite | Self::AuxiliaryVoice)
    }

    fn has_tone(self) -> bool {
        matches!(self, Self::Tone | Self::Composite)
    }
}

/// Per-output calibration after C ABI structure validation.
pub(crate) struct RenderConfig {
    pub(crate) output_a: OutputRoute,
    pub(crate) output_b: OutputRoute,
    pub(crate) ctcss_peak_a: f32,
    pub(crate) ctcss_bias_a: f32,
    pub(crate) ctcss_peak_b: f32,
    pub(crate) ctcss_bias_b: f32,
}

fn normalized_to_pcm(value: f32) -> f32 {
    value * PCM_CODE_SCALE
}

fn quantize_pcm(value: f32) -> i16 {
    // C fmin/fmax map NaN to the non-NaN bound. Preserve that legacy result
    // before Rust's explicit integer conversion at the hardware edge.
    let bounded = if value.is_nan() || value > PCM_MAXIMUM {
        PCM_MAXIMUM
    } else if value < PCM_MINIMUM {
        PCM_MINIMUM
    } else {
        value
    };
    bounded.round_ties_even() as i16
}

fn render_tone(ctcss: f32, dcs: f32, peak: f32, bias: f32) -> i16 {
    // DCS is an absolute normalized PCM-code level, not a CTCSS amplitude;
    // consequently it is added after CTCSS calibration just as in the C path.
    quantize_pcm(normalized_to_pcm(ctcss * peak + bias + dcs))
}

/// Render one validated block without allocation or external interaction.
///
/// # Safety
///
/// The caller supplies readable mono f32 arrays and writable interleaved i16
/// arrays of the documented lengths. `meter_stereo`, when non-null, is also
/// writable for twice `frame_count` samples.
pub(crate) unsafe fn render(
    program: *const f32,
    ctcss: *const f32,
    dcs: *const f32,
    frame_count: u32,
    config: &RenderConfig,
    stereo: *mut i16,
    meter_stereo: *mut i16,
) -> u64 {
    let mut rails = 0_u64;
    for index in 0..frame_count as usize {
        let program_code = normalized_to_pcm(unsafe { program.add(index).read() });
        if !program_code.is_nan() && !(PCM_MINIMUM..=PCM_MAXIMUM).contains(&program_code) {
            rails += 1;
        }
        let program_output = quantize_pcm(program_code);
        let stereo_index = index * 2;
        if !meter_stereo.is_null() {
            unsafe {
                meter_stereo.add(stereo_index).write(program_output);
                meter_stereo.add(stereo_index + 1).write(program_output);
            }
        }
        if config.output_a.has_program() {
            unsafe {
                let output = stereo.add(stereo_index);
                output.write(output.read().saturating_add(program_output));
            }
        }
        if config.output_b.has_program() {
            unsafe {
                let output = stereo.add(stereo_index + 1);
                output.write(output.read().saturating_add(program_output));
            }
        }
        let ctcss_sample = unsafe { ctcss.add(index).read() };
        let dcs_sample = unsafe { dcs.add(index).read() };
        if config.output_a.has_tone() {
            let tone = render_tone(
                ctcss_sample,
                dcs_sample,
                config.ctcss_peak_a,
                config.ctcss_bias_a,
            );
            unsafe {
                let output = stereo.add(stereo_index);
                output.write(output.read().saturating_add(tone));
            }
        }
        if config.output_b.has_tone() {
            let tone = render_tone(
                ctcss_sample,
                dcs_sample,
                config.ctcss_peak_b,
                config.ctcss_bias_b,
            );
            unsafe {
                let output = stereo.add(stereo_index + 1);
                output.write(output.read().saturating_add(tone));
            }
        }
    }
    rails
}
