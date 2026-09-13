//! Allocation-free compatibility parsers for radio-signaling assignments.
//!
//! These functions deliberately accept only the historical ASCII tokens.  The
//! compatibility adapter owns configuration-file policy, while this portable
//! core preserves the symbolic-to-numeric mapping without depending on
//! Asterisk configuration APIs.

use std::ffi::{CStr, c_char, c_int};

use crate::{RADIO_INVALID_ARGUMENT, RADIO_OK};

/// Native receive-audio source assignments shared with USBRadioPlus.
pub(crate) const RX_AUDIO_DISABLED: u32 = 0;
/// Speaker audio with radio-provided deemphasis.
pub(crate) const RX_AUDIO_SPEAKER: u32 = 1;
/// Flat discriminator audio requiring deemphasis.
pub(crate) const RX_AUDIO_FLAT: u32 = 2;

/// No carrier indication.
pub(crate) const CARRIER_DISABLED: u32 = 0;
/// Native discriminator-noise squelch.
pub(crate) const CARRIER_DSP: u32 = 1;
/// Audio-level carrier detection.
pub(crate) const CARRIER_VOX: u32 = 2;
/// USB GPIO carrier indication.
pub(crate) const CARRIER_USB: u32 = 3;
/// Inverted USB GPIO carrier indication.
pub(crate) const CARRIER_USB_INVERTED: u32 = 4;
/// Parallel-port carrier indication.
pub(crate) const CARRIER_PARALLEL: u32 = 5;
/// Inverted parallel-port carrier indication.
pub(crate) const CARRIER_PARALLEL_INVERTED: u32 = 6;

/// Do not require a CTCSS indication.
pub(crate) const CTCSS_DISABLED: u32 = 0;
/// USB GPIO CTCSS indication.
pub(crate) const CTCSS_USB: u32 = 1;
/// Inverted USB GPIO CTCSS indication.
pub(crate) const CTCSS_USB_INVERTED: u32 = 2;
/// Native CTCSS decoder indication.
pub(crate) const CTCSS_DSP: u32 = 3;
/// Parallel-port CTCSS indication.
pub(crate) const CTCSS_PARALLEL: u32 = 4;
/// Inverted parallel-port CTCSS indication.
pub(crate) const CTCSS_PARALLEL_INVERTED: u32 = 5;

/// End signaling with PTT release.
pub(crate) const TONE_OFF_NONE: u32 = 0;
/// Shift CTCSS phase before PTT release.
pub(crate) const TONE_OFF_PHASE_SHIFT: u32 = 1;
/// Remove CTCSS before PTT release.
pub(crate) const TONE_OFF_TONE_REMOVE: u32 = 2;
/// Substitute a low-frequency tail tone before PTT release.
pub(crate) const TONE_OFF_TAIL_TONE: u32 = 3;

/// Match one C configuration token exactly except for ASCII case.
///
/// USBRadioPlus historically used `strcasecmp()` on these ASCII tokens.  An
/// ASCII-only comparison preserves every accepted spelling while making
/// arbitrary non-UTF-8 configuration bytes safely fail rather than requiring
/// an allocation or locale-sensitive conversion.
fn parse_token(text: *const c_char, values: &[(&[u8], u32)]) -> Option<u32> {
    if text.is_null() {
        return None;
    }
    let bytes = unsafe { CStr::from_ptr(text) }.to_bytes();
    values
        .iter()
        .find_map(|(name, value)| bytes.eq_ignore_ascii_case(name).then_some(*value))
}

/// Write a parsed symbolic value without changing caller storage on failure.
fn parse_into(text: *const c_char, output: *mut u32, values: &[(&[u8], u32)]) -> c_int {
    if output.is_null() {
        return RADIO_INVALID_ARGUMENT;
    }
    let Some(value) = parse_token(text, values) else {
        return RADIO_INVALID_ARGUMENT;
    };
    unsafe { output.write(value) };
    RADIO_OK
}

/// Parse the established `rxdemod` symbolic assignment.
pub(crate) extern "C" fn radio_parse_rx_audio_mode(text: *const c_char, output: *mut u32) -> c_int {
    parse_into(
        text,
        output,
        &[
            (b"no", RX_AUDIO_DISABLED),
            (b"speaker", RX_AUDIO_SPEAKER),
            (b"flat", RX_AUDIO_FLAT),
        ],
    )
}

/// Parse the established `carrierfrom` symbolic assignment.
pub(crate) extern "C" fn radio_parse_carrier_source(
    text: *const c_char,
    output: *mut u32,
) -> c_int {
    parse_into(
        text,
        output,
        &[
            (b"no", CARRIER_DISABLED),
            (b"dsp", CARRIER_DSP),
            (b"vox", CARRIER_VOX),
            (b"usb", CARRIER_USB),
            (b"usbinvert", CARRIER_USB_INVERTED),
            (b"pp", CARRIER_PARALLEL),
            (b"ppinvert", CARRIER_PARALLEL_INVERTED),
        ],
    )
}

/// Parse the established `ctcssfrom` symbolic assignment.
pub(crate) extern "C" fn radio_parse_ctcss_source(text: *const c_char, output: *mut u32) -> c_int {
    parse_into(
        text,
        output,
        &[
            (b"no", CTCSS_DISABLED),
            (b"usb", CTCSS_USB),
            (b"usbinvert", CTCSS_USB_INVERTED),
            (b"dsp", CTCSS_DSP),
            (b"pp", CTCSS_PARALLEL),
            (b"ppinvert", CTCSS_PARALLEL_INVERTED),
        ],
    )
}

/// Parse the established `hardware_ctcss_turnoff_mode` symbolic assignment.
pub(crate) extern "C" fn radio_parse_tone_off_mode(text: *const c_char, output: *mut u32) -> c_int {
    parse_into(
        text,
        output,
        &[
            (b"no", TONE_OFF_NONE),
            (b"ctcss_phase_shift", TONE_OFF_PHASE_SHIFT),
            (b"ctcss_tone_remove", TONE_OFF_TONE_REMOVE),
            (b"ctcss_tail_tone", TONE_OFF_TAIL_TONE),
        ],
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;

    fn parse(parser: extern "C" fn(*const c_char, *mut u32) -> c_int, text: &str) -> (c_int, u32) {
        let text = CString::new(text).expect("test token cannot contain NUL");
        let mut value = u32::MAX;
        let result = parser(text.as_ptr(), &mut value);
        (result, value)
    }

    #[test]
    fn parser_tokens_preserve_all_legacy_assignments_and_ascii_case() {
        for (token, value) in [
            ("no", RX_AUDIO_DISABLED),
            ("SpEaKeR", RX_AUDIO_SPEAKER),
            ("FLAT", RX_AUDIO_FLAT),
        ] {
            assert_eq!(parse(radio_parse_rx_audio_mode, token), (RADIO_OK, value));
        }
        for (token, value) in [
            ("no", CARRIER_DISABLED),
            ("DSP", CARRIER_DSP),
            ("Vox", CARRIER_VOX),
            ("usb", CARRIER_USB),
            ("UsbInvert", CARRIER_USB_INVERTED),
            ("PP", CARRIER_PARALLEL),
            ("pPiNvErT", CARRIER_PARALLEL_INVERTED),
        ] {
            assert_eq!(parse(radio_parse_carrier_source, token), (RADIO_OK, value));
        }
        for (token, value) in [
            ("no", CTCSS_DISABLED),
            ("USB", CTCSS_USB),
            ("UsbInvert", CTCSS_USB_INVERTED),
            ("dsp", CTCSS_DSP),
            ("PP", CTCSS_PARALLEL),
            ("pPiNvErT", CTCSS_PARALLEL_INVERTED),
        ] {
            assert_eq!(parse(radio_parse_ctcss_source, token), (RADIO_OK, value));
        }
        for (token, value) in [
            ("no", TONE_OFF_NONE),
            ("CtCsS_PhAsE_ShIfT", TONE_OFF_PHASE_SHIFT),
            ("CTCSS_TONE_REMOVE", TONE_OFF_TONE_REMOVE),
            ("ctcss_tail_tone", TONE_OFF_TAIL_TONE),
        ] {
            assert_eq!(parse(radio_parse_tone_off_mode, token), (RADIO_OK, value));
        }
    }

    #[test]
    fn parser_failures_preserve_output_and_reject_unrecognized_tokens() {
        for parser in [
            radio_parse_rx_audio_mode,
            radio_parse_carrier_source,
            radio_parse_ctcss_source,
            radio_parse_tone_off_mode,
        ] {
            let mut value = 0x5a5a_5a5a_u32;
            assert_eq!(
                parser(c"flat ".as_ptr(), &mut value),
                RADIO_INVALID_ARGUMENT
            );
            assert_eq!(value, 0x5a5a_5a5a);
            assert_eq!(parser(c"yes".as_ptr(), &mut value), RADIO_INVALID_ARGUMENT);
            assert_eq!(value, 0x5a5a_5a5a);
            assert_eq!(parser(std::ptr::null(), &mut value), RADIO_INVALID_ARGUMENT);
            assert_eq!(value, 0x5a5a_5a5a);
            assert_eq!(
                parser(c"no".as_ptr(), std::ptr::null_mut()),
                RADIO_INVALID_ARGUMENT
            );
        }
    }
}
