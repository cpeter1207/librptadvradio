//! Setup-only source selection, independent of any device or pin assignment.

use std::{ffi::c_int, mem::size_of};

use crate::session::{INVALID_ARGUMENT, SessionConfig, UNSUPPORTED};

/// Disabled CTCSS direction.
pub(crate) const DISABLED: u32 = 0;
/// Existing native DSP detector or generator.
pub(crate) const DSP: u32 = 1;
/// Normalized external decoder or encoder-enable signal.
pub(crate) const EXTERNAL: u32 = 2;

/// Immutable source assignments. Kind 0 is native audio; kind 1 is direct codec.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct Profile {
    pub(crate) struct_size: u32,
    pub(crate) kind: u32,
    /// 0 selects normalized hardware COR; 1 selects DSP noise squelch.
    pub(crate) carrier_source: u32,
    pub(crate) ctcss_encode_source: u32,
    pub(crate) ctcss_decode_source: u32,
}

/// Actual backend facilities; every capability is a 0/1 value.
#[repr(C)]
#[derive(Clone, Copy)]
pub(crate) struct ProfileCapabilities {
    pub(crate) struct_size: u32,
    pub(crate) hardware_cor: u32,
    pub(crate) external_ctcss_decode: u32,
    pub(crate) external_ctcss_encode: u32,
    pub(crate) dsp_tone_audio: u32,
}

/// Copied identity compared during updates; never borrows setup records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ValidatedProfile {
    pub(crate) kind: u32,
    pub(crate) carrier_source: u32,
    pub(crate) encode: u32,
    pub(crate) decode: u32,
    capabilities: [u32; 4],
}

impl ValidatedProfile {
    /// Validate directions separately; an unused missing capability is harmless.
    pub(crate) fn new(
        p: &Profile,
        caps: &ProfileCapabilities,
        config: &SessionConfig,
    ) -> Result<Self, c_int> {
        let capabilities = [
            caps.hardware_cor,
            caps.external_ctcss_decode,
            caps.external_ctcss_encode,
            caps.dsp_tone_audio,
        ];
        if p.struct_size < size_of::<Profile>() as u32
            || caps.struct_size < size_of::<ProfileCapabilities>() as u32
            || p.kind > 1
            || p.carrier_source > 1
            || p.ctcss_encode_source > EXTERNAL
            || p.ctcss_decode_source > EXTERNAL
            || capabilities.iter().any(|value| *value > 1)
        {
            return Err(INVALID_ARGUMENT);
        }
        let rx = config.receive;
        let tx = config.transmit;
        if (p.carrier_source == 0 && caps.hardware_cor == 0)
            || (p.ctcss_decode_source == EXTERNAL && caps.external_ctcss_decode == 0)
            || (p.ctcss_encode_source == EXTERNAL && caps.external_ctcss_encode == 0)
            || ((p.ctcss_encode_source == DSP || tx.dcs_transmit_enabled != 0)
                && caps.dsp_tone_audio == 0)
            || (rx.dcs_enabled != 0 && p.ctcss_decode_source != DISABLED)
            || (tx.dcs_transmit_enabled != 0 && p.ctcss_encode_source != DISABLED)
        {
            return Err(UNSUPPORTED);
        }
        if (rx.ctcss_enabled != u32::from(p.ctcss_decode_source == DSP))
            || (tx.ctcss_transmit_enabled != u32::from(p.ctcss_encode_source == DSP))
            || (p.ctcss_decode_source == DSP && rx.ctcss_tone_mask == 0)
            || (p.ctcss_encode_source == DSP && tx.default_ctcss_frequency_tenths_hz <= 0)
            || ((p.ctcss_encode_source == DSP || tx.dcs_transmit_enabled != 0)
                && ![tx.output_a_route, tx.output_b_route]
                    .iter()
                    .any(|v| matches!(v, 2 | 3)))
        {
            return Err(INVALID_ARGUMENT);
        }
        if p.ctcss_encode_source == EXTERNAL
            && (tx.default_ctcss_frequency_tenths_hz != 0
                || tx.mapped_ctcss_frequency_tenths_hz.iter().any(|v| *v != 0)
                || tx.ctcss_peak != 0.0
                || !matches!(tx.tone_off_mode, 0 | 2)
                || tx.ctcss_turnoff_phase_shift_degrees != 0.0
                || tx.ctcss_turnoff_tail_tone_hz != 0.0)
        {
            return Err(UNSUPPORTED);
        }
        Ok(Self {
            kind: p.kind,
            carrier_source: p.carrier_source,
            encode: p.ctcss_encode_source,
            decode: p.ctcss_decode_source,
            capabilities,
        })
    }

    /// Resolve modern selections into the existing shared receive pipeline.
    pub(crate) fn configure(self, mut config: SessionConfig) -> SessionConfig {
        config.qualification.carrier_source = if self.carrier_source == 0 { 3 } else { 1 };
        config.qualification.subaudible_source = if self.decode == EXTERNAL { 1 } else { 3 };
        config
    }

    /// A required unavailable snapshot must not be bypassed by CTCSS override.
    pub(crate) fn snapshots_valid(self, cor_valid: bool, ctcss_valid: bool) -> bool {
        (self.carrier_source != 0 || cor_valid) && (self.decode != EXTERNAL || ctcss_valid)
    }
}
