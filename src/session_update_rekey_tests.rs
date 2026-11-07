use super::*;

struct TestUpdate(*mut SessionUpdate);

impl Drop for TestUpdate {
    fn drop(&mut self) {
        // SAFETY: this test exclusively owns the prepared update until its single destruction.
        unsafe { ((*crate::rptadv_radio_descriptor()).session_destroy_update)(self.0) };
    }
}

#[test]
fn public_session_update_preserves_checked_dcs_rekey_rejection() {
    for (duration, expected) in [(0, INVALID_ARGUMENT), (20, OK)] {
        let mut config = valid_config();
        config.transmit.dcs_transmit_enabled = 1;
        config.transmit.dcs_turnoff_enabled = 1;
        config.transmit.dcs_turnoff_duration_ms = 20;
        config.transmit.dcs_code = 0o023;
        config.transmit.dcs_peak = 0.1;
        config.transmit.output_a_route = 2;
        config.transmit.output_b_route = 2;
        config.transmit.output_a_tone_gain = 1.0;
        config.transmit.output_b_tone_gain = 1.0;
        let ports = SessionPorts::default();
        let keyed = TransmitInput {
            external_ptt_request: 1,
            physical_ptt_applied: 1,
            render_admitted: 1,
            ..TransmitInput::default()
        };
        let released = TransmitInput {
            external_ptt_request: 0,
            ..keyed
        };
        let mut raw = ptr::null_mut();
        let mut output = [9.0; 8];
        let mut result = TransmitResult::default();

        // SAFETY: the immutable exported table has process lifetime; every sized input,
        // output, session, and prepared update remains live throughout its serial ABI calls.
        unsafe {
            let api = &*crate::rptadv_radio_descriptor();
            assert_eq!((api.session_create)(&config, &ports, &mut raw), OK);
            let session = TestSession(raw);
            assert_eq!((api.session_warm)(session.0), OK);
            assert_eq!(
                (api.session_transmit)(session.0, output.as_mut_ptr(), 4, &keyed, &mut result),
                OK
            );
            assert_eq!(result.transmitter_state, transmit_signaling::STATE_ACTIVE);
            assert!(output.iter().any(|sample| sample.abs() > 0.0));
            assert_eq!(
                (api.session_transmit)(session.0, output.as_mut_ptr(), 4, &released, &mut result),
                OK
            );
            assert_eq!(result.transmitter_state, transmit_signaling::STATE_TOC);
            assert_eq!(result.logical_ptt, 1);
            assert!(output.iter().any(|sample| sample.abs() > 0.0));
            let before = session.snapshot();

            // Both candidates are valid disabled policies. Their code and polarity stay
            // unchanged; only the zero duration makes the retained tail's rekey reject.
            config.transmit.dcs_transmit_enabled = 0;
            config.transmit.dcs_turnoff_enabled = 0;
            config.transmit.dcs_turnoff_duration_ms = duration;
            let mut update = ptr::null_mut();
            assert_eq!(
                (api.session_prepare_update)(&config, &ports, &mut update),
                OK
            );
            let update = TestUpdate(update);
            assert_eq!((api.session_apply_transmit_update)(session.0, update.0), OK);
            drop(update);

            output.fill(9.0);
            assert_eq!(
                (api.session_transmit)(session.0, output.as_mut_ptr(), 4, &keyed, &mut result),
                expected
            );
            let after = session.snapshot();
            assert_eq!(after.provider_failures, before.provider_failures);
            if expected == INVALID_ARGUMENT {
                assert_eq!(output, [0.0; 8]);
                assert_eq!(after.transmit_frames, before.transmit_frames);
            } else {
                assert_eq!(result.transmitter_state, transmit_signaling::STATE_ACTIVE);
                assert_eq!(result.logical_ptt, 1);
                assert_eq!(after.transmit_frames, before.transmit_frames + 4);
            }
        }
    }
}
