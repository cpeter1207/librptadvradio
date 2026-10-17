use crate::{profile::*, session::*};
use std::{mem::size_of, ptr};

fn config() -> SessionConfig {
    SessionConfig {
        struct_size: size_of::<SessionConfig>() as u32,
        abi_version: ABI_VERSION,
        native_sample_rate_hz: 48_000,
        interleaved_channels: 2,
        maximum_receive_frame_count: 960,
        maximum_transmit_frame_count: 960,
        publication_interval_ms: 50,
        receive_input_gain: 1.0,
        receive: ReceiveConfig {
            ctcss_decoder_gain: 1.0,
            ..Default::default()
        },
        qualification: QualificationConfig {
            radio_duplex: 1,
            ..Default::default()
        },
        ..Default::default()
    }
}

fn profile(encode: u32, decode: u32) -> Profile {
    Profile {
        struct_size: size_of::<Profile>() as u32,
        kind: 1,
        carrier_source: 0,
        ctcss_encode_source: encode,
        ctcss_decode_source: decode,
    }
}

fn capabilities() -> ProfileCapabilities {
    ProfileCapabilities {
        struct_size: size_of::<ProfileCapabilities>() as u32,
        hardware_cor: 1,
        external_ctcss_decode: 1,
        external_ctcss_encode: 1,
        dsp_tone_audio: 1,
    }
}

fn source_config(encode: u32, decode: u32) -> SessionConfig {
    let mut config = config();
    if encode == 1 {
        config.transmit.ctcss_transmit_enabled = 1;
        config.transmit.default_ctcss_frequency_tenths_hz = 1148;
        config.transmit.ctcss_peak = 0.1;
        config.transmit.output_a_route = 2;
    }
    if decode == 1 {
        config.receive.ctcss_enabled = 1;
        config.receive.ctcss_tone_mask = 1;
    }
    config
}

#[test]
fn profile_independent_source_matrix() {
    for encode in 0..=2 {
        for decode in 0..=2 {
            let p = profile(encode, decode);
            let c = source_config(encode, decode);
            let mut caps = capabilities();
            assert!(ValidatedProfile::new(&p, &caps, &c).is_ok());
            for missing in 0..4 {
                caps = capabilities();
                match missing {
                    0 => caps.hardware_cor = 0,
                    1 => caps.external_ctcss_decode = 0,
                    2 => caps.external_ctcss_encode = 0,
                    _ => caps.dsp_tone_audio = 0,
                }
                let required = missing == 0
                    || (missing == 1 && decode == 2)
                    || (missing == 2 && encode == 2)
                    || (missing == 3 && encode == 1);
                assert_eq!(ValidatedProfile::new(&p, &caps, &c).is_err(), required);
            }
        }
    }
}

#[test]
fn profile_rejects_incompatible_native_ctcss_and_dcs() {
    let mut c = config();
    c.transmit.default_ctcss_frequency_tenths_hz = 1148;
    assert!(ValidatedProfile::new(&profile(2, 0), &capabilities(), &c).is_err());
    c = config();
    c.receive.dcs_enabled = 1;
    c.receive.dcs_code = 23;
    assert!(ValidatedProfile::new(&profile(0, 2), &capabilities(), &c).is_err());
    assert!(ValidatedProfile::new(&profile(0, 0), &capabilities(), &c).is_ok());
    for mode in [1, 3] {
        c = config();
        c.transmit.tone_off_mode = mode;
        assert!(ValidatedProfile::new(&profile(2, 0), &capabilities(), &c).is_err());
    }
}

struct Runtime(*mut Session);
impl Runtime {
    fn new(p: &Profile, config: &SessionConfig) -> Self {
        let mut handle = ptr::null_mut();
        assert_eq!(
            unsafe {
                create_profiled(
                    config,
                    &SessionPorts::default(),
                    p,
                    &capabilities(),
                    &mut handle,
                )
            },
            OK
        );
        assert_eq!(unsafe { warm(handle) }, OK);
        Self(handle)
    }

    fn receive(&self, cor: u32, tone: u32, valid: u32, count: usize) -> ProfileReceiveResult {
        let input = vec![0.1; count * 2];
        let mut output = vec![9.0; count];
        let controls = ProfileReceiveInput {
            struct_size: size_of::<ProfileReceiveInput>() as u32,
            controls: ReceiveInput::default(),
            hardware_cor: cor,
            hardware_cor_valid: valid,
            external_ctcss: tone,
            external_ctcss_valid: valid,
        };
        let mut result = ProfileReceiveResult {
            struct_size: size_of::<ProfileReceiveResult>() as u32,
            ..Default::default()
        };
        assert_eq!(
            unsafe {
                receive_profiled(
                    self.0,
                    input.as_ptr(),
                    output.as_mut_ptr(),
                    count as u32,
                    &controls,
                    &mut result,
                )
            },
            OK
        );
        assert_eq!(result.result.frame_count as usize, count);
        result
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        unsafe { destroy(self.0) };
    }
}

#[test]
fn profiled_external_receive_qualification() {
    let runtime = Runtime::new(&profile(0, 2), &config());
    for count in [1, 17, 480, 960] {
        assert_eq!(runtime.receive(1, 1, 1, count).result.receiver_keyed, 1);
        assert_eq!(runtime.receive(1, 0, 1, count).result.receiver_keyed, 0);
        assert_eq!(runtime.receive(0, 1, 1, count).result.receiver_keyed, 0);
    }
}

#[test]
fn profiled_disabled_decode_keeps_carrier_gate() {
    let runtime = Runtime::new(&profile(0, 0), &config());
    assert_eq!(runtime.receive(1, 0, 1, 17).result.receiver_keyed, 1);
    let observation = runtime.receive(0, 1, 1, 17);
    assert_eq!(observation.result.receiver_keyed, 0);
    assert_eq!(observation.external_ctcss, 1);
    assert_eq!(observation.external_ctcss_valid, 1);
}

#[test]
fn profiled_invalid_snapshot_is_unqualified() {
    let runtime = Runtime::new(&profile(0, 2), &config());
    assert_eq!(runtime.receive(1, 1, 1, 17).result.receiver_keyed, 1);
    assert_eq!(runtime.receive(1, 1, 0, 17).result.receiver_keyed, 0);
}

#[test]
fn profile_validates_sizes_flags_and_dsp_requirements() {
    let good = profile(0, 0);
    for p in [
        Profile {
            struct_size: 0,
            ..good
        },
        Profile { kind: 2, ..good },
        Profile {
            carrier_source: 2,
            ..good
        },
        Profile {
            ctcss_encode_source: 3,
            ..good
        },
        Profile {
            ctcss_decode_source: 3,
            ..good
        },
    ] {
        assert!(ValidatedProfile::new(&p, &capabilities(), &config()).is_err());
    }
    let good_caps = capabilities();
    for caps in [
        ProfileCapabilities {
            struct_size: 0,
            ..good_caps
        },
        ProfileCapabilities {
            hardware_cor: 2,
            ..good_caps
        },
        ProfileCapabilities {
            external_ctcss_decode: 2,
            ..good_caps
        },
        ProfileCapabilities {
            external_ctcss_encode: 2,
            ..good_caps
        },
        ProfileCapabilities {
            dsp_tone_audio: 2,
            ..good_caps
        },
    ] {
        assert!(ValidatedProfile::new(&good, &caps, &config()).is_err());
    }
    assert!(ValidatedProfile::new(&profile(1, 0), &good_caps, &config()).is_err());
    assert!(ValidatedProfile::new(&profile(0, 1), &good_caps, &config()).is_err());
    let mut c = source_config(1, 1);
    c.transmit.output_a_route = 1;
    assert!(ValidatedProfile::new(&profile(1, 1), &good_caps, &c).is_err());
    c = source_config(1, 1);
    c.receive.ctcss_tone_mask = 0;
    assert!(ValidatedProfile::new(&profile(1, 1), &good_caps, &c).is_err());
}

#[test]
fn profiled_dsp_and_dcs_receive() {
    let mut p = profile(0, 1);
    p.carrier_source = 1;
    let runtime = Runtime::new(&p, &source_config(0, 1));
    // DSP decode, not the asserted external decoder, qualifies this profile.
    assert_eq!(runtime.receive(1, 1, 1, 960).result.receiver_keyed, 0);
    let mut c = config();
    c.receive.dcs_enabled = 1;
    c.receive.dcs_code = 23;
    let runtime = Runtime::new(&profile(0, 0), &c);
    assert_eq!(runtime.receive(1, 1, 1, 960).result.receiver_keyed, 0);
}

#[test]
fn profiled_update_rejects_source_change() {
    let p = profile(0, 2);
    let runtime = Runtime::new(&p, &config());
    let mut update = ptr::null_mut();
    assert_eq!(
        unsafe {
            prepare_profiled_update(
                runtime.0,
                &config(),
                &SessionPorts::default(),
                &profile(0, 0),
                &capabilities(),
                &mut update,
            )
        },
        UNSUPPORTED
    );
    assert!(update.is_null());
    assert_eq!(runtime.receive(1, 0, 1, 17).result.receiver_keyed, 0);
    assert_eq!(
        unsafe {
            prepare_profiled_update(
                runtime.0,
                &config(),
                &SessionPorts::default(),
                &p,
                &capabilities(),
                &mut update,
            )
        },
        OK
    );
    assert_eq!(unsafe { apply_receive_update(runtime.0, update) }, OK);
    assert_eq!(unsafe { apply_transmit_update(runtime.0, update) }, OK);
    unsafe { destroy_update(update) };
    // The unspecialized update cannot overwrite an immutable profiled session.
    assert_eq!(
        unsafe { prepare_update(&config(), &SessionPorts::default(), &mut update) },
        OK
    );
    assert_eq!(
        unsafe { apply_receive_update(runtime.0, update) },
        UNSUPPORTED
    );
    assert_eq!(
        unsafe { apply_transmit_update(runtime.0, update) },
        UNSUPPORTED
    );
    unsafe { destroy_update(update) };
}
