//! Sample-clocked MICOR-style discriminator-noise squelch.
//!
//! The state and arithmetic retain the established USBRadioPlus comparator
//! behavior while keeping it portable, allocation-free, and independent of
//! Asterisk or a hardware adapter.  Callers invoke [`update`] once per native
//! discriminator sample; callback partitioning therefore cannot alter timing.

/// Native samples used to establish the initial noise-detector reference.
pub const SETTLE_SAMPLES: u32 = 480;

const DETECTOR_ALPHA: f64 = 0.004_157_998_154_890_04;
const DEFEAT_ALPHA: f64 = 0.020_617_818_668_759_89;
const CHARGE_ALPHA: f64 = 0.001_387_924_829_091_71;
const RELEASE_ALPHA: f64 = 0.000_051_352_434_961_13;
const IDLE_ALPHA: f64 = 0.000_032_551_553_520_44;
const HOLD_THRESHOLD: f64 = 3.8 / 5.5;

/// Caller-owned persistent state for one MICOR comparator.
///
/// This exact C layout is intentionally public through the radio-core ABI so
/// a compatibility adapter can retain state alongside its existing receiver
/// stage.  It contains no pointers and never owns storage.
#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct State {
    /// Smoothed discriminator-noise power in established squared units.
    pub noise_power: f64,
    /// Slowly tracked no-carrier reference used by the strong-signal defeat.
    pub idle_power: f64,
    /// Long-tail capacitor charge normalized to a full charge of one.
    pub hold_charge: f64,
    /// Native samples consumed while establishing the initial detector state.
    pub settling_samples: u32,
}

/// Advance one compatibility MICOR noise-squelch sample.
///
/// `squelched` is the previous comparator result. `sample_power`,
/// `open_level`, and `hysteresis` retain their historical USBRadioPlus units.
/// The function allocates nothing, performs no I/O, and is safe to call from
/// the bounded native audio path.
pub fn update(
    state: &mut State,
    squelched: bool,
    sample_power: f64,
    open_level: u32,
    hysteresis: u32,
) -> bool {
    let open_power = f64::from(open_level) * f64::from(open_level);
    let limit = f64::from(open_level)
        + if squelched {
            0.0
        } else {
            f64::from(hysteresis)
        };
    let limit_power = limit * limit;

    // The startup average prevents zero-filled FIR history from qualifying as
    // a signal. It is intentionally sample-counted rather than block-counted.
    if state.settling_samples < SETTLE_SAMPLES {
        state.settling_samples += 1;
        state.noise_power += (sample_power - state.noise_power) / f64::from(state.settling_samples);
        state.idle_power = state.noise_power;
        return true;
    }

    state.noise_power += DETECTOR_ALPHA * (sample_power - state.noise_power);
    if squelched && state.noise_power >= open_power {
        state.idle_power += IDLE_ALPHA * (state.noise_power - state.idle_power);
    }
    let strong_power = state.idle_power * 0.01;
    let direct_power = if strong_power < limit_power {
        limit_power + (strong_power - limit_power) * (5.0 / 11.0)
    } else {
        limit_power
    };

    if state.noise_power <= strong_power {
        state.hold_charge -= DEFEAT_ALPHA * state.hold_charge;
    } else if state.noise_power < limit_power {
        state.hold_charge += CHARGE_ALPHA * (1.0 - state.hold_charge);
    } else {
        state.hold_charge -= RELEASE_ALPHA * state.hold_charge;
    }

    // Do not let inaudible residual state enter slow subnormal arithmetic
    // during long quiet periods. This mirrors the legacy compatibility path.
    if state.hold_charge < 1.0e-12 {
        state.hold_charge = 0.0;
    }
    if state.noise_power < 1.0e-12 {
        state.noise_power = 0.0;
    }
    !(state.noise_power < direct_power || state.hold_charge > HOLD_THRESHOLD)
}

#[cfg(test)]
mod tests {
    use super::{SETTLE_SAMPLES, State, update};

    fn feed(
        state: &mut State,
        closed: &mut bool,
        level: f64,
        samples: u32,
        open_level: u32,
        hysteresis: u32,
    ) {
        for _ in 0..samples {
            *closed = update(state, *closed, level * level, open_level, hysteresis);
        }
    }

    #[test]
    fn startup_stays_closed_for_the_exact_settle_span() {
        let mut state = State::default();
        let mut closed = true;

        feed(
            &mut state,
            &mut closed,
            10_000.0,
            SETTLE_SAMPLES,
            7_000,
            500,
        );
        assert!(closed);
        assert_eq!(state.settling_samples, SETTLE_SAMPLES);
    }

    #[test]
    fn strong_loss_closes_without_a_block_boundary() {
        let mut state = State::default();
        let mut closed = true;

        feed(&mut state, &mut closed, 10_000.0, 48_000, 7_000, 500);
        feed(&mut state, &mut closed, 900.0, 24_000, 7_000, 500);
        assert!(!closed);
        let mut elapsed = 0_u32;
        while !closed && elapsed < 480 {
            feed(&mut state, &mut closed, 10_000.0, 1, 7_000, 500);
            elapsed += 1;
        }
        assert!(closed);
        assert_eq!(elapsed, 88);
    }

    #[test]
    fn quiet_state_is_clamped_to_exact_zero() {
        let mut state = State {
            noise_power: 1.0e-13,
            idle_power: 0.0,
            hold_charge: 1.0e-13,
            settling_samples: SETTLE_SAMPLES,
        };

        assert!(update(&mut state, true, 0.0, 0, 0));
        assert_eq!(state.noise_power, 0.0);
        assert_eq!(state.hold_charge, 0.0);
    }
}
