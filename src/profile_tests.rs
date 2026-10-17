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

#[test]
fn profile_rejects_each_unused_native_waveform_and_missing_dsp_setting() {
    for field in 0..6 {
        let mut c = source_config(2, 0);
        match field {
            0 => c.transmit.mapped_ctcss_frequency_tenths_hz[3] = 1000,
            1 => c.transmit.ctcss_peak = 0.1,
            2 => c.transmit.ctcss_turnoff_phase_shift_degrees = 120.0,
            3 => c.transmit.ctcss_turnoff_tail_tone_hz = 55.0,
            4 => c.transmit.dcs_transmit_enabled = 1,
            _ => c.transmit.ctcss_transmit_enabled = 1,
        }
        assert!(ValidatedProfile::new(&profile(2, 0), &capabilities(), &c).is_err());
    }
    let mut c = source_config(0, 1);
    c.receive.ctcss_tone_mask = 0;
    assert!(ValidatedProfile::new(&profile(0, 1), &capabilities(), &c).is_err());
    let mut c = source_config(1, 0);
    c.transmit.default_ctcss_frequency_tenths_hz = 0;
    assert!(ValidatedProfile::new(&profile(1, 0), &capabilities(), &c).is_err());
    let mut c = source_config(1, 0);
    c.transmit.output_a_route = 1;
    assert!(ValidatedProfile::new(&profile(1, 0), &capabilities(), &c).is_err());
    let mut c = config();
    c.transmit.dcs_transmit_enabled = 1;
    c.transmit.output_a_route = 2;
    assert!(ValidatedProfile::new(&profile(0, 0), &capabilities(), &c).is_ok());
}

#[test]
fn profile_dcs_encode_requires_a_native_signaling_route() {
    let mut c = config();
    c.transmit.dcs_transmit_enabled = 1;
    let mut caps = capabilities();
    assert!(ValidatedProfile::new(&profile(0, 0), &caps, &c).is_err());
    c.transmit.output_a_route = 2;
    caps.dsp_tone_audio = 0;
    assert!(ValidatedProfile::new(&profile(0, 0), &caps, &c).is_err());
}

#[test]
fn profiled_ffi_rejects_missing_short_and_invalid_records() {
    let c = config();
    let p = profile(0, 0);
    let caps = capabilities();
    let ports = SessionPorts::default();
    let runtime = Runtime::new(&p, &c);
    let short = 0_u32;
    let mut handle = ptr::null_mut();
    let mut update = ptr::null_mut();
    let mut output = [0.0; 2];
    let mut rx = ProfileReceiveResult {
        struct_size: size_of::<ProfileReceiveResult>() as u32,
        ..Default::default()
    };
    let mut tx = ProfileTransmitResult {
        struct_size: size_of::<ProfileTransmitResult>() as u32,
        ..Default::default()
    };
    let mut controls = ProfileReceiveInput {
        struct_size: size_of::<ProfileReceiveInput>() as u32,
        ..Default::default()
    };
    unsafe {
        assert_eq!(
            create_profiled(&c, &ports, &p, &caps, ptr::null_mut()),
            INVALID_ARGUMENT
        );
        for bad in [ptr::null(), ptr::from_ref(&short)] {
            assert_eq!(
                create_profiled(bad.cast(), &ports, &p, &caps, &mut handle),
                INVALID_ARGUMENT
            );
            assert_eq!(
                create_profiled(&c, &ports, bad.cast(), &caps, &mut handle),
                INVALID_ARGUMENT
            );
            assert_eq!(
                create_profiled(&c, &ports, &p, bad.cast(), &mut handle),
                INVALID_ARGUMENT
            );
            assert_eq!(
                receive_profiled(
                    runtime.0,
                    output.as_ptr(),
                    output.as_mut_ptr(),
                    1,
                    bad.cast(),
                    &mut rx
                ),
                INVALID_ARGUMENT
            );
            // Mutable short prefixes are supplied below; never cast shared storage writable.
        }
        let mut short_result = 0_u32;
        for bad in [ptr::null_mut(), ptr::from_mut(&mut short_result)] {
            assert_eq!(
                receive_profiled(
                    runtime.0,
                    output.as_ptr(),
                    output.as_mut_ptr(),
                    1,
                    &controls,
                    bad.cast()
                ),
                INVALID_ARGUMENT
            );
            assert_eq!(
                transmit_profiled(
                    runtime.0,
                    output.as_mut_ptr(),
                    1,
                    &TransmitInput::default(),
                    bad.cast()
                ),
                INVALID_ARGUMENT
            );
        }
        let mut bad = c;
        bad.native_sample_rate_hz = 8000;
        assert_ne!(create_profiled(&bad, &ports, &p, &caps, &mut handle), OK);
        assert_ne!(
            prepare_profiled_update(runtime.0, &bad, &ports, &p, &caps, &mut update),
            OK
        );
        assert_eq!(
            prepare_profiled_update(runtime.0, &c, &ports, &p, &caps, ptr::null_mut()),
            INVALID_ARGUMENT
        );
        assert_eq!(
            prepare_profiled_update(ptr::null(), &c, &ports, &p, &caps, &mut update),
            INVALID_ARGUMENT
        );
        assert_eq!(
            prepare_profiled_update(runtime.0, ptr::null(), &ports, &p, &caps, &mut update),
            INVALID_ARGUMENT
        );
        let mut invalid = p;
        invalid.kind = 2;
        assert_eq!(
            create_profiled(&c, &ports, &invalid, &caps, &mut handle),
            INVALID_ARGUMENT
        );
        assert_eq!(
            receive_profiled(
                ptr::null_mut(),
                output.as_ptr(),
                output.as_mut_ptr(),
                1,
                &controls,
                &mut rx
            ),
            INVALID_ARGUMENT
        );
        controls.external_ctcss_valid = 2;
        assert_eq!(
            receive_profiled(
                runtime.0,
                output.as_ptr(),
                output.as_mut_ptr(),
                1,
                &controls,
                &mut rx
            ),
            INVALID_ARGUMENT
        );
        controls.external_ctcss_valid = 0;
        assert_eq!(
            receive_profiled(
                runtime.0,
                ptr::null(),
                output.as_mut_ptr(),
                1,
                &controls,
                &mut rx
            ),
            INVALID_ARGUMENT
        );
        assert_eq!(
            transmit_profiled(
                ptr::null_mut(),
                output.as_mut_ptr(),
                1,
                &TransmitInput::default(),
                &mut tx
            ),
            INVALID_ARGUMENT
        );
        assert_eq!(
            transmit_profiled(
                runtime.0,
                ptr::null_mut(),
                1,
                &TransmitInput::default(),
                &mut tx
            ),
            INVALID_ARGUMENT
        );
        assert_eq!(
            transmit_profiled(runtime.0, output.as_mut_ptr(), 1, ptr::null(), &mut tx),
            INVALID_ARGUMENT
        );
        let mut raw_rx = ReceiveResult::default();
        let mut raw_tx = TransmitResult::default();
        assert_eq!(
            receive(
                runtime.0,
                output.as_ptr(),
                output.as_mut_ptr(),
                1,
                &ReceiveInput::default(),
                &mut raw_rx
            ),
            UNSUPPORTED
        );
        assert_eq!(
            transmit(
                runtime.0,
                output.as_mut_ptr(),
                1,
                &TransmitInput::default(),
                &mut raw_tx
            ),
            UNSUPPORTED
        );
        assert_eq!(create(&c, &ports, &mut handle), OK);
        assert_eq!(
            receive_profiled(
                handle,
                output.as_ptr(),
                output.as_mut_ptr(),
                1,
                &controls,
                &mut rx
            ),
            UNSUPPORTED
        );
        assert_eq!(
            transmit_profiled(
                handle,
                output.as_mut_ptr(),
                1,
                &TransmitInput::default(),
                &mut tx
            ),
            UNSUPPORTED
        );
        destroy(handle);
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
        config.transmit.output_a_tone_gain = 1.0;
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

impl Runtime {
    fn transmit(
        &self,
        keyed: bool,
        inhibit: bool,
        count: usize,
    ) -> (ProfileTransmitResult, Vec<f32>) {
        let mut pcm = vec![9.0; count * 2];
        let mut result = ProfileTransmitResult {
            struct_size: size_of::<ProfileTransmitResult>() as u32,
            ..Default::default()
        };
        let controls = TransmitInput {
            external_ptt_request: u32::from(keyed),
            physical_ptt_applied: 1,
            render_admitted: 1,
            ctcss_inhibit: u32::from(inhibit),
            ..Default::default()
        };
        assert_eq!(
            unsafe {
                transmit_profiled(
                    self.0,
                    pcm.as_mut_ptr(),
                    count as u32,
                    &controls,
                    &mut result,
                )
            },
            OK
        );
        (result, pcm)
    }
}

#[test]
fn profiled_external_encode_has_no_tone_pcm() {
    for encode in 0..=2 {
        let runtime = Runtime::new(&profile(encode, 0), &source_config(encode, 0));
        let (result, pcm) = runtime.transmit(true, false, 960);
        assert_eq!(result.outputs.logical_ptt, 1);
        assert_eq!(result.outputs.external_ctcss_enable, u32::from(encode == 2));
        assert_eq!(pcm.iter().any(|value| value.abs() > 0.01), encode == 1);
        assert!(pcm.chunks_exact(2).all(|frame| frame[1] == 0.0));
    }
}

#[test]
fn profiled_encode_follows_live_inhibit() {
    for encode in [1, 2] {
        let runtime = Runtime::new(&profile(encode, 0), &source_config(encode, 0));
        for inhibit in [false, true, false] {
            let (result, pcm) = runtime.transmit(true, inhibit, 960);
            assert_eq!(result.outputs.logical_ptt, 1);
            assert_eq!(
                result.outputs.external_ctcss_enable,
                u32::from(encode == 2 && !inhibit)
            );
            assert_eq!(
                pcm.iter().any(|value| value.abs() > 0.01),
                encode == 1 && !inhibit
            );
        }
    }
}

#[test]
fn profiled_encode_release_and_partitioning() {
    for mode in [0, 2] {
        let mut c = config();
        c.transmit.tone_off_mode = mode;
        c.transmit.ctcss_turnoff_duration_ms = 30;
        let mut results = Vec::new();
        for partition in [&[960][..], &[480][..], &[1, 103, 337, 519][..]] {
            let runtime = Runtime::new(&profile(2, 0), &c);
            runtime.transmit(true, false, 960);
            let mut elapsed = 0;
            let mut last = None;
            while elapsed < 9600 {
                for count in partition {
                    let count = (*count).min(9600 - elapsed);
                    if count == 0 {
                        break;
                    }
                    let (result, pcm) = runtime.transmit(false, false, count);
                    assert_eq!(result.outputs.external_ctcss_enable, 0);
                    assert!(pcm.iter().all(|value| *value == 0.0));
                    elapsed += count;
                    last = Some(result.outputs);
                }
            }
            results.push(last.unwrap());
        }
        assert_eq!(results[0], results[1]);
        assert_eq!(results[0], results[2]);
        assert_eq!(results[0].logical_ptt, 0);
    }
}

#[test]
fn profiled_transmit_errors_release_actions_without_overwrite() {
    let runtime = Runtime::new(&profile(2, 0), &config());
    runtime.transmit(true, false, 960);
    let mut output = [9.0; 2];
    let mut result = ProfileTransmitResult {
        struct_size: size_of::<ProfileTransmitResult>() as u32,
        outputs: PreparedOutputs {
            logical_ptt: 1,
            external_ctcss_enable: 1,
        },
        ..Default::default()
    };
    let controls = TransmitInput::default();
    assert_eq!(
        unsafe { transmit_profiled(runtime.0, output.as_mut_ptr(), 961, &controls, &mut result) },
        FRAME_COUNT_EXCEEDED
    );
    assert_eq!(output, [9.0; 2]);
    assert_eq!(result.outputs, PreparedOutputs::default());
    let controls = TransmitInput {
        forced_ctcss_tenths_hz: 1148,
        ..Default::default()
    };
    assert_eq!(
        unsafe { transmit_profiled(runtime.0, output.as_mut_ptr(), 1, &controls, &mut result) },
        UNSUPPORTED
    );
    assert_eq!(result.outputs, PreparedOutputs::default());
    assert_eq!(output, [0.0; 2]);
}

#[test]
fn profiled_transmit_unwarmed_and_provider_failure_are_safe() {
    unsafe extern "C" fn fail(_: *mut std::ffi::c_void, _: *const f32, _: *mut f32, _: u32) -> i32 {
        -1
    }
    let mut handle = ptr::null_mut();
    let mut ports = SessionPorts::default();
    assert_eq!(
        unsafe {
            create_profiled(
                &config(),
                &ports,
                &profile(2, 0),
                &capabilities(),
                &mut handle,
            )
        },
        OK
    );
    let runtime = Runtime(handle);
    let mut output = [9.0; 2];
    let mut result = ProfileTransmitResult {
        struct_size: size_of::<ProfileTransmitResult>() as u32,
        ..Default::default()
    };
    let controls = TransmitInput {
        external_ptt_request: 1,
        render_admitted: 1,
        ..Default::default()
    };
    assert_eq!(
        unsafe { transmit_profiled(runtime.0, output.as_mut_ptr(), 1, &controls, &mut result) },
        NOT_READY
    );
    assert_eq!(output, [0.0; 2]);
    assert_eq!(result.outputs, PreparedOutputs::default());
    assert_eq!(unsafe { warm(runtime.0) }, OK);
    ports.transmit_program.process_f32 = Some(fail);
    let mut update = ptr::null_mut();
    assert_eq!(
        unsafe {
            prepare_profiled_update(
                runtime.0,
                &config(),
                &ports,
                &profile(2, 0),
                &capabilities(),
                &mut update,
            )
        },
        OK
    );
    assert_eq!(unsafe { apply_transmit_update(runtime.0, update) }, OK);
    unsafe { destroy_update(update) };
    assert_eq!(
        unsafe { transmit_profiled(runtime.0, output.as_mut_ptr(), 1, &controls, &mut result) },
        PROVIDER_FAILED
    );
    assert_eq!(output, [0.0; 2]);
    assert_eq!(result.outputs, PreparedOutputs::default());
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
