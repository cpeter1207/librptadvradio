//! Fixed receive-detector profiles hidden behind the session ABI.
//!
//! These tables and fixed-point normalization values preserve the deployed
//! discriminator-noise and CTCSS detector response. They are implementation
//! details: callers select only the documented high-level noise profile.

/// Fixed FIR coefficients and their historical normalization divisor.
#[derive(Clone, Copy)]
pub(crate) struct FilterProfile {
    pub(crate) coefficients: &'static [i16],
    pub(crate) divisor: i32,
}

const BASEBAND_3KHZ: [i16; 66] = [
    259, 58, -185, -437, -654, -793, -815, -696, -434, -48, 414, 886, 1_284, 1_523, 1_529, 1_254,
    691, -117, -1_078, -2_049, -2_854, -3_303, -3_220, -2_472, -995, 1_187, 3_952, 7_086, 10_300,
    13_270, 15_672, 17_236, 17_778, 17_236, 15_672, 13_270, 10_300, 7_086, 3_952, 1_187, -995,
    -2_472, -3_220, -3_303, -2_854, -2_049, -1_078, -117, 691, 1_254, 1_529, 1_523, 1_284, 886,
    414, -48, -434, -696, -815, -793, -654, -437, -185, 58, 259, 393,
];

const NOISE_STANDARD: [i16; 66] = [
    139, -182, -269, -66, 56, 59, 250, 395, -80, -775, -557, 437, 779, 210, -17, 123, -692, -1_664,
    -256, 2_495, 2_237, -1_018, -2_133, -478, -1_134, -2_711, 2_642, 10_453, 4_010, -14_385,
    -16_488, 6_954, 23_030, 6_954, -16_488, -14_385, 4_010, 10_453, 2_642, -2_711, -1_134, -478,
    -2_133, -1_018, 2_237, 2_495, -256, -1_664, -692, 123, -17, 210, 779, 437, -557, -775, -80,
    395, 250, 59, 56, -66, -269, -182, 139, 257,
];

const NOISE_ALTERNATE: [i16; 66] = [
    581, -251, -1_027, -766, 63, 346, 148, 459, 1_165, 847, -824, -1_994, -1_147, 462, 704, 32,
    651, 2_277, 1_790, -1_635, -4_071, -2_240, 1_060, 1_127, -502, 1_963, 7_399, 5_862, -6_693,
    -17_483, -10_387, 10_549, 22_110, 10_549, -10_387, -17_483, -6_693, 5_862, 7_399, 1_963, -502,
    1_127, 1_060, -2_240, -4_071, -1_635, 1_790, 2_277, 651, 32, 704, 462, -1_147, -1_994, -824,
    847, 1_165, 459, 148, 346, 63, -766, -1_027, -251, 581, 537,
];

const CTCSS_215HZ: [i16; 88] = [
    2_038, 2_049, 1_991, 1_859, 1_650, 1_363, 999, 562, 58, -502, -1_106, -1_739, -2_382, -3_014,
    -3_612, -4_153, -4_610, -4_959, -5_172, -5_226, -5_098, -4_769, -4_222, -3_444, -2_430, -1_176,
    310, 2_021, 3_937, 6_035, 8_284, 10_648, 13_086, 15_550, 17_993, 20_363, 22_608, 24_677,
    26_522, 28_099, 29_369, 30_299, 30_867, 31_058, 30_867, 30_299, 29_369, 28_099, 26_522, 24_677,
    22_608, 20_363, 17_993, 15_550, 13_086, 10_648, 8_284, 6_035, 3_937, 2_021, 310, -1_176,
    -2_430, -3_444, -4_222, -4_769, -5_098, -5_226, -5_172, -4_959, -4_610, -4_153, -3_612, -3_014,
    -2_382, -1_739, -1_106, -502, 58, 562, 999, 1_363, 1_650, 1_859, 1_991, 2_049, 2_038, 1_966,
];

const CTCSS_250HZ: [i16; 66] = [
    676, 364, -3, -415, -860, -1_320, -1_777, -2_209, -2_593, -2_904, -3_119, -3_212, -3_162,
    -2_949, -2_557, -1_975, -1_198, -226, 932, 2_263, 3_744, 5_346, 7_034, 8_767, 10_499, 12_184,
    13_770, 15_211, 16_462, 17_480, 18_234, 18_696, 18_852, 18_696, 18_234, 17_480, 16_462, 15_211,
    13_770, 12_184, 10_499, 8_767, 7_034, 5_346, 3_744, 2_263, 932, -226, -1_198, -1_975, -2_557,
    -2_949, -3_162, -3_212, -3_119, -2_904, -2_593, -2_209, -1_777, -1_320, -860, -415, -3, 364,
    676, 927,
];

pub(crate) const BASEBAND: FilterProfile = FilterProfile {
    coefficients: &BASEBAND_3KHZ,
    divisor: 131_072,
};

/// Resolve the documented discriminator-noise profile number.
pub(crate) fn noise(profile: u32) -> Option<FilterProfile> {
    match profile {
        0 => Some(FilterProfile {
            coefficients: &NOISE_STANDARD,
            divisor: 65_536,
        }),
        1 => Some(FilterProfile {
            coefficients: &NOISE_ALTERNATE,
            divisor: 65_536,
        }),
        _ => None,
    }
}

/// Select the wider detector LPF for an enabled tone at or above 203.5 Hz.
pub(crate) fn ctcss(tone_mask: u64) -> FilterProfile {
    if tone_mask & (u64::MAX << 31) == 0 {
        FilterProfile {
            coefficients: &CTCSS_215HZ,
            divisor: 524_288,
        }
    } else {
        FilterProfile {
            coefficients: &CTCSS_250HZ,
            divisor: 262_144,
        }
    }
}

#[cfg(test)]
#[cfg_attr(coverage, coverage(off))]
mod tests {
    use super::{BASEBAND, ctcss, noise};

    #[test]
    fn profiles_hide_fixed_tables_behind_stable_choices() {
        assert_eq!(BASEBAND.coefficients.len(), 66);
        assert_eq!(BASEBAND.divisor, 131_072);
        assert_eq!(noise(0).unwrap().coefficients.len(), 66);
        assert_eq!(noise(1).unwrap().coefficients.len(), 66);
        assert!(noise(2).is_none());
        assert_eq!(ctcss(1 << 30).coefficients.len(), 88);
        assert_eq!(ctcss(1 << 31).coefficients.len(), 66);
    }
}
