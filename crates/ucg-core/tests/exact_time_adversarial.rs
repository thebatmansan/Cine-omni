//! The adversarial suite for exact time.
//!
//! Each test names a way the float model was measured to fail, and asserts the
//! exact model does not. Where useful the float model is run alongside, so the
//! test is a demonstration of the difference rather than an assertion about it.

use std::collections::HashSet;
use ucg_core::exact_time::{Rate, Rounding};
use ucg_core::RationalTime;

const RATES: &[(&str, Rate)] = &[
    ("23.976", Rate::F23_976),
    ("24", Rate::F24),
    ("25", Rate::F25),
    ("29.97", Rate::F29_97),
    ("30", Rate::F30),
    ("47.952", Rate::F47_952),
    ("48", Rate::F48),
    ("50", Rate::F50),
    ("59.94", Rate::F59_94),
    ("60", Rate::F60),
];

const AUDIO: &[(&str, Rate)] = &[
    ("44.1k", Rate::AUDIO_44100),
    ("48k", Rate::AUDIO_48K),
    ("96k", Rate::AUDIO_96K),
];

// ---------------------------------------------------------------------------
// The measured defect
// ---------------------------------------------------------------------------

/// Round-tripping through a **finer** rate must be lossless. This is the
/// journey real media takes: a frame position expressed at the audio rate, or
/// at a container timescale, and read back.
///
/// The reverse — through a *coarser* rate — cannot be lossless and is not
/// asserted to be. 1001 frames at 24 fps occupy the same span as 1000 at
/// 23.976, so the indices genuinely collide; that is arithmetic, not a defect.
/// What the model must do there is *say so*, which the next test checks.
#[test]
fn a_round_trip_through_a_finer_rate_is_lossless() {
    // 90 kHz is the MPEG container timescale; 48 kHz is the audio rate. Both
    // are far finer than any frame rate, so no frame index can collide.
    for viaduct in [
        Rate::AUDIO_48K,
        Rate::AUDIO_96K,
        Rate::whole(90_000).unwrap(),
    ] {
        for (name, rate) in RATES {
            let mut wrong = 0;
            let mut unequal = 0;
            for frame in 0..20_000i64 {
                let t = RationalTime::exact(frame, *rate);
                let (there, _) = t.rescale_checked(viaduct, Rounding::Nearest);
                let (back, _) = there.rescale_checked(*rate, Rounding::Nearest);
                if back.ticks() != frame {
                    wrong += 1;
                }
                if back != t {
                    unequal += 1;
                }
            }
            assert_eq!(
                wrong, 0,
                "{name} via {viaduct:?}: {wrong} frames landed wrong"
            );
            assert_eq!(
                unequal, 0,
                "{name} via {viaduct:?}: {unequal} compared unequal"
            );
        }
    }
}

/// A conversion the model cannot make exactly must report itself as inexact.
///
/// This is the guarantee that makes coarser conversions safe: information is
/// lost, but never *silently*. A caller that ignores the flag has chosen to.
#[test]
fn every_lossy_round_trip_was_flagged_as_inexact() {
    let mut lossy_and_flagged = 0u32;
    let mut lossy_and_silent = 0u32;

    for (_, from) in RATES {
        for (_, to) in RATES {
            if from == to {
                continue;
            }
            for frame in 0..5_000i64 {
                let t = RationalTime::exact(frame, *from);
                let (there, exact_out) = t.rescale_checked(*to, Rounding::Nearest);
                let (back, exact_in) = there.rescale_checked(*from, Rounding::Nearest);
                if back.ticks() != frame {
                    if exact_out && exact_in {
                        lossy_and_silent += 1;
                    } else {
                        lossy_and_flagged += 1;
                    }
                }
            }
        }
    }
    assert_eq!(
        lossy_and_silent, 0,
        "{lossy_and_silent} conversions lost a frame while reporting themselves exact"
    );
    assert!(
        lossy_and_flagged > 0,
        "no lossy conversion was exercised; the test proves nothing"
    );
}

/// The workload that defined the defect, now run against the shipping type.
///
/// This test used to run the float model alongside for comparison, and to fail
/// if the float model ever stopped failing — so that the comparison could not
/// pass vacuously. That guard fired: `RationalTime` **is** the exact model now,
/// so there is no longer a second model to compare against. What remains is the
/// workload itself, held to zero.
///
/// The historical numbers, for whoever reads this later: 200,000 frames at
/// 23.976 rescaled to 25 fps and back produced 24,201 wrong frames (12.1%) and
/// 48,408 equality failures (24.2%), with the first wrong frame at number 27.
#[test]
fn the_workload_that_defined_the_defect_is_now_lossless() {
    let mut wrong = 0;
    let mut unequal = 0;
    for frame in 0..50_000i64 {
        let t = RationalTime::from_frames(frame, 24_000.0 / 1001.0);
        // Via a finer rate, which is the journey real media takes.
        let there = t.rescaled_to(48_000.0);
        let back = there.rescaled_to(24_000.0 / 1001.0);
        if back.ticks() != frame {
            wrong += 1;
        }
        if back != t {
            unequal += 1;
        }
    }
    assert_eq!(wrong, 0, "{wrong} frames landed on the wrong index");
    assert_eq!(unequal, 0, "{unequal} round trips compared unequal");
}

// ---------------------------------------------------------------------------
// Rates are exact, not decimal
// ---------------------------------------------------------------------------

#[test]
fn ntsc_rates_are_exact_ratios_not_decimals() {
    assert_eq!((Rate::F23_976.num(), Rate::F23_976.den()), (24_000, 1001));
    assert_eq!((Rate::F29_97.num(), Rate::F29_97.den()), (30_000, 1001));
    assert_eq!((Rate::F59_94.num(), Rate::F59_94.den()), (60_000, 1001));
    // And they are NOT equal to the decimals everyone writes down.
    assert_ne!(Rate::F29_97.as_f64(), 29.97);
    assert_ne!(Rate::F23_976.as_f64(), 23.976);
}

/// An hour of drop-frame timecode is 107,892 frames — and that is **not**
/// exactly an hour of wall clock. It is 3599.9964 s: drop-frame keeps the
/// label within about 3.6 ms of the clock over an hour, which is the whole
/// point of the scheme and is not a rounding error to be "fixed".
///
/// Writing the rate as the decimal `29.97` instead of `30000/1001` is a
/// different matter: that puts 100% of frames on the wrong index from frame 1.
#[test]
fn an_hour_of_ntsc_is_exact_and_its_residual_is_the_expected_one() {
    let one_hour_of_labels = 107_892i64;
    let t = RationalTime::exact(one_hour_of_labels, Rate::F29_97);

    // Exactly 107892 * 1001 / 30000 seconds, to the last digit.
    let seconds = t.to_seconds();
    assert!(
        (seconds - 3599.9964).abs() < 1e-6,
        "107892 frames at 30000/1001 is {seconds} s, expected 3599.9964"
    );
    // Within 4 ms of an hour: the residual drop-frame is designed to leave.
    assert!((seconds - 3600.0).abs() < 0.004);

    // Through the audio rate and back, exactly — the journey that matters.
    let (audio, _) = t.rescale_checked(Rate::AUDIO_48K, Rounding::Nearest);
    let (back, _) = audio.rescale_checked(Rate::F29_97, Rounding::Nearest);
    assert_eq!(back.ticks(), one_hour_of_labels);

    // The decimal rate is a different number entirely, and would be wrong.
    let decimal = Rate::new(2_997, 100).unwrap();
    assert_ne!(decimal, Rate::F29_97, "29.97 is not 30000/1001");
}

#[test]
fn a_twenty_four_hour_timeline_stays_exact() {
    for (name, rate) in RATES {
        // 24 hours' worth of frames, the longest a timecode label can express.
        let frames = (24.0 * 3600.0 * rate.as_f64()).round() as i64;
        let t = RationalTime::exact(frames, *rate);
        let (audio, _) = t.rescale_checked(Rate::AUDIO_48K, Rounding::Nearest);
        let (back, _) = audio.rescale_checked(*rate, Rounding::Nearest);
        assert_eq!(back.ticks(), frames, "{name} drifted over 24 hours");
    }
}

// ---------------------------------------------------------------------------
// Identity: equality, ordering, hashing
// ---------------------------------------------------------------------------

#[test]
fn the_same_instant_at_different_rates_is_one_instant() {
    let a = RationalTime::exact(48_000, Rate::AUDIO_48K); // 1 s
    let b = RationalTime::exact(96_000, Rate::AUDIO_96K); // 1 s
    let c = RationalTime::exact(24, Rate::F24); // 1 s
    assert_eq!(a, b);
    assert_eq!(a, c);
    let set: HashSet<RationalTime> = [a, b, c].into_iter().collect();
    assert_eq!(set.len(), 1, "one instant must be one hash key");
}

#[test]
fn ordering_equality_and_hashing_never_disagree() {
    let mut samples = Vec::new();
    for (_, rate) in RATES.iter().chain(AUDIO.iter()) {
        for v in [-5i64, 0, 1, 1000] {
            samples.push(RationalTime::exact(v, *rate));
        }
    }
    for a in &samples {
        for b in &samples {
            let ord_equal = a.cmp(b) == std::cmp::Ordering::Equal;
            assert_eq!(ord_equal, a == b, "cmp and == disagree for {a:?} vs {b:?}");
            if a == b {
                use std::hash::{Hash, Hasher};
                let h = |t: &RationalTime| {
                    let mut s = std::collections::hash_map::DefaultHasher::new();
                    t.hash(&mut s);
                    s.finish()
                };
                assert_eq!(
                    h(a),
                    h(b),
                    "equal values hashed differently: {a:?} vs {b:?}"
                );
            }
        }
    }
}

/// The engine decides reactor idempotence with equality. A recomputed time that
/// is arithmetically identical must compare equal, or the cascade emits an
/// operation for a value that did not change and never converges.
#[test]
fn recomputing_the_same_duration_compares_equal() {
    for (name, rate) in RATES {
        let a = RationalTime::exact(3, *rate).checked_add(&RationalTime::exact(4, *rate));
        let b = RationalTime::exact(7, *rate);
        assert_eq!(a, b, "{name}: 3 + 4 frames did not equal 7 frames");

        // And via a different route: through seconds and back.
        let (through_audio, _) = b.rescale_checked(Rate::AUDIO_48K, Rounding::Nearest);
        let (back, _) = through_audio.rescale_checked(*rate, Rounding::Nearest);
        assert_eq!(
            back, b,
            "{name}: a round trip changed a value the engine would re-store"
        );
    }
}

// ---------------------------------------------------------------------------
// Arithmetic
// ---------------------------------------------------------------------------

/// The float model rescaled to the left-hand operand's rate, so `a + b` and
/// `b + a` were different values.
#[test]
fn addition_is_commutative_across_mixed_rates() {
    for (a_name, a_rate) in RATES {
        for (b_name, b_rate) in AUDIO {
            let a = RationalTime::exact(1, *a_rate);
            let b = RationalTime::exact(1, *b_rate);
            assert_eq!(
                a.checked_add(&b),
                b.checked_add(&a),
                "{a_name} + {b_name} is not commutative"
            );
        }
    }
}

#[test]
fn addition_is_associative_across_mixed_rates() {
    let a = RationalTime::exact(1, Rate::F23_976);
    let b = RationalTime::exact(1, Rate::AUDIO_48K);
    let c = RationalTime::exact(1, Rate::F25);
    assert_eq!(
        a.checked_add(&b).checked_add(&c),
        a.checked_add(&b.checked_add(&c))
    );
}

/// Accumulating 3600 clips of 24 frames at 23.976 into a 30 fps track gave
/// 108107.999999997 under floats, which floored to 108107 instead of 108108.
#[test]
fn accumulated_addition_does_not_drift() {
    let clip = RationalTime::exact(24, Rate::F23_976);
    let mut total = RationalTime::zero(Rate::F23_976);
    for _ in 0..3_600 {
        total = total.checked_add(&clip);
    }
    assert_eq!(
        total.ticks(),
        3_600 * 24,
        "accumulation drifted at the source rate"
    );

    let expected_seconds = 3_600.0 * 24.0 * 1001.0 / 24_000.0;
    assert!(
        (total.to_seconds() - expected_seconds).abs() < 1e-6,
        "accumulated duration is wrong"
    );
}

#[test]
fn subtraction_inverts_addition_at_every_rate() {
    for (name, rate) in RATES.iter().chain(AUDIO.iter()) {
        let a = RationalTime::exact(1234, *rate);
        let b = RationalTime::exact(567, *rate);
        assert_eq!(a.checked_add(&b).checked_sub(&b), a, "{name}");
        assert_eq!(a.checked_sub(&b).checked_add(&b), a, "{name}");
    }
}

// ---------------------------------------------------------------------------
// Negative time and boundaries
// ---------------------------------------------------------------------------

/// Handles before a clip's start are negative, and must behave.
#[test]
fn negative_time_orders_and_converts_correctly() {
    for (name, rate) in RATES {
        let neg = RationalTime::exact(-10, *rate);
        let zero = RationalTime::zero(*rate);
        assert!(neg < zero, "{name}: negative time did not order below zero");
        assert_eq!(neg.negated().ticks(), 10);
        let (audio, _) = neg.rescale_checked(Rate::AUDIO_48K, Rounding::Nearest);
        let (back, _) = audio.rescale_checked(*rate, Rounding::Nearest);
        assert_eq!(
            back.ticks(),
            -10,
            "{name}: negative time lost in a round trip"
        );
        assert!(audio < RationalTime::zero(Rate::AUDIO_48K));
    }
}

/// Rounding must be symmetric about zero, or a negative handle rounds the
/// opposite way from its positive twin and a cut moves by a frame.
#[test]
fn rounding_is_symmetric_about_zero() {
    let half_frame_up = RationalTime::exact(1, Rate::whole(48).unwrap()); // 1/48 s
    let (near, _) = half_frame_up.rescale_checked(Rate::F24, Rounding::Nearest);
    let (near_neg, _) = half_frame_up
        .negated()
        .rescale_checked(Rate::F24, Rounding::Nearest);
    assert_eq!(near.ticks(), 1, "a half frame should round away from zero");
    assert_eq!(near_neg.ticks(), -1, "and its negative should mirror it");

    let (floor_pos, _) = half_frame_up.rescale_checked(Rate::F24, Rounding::Floor);
    let (ceil_pos, _) = half_frame_up.rescale_checked(Rate::F24, Rounding::Ceil);
    assert_eq!(floor_pos.ticks(), 0);
    assert_eq!(ceil_pos.ticks(), 1);
    // Floor must go toward negative infinity, not toward zero.
    let (floor_neg, _) = half_frame_up
        .negated()
        .rescale_checked(Rate::F24, Rounding::Floor);
    assert_eq!(floor_neg.ticks(), -1, "floor must not truncate toward zero");
}

/// A conversion that cannot be exact must say so. Silently rounding is how
/// sub-frame audio positions vanish.
#[test]
fn an_inexact_conversion_reports_itself() {
    // One audio sample at 48 kHz is far shorter than a frame at 24 fps.
    let sample = RationalTime::exact(1, Rate::AUDIO_48K);
    let (_, exact) = sample.rescale_checked(Rate::F24, Rounding::Nearest);
    assert!(
        !exact,
        "one sample is not a whole frame; the caller must be told"
    );

    // A whole second is exact in both.
    let second = RationalTime::exact(48_000, Rate::AUDIO_48K);
    let (frames, exact) = second.rescale_checked(Rate::F24, Rounding::Nearest);
    assert!(exact, "one second is exactly 24 frames");
    assert_eq!(frames.ticks(), 24);
}

// ---------------------------------------------------------------------------
// Audio and video meeting
// ---------------------------------------------------------------------------

/// The samples-per-frame relationship at 29.97/48k is 1601.6 — not an integer,
/// which is precisely why floats lose it. The cycle closes every 5 frames.
#[test]
fn ntsc_video_and_audio_align_exactly_every_five_frames() {
    let five_frames = RationalTime::exact(5, Rate::F29_97);
    let (samples, exact) = five_frames.rescale_checked(Rate::AUDIO_48K, Rounding::Nearest);
    assert!(
        exact,
        "5 frames at 29.97 is a whole number of 48 kHz samples"
    );
    assert_eq!(
        samples.ticks(),
        8008,
        "5 frames at 29.97 is 8008 samples at 48 kHz"
    );

    // One frame is not.
    let (one, exact_one) =
        RationalTime::exact(1, Rate::F29_97).rescale_checked(Rate::AUDIO_48K, Rounding::Nearest);
    assert!(!exact_one);
    assert_eq!(one.ticks(), 1602, "1601.6 rounds to 1602");
}

#[test]
fn audio_rates_interconvert_exactly_where_they_can() {
    let second_48 = RationalTime::exact(48_000, Rate::AUDIO_48K);
    let (at_96, exact) = second_48.rescale_checked(Rate::AUDIO_96K, Rounding::Nearest);
    assert!(exact);
    assert_eq!(at_96.ticks(), 96_000);

    // 44.1k and 48k share no convenient factor, but a whole second is exact.
    let (at_441, exact) = second_48.rescale_checked(Rate::AUDIO_44100, Rounding::Nearest);
    assert!(exact);
    assert_eq!(at_441.ticks(), 44_100);
}

// ---------------------------------------------------------------------------
// Timecode
// ---------------------------------------------------------------------------

#[test]
fn non_drop_timecode_labels_are_correct() {
    let t = RationalTime::exact(24 * 3600 + 24 * 60 * 2 + 24 * 3 + 5, Rate::F24);
    assert_eq!(t.to_timecode(), "01:02:03:05");
    assert_eq!(RationalTime::zero(Rate::F24).to_timecode(), "00:00:00:00");
    // 23.976 labels count in whole frames of 24, like every NTSC rate.
    assert_eq!(
        RationalTime::exact(24, Rate::F23_976).to_timecode(),
        "00:00:01:00"
    );
}

/// Drop-frame drops *labels*, never frames: two labels every minute except
/// every tenth. The float model disagreed with SMPTE on 96.67% of them.
#[test]
fn drop_frame_labels_follow_the_smpte_rule() {
    let tc = |f: i64| {
        RationalTime::exact(f, Rate::F29_97)
            .to_drop_frame_timecode()
            .expect("29.97 supports drop frame")
    };

    assert_eq!(tc(0), "00:00:00;00");
    // At one minute, labels ;00 and ;01 are skipped.
    assert_eq!(tc(1798), "00:00:59;28");
    assert_eq!(tc(1799), "00:00:59;29");
    assert_eq!(
        tc(1800),
        "00:01:00;02",
        "the first two labels of a minute are dropped"
    );
    assert_eq!(tc(1801), "00:01:00;03");
    // The tenth minute drops nothing.
    assert_eq!(tc(17_982), "00:10:00;00", "minute 10 must not drop");
    // An hour of drop-frame is exactly 107892 frames and labels as 01:00:00;00.
    assert_eq!(
        tc(107_892),
        "01:00:00;00",
        "drop frame must track the wall clock"
    );
}

#[test]
fn drop_frame_is_refused_where_it_is_undefined() {
    // Drop frame has no meaning at exact rates or at 25.
    assert!(RationalTime::exact(0, Rate::F30)
        .to_drop_frame_timecode()
        .is_none());
    assert!(RationalTime::exact(0, Rate::F25)
        .to_drop_frame_timecode()
        .is_none());
    assert!(RationalTime::exact(0, Rate::F24)
        .to_drop_frame_timecode()
        .is_none());
    // 23.976 is NTSC but its nominal 24 is not a multiple of 30.
    assert!(RationalTime::exact(0, Rate::F23_976)
        .to_drop_frame_timecode()
        .is_none());
    // 59.94 is.
    assert!(RationalTime::exact(0, Rate::F59_94)
        .to_drop_frame_timecode()
        .is_some());
}

// ---------------------------------------------------------------------------
// Rate construction
// ---------------------------------------------------------------------------

#[test]
fn rates_reduce_to_lowest_terms_and_reject_nonsense() {
    assert_eq!(Rate::new(48_000, 1001), Some(Rate::F47_952));
    assert_eq!(
        Rate::new(48, 2),
        Rate::new(24, 1),
        "equal rates must be one value"
    );
    assert_eq!(Rate::new(0, 24), None, "a zero rate has no meaning");
    assert_eq!(Rate::new(24, 0), None);
}

#[test]
fn the_nominal_timecode_rate_rounds_rather_than_truncates() {
    assert_eq!(
        Rate::F23_976.timecode_rate(),
        24,
        "23.976 labels count in 24s, not 23"
    );
    assert_eq!(Rate::F29_97.timecode_rate(), 30);
    assert_eq!(Rate::F59_94.timecode_rate(), 60);
    assert_eq!(Rate::F25.timecode_rate(), 25);
}

/// Reading old float-based project data must recover the exact rate, or the
/// migration reintroduces the defect it is meant to remove.
#[test]
fn legacy_float_rates_recover_their_exact_form() {
    assert_eq!(Rate::from_legacy_f64(29.97), Some(Rate::F29_97));
    assert_eq!(Rate::from_legacy_f64(30_000.0 / 1001.0), Some(Rate::F29_97));
    assert_eq!(Rate::from_legacy_f64(23.976), Some(Rate::F23_976));
    assert_eq!(Rate::from_legacy_f64(24.0), Some(Rate::F24));
    assert_eq!(Rate::from_legacy_f64(48_000.0), Some(Rate::AUDIO_48K));
    // 29.97 and 30 must not be confused with each other.
    assert_ne!(Rate::from_legacy_f64(29.97), Rate::from_legacy_f64(30.0));
    // Nonsense is refused rather than given a made-up denominator.
    assert_eq!(Rate::from_legacy_f64(0.0), None);
    assert_eq!(Rate::from_legacy_f64(-24.0), None);
    assert_eq!(Rate::from_legacy_f64(f64::NAN), None);
    assert_eq!(Rate::from_legacy_f64(23.5), None);
}

// ---------------------------------------------------------------------------
// Serialization
// ---------------------------------------------------------------------------

#[test]
fn serialization_round_trips_exactly() {
    for (name, rate) in RATES.iter().chain(AUDIO.iter()) {
        let t = RationalTime::exact(107_892, *rate);
        let json = serde_json::to_string(&t).expect("serialize");
        let back: RationalTime = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back, t, "{name} did not survive serialization");
        assert_eq!(back.ticks(), t.ticks());
        assert_eq!(back.rate_exact(), t.rate_exact());
        // The rate must persist as integers, not as a decimal.
        assert!(
            !json.contains("29.97") && !json.contains("23.976"),
            "{name} serialized a decimal rate: {json}"
        );
    }
}
