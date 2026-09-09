//! Colour conformance harness: `cine-color` measured against OpenColorIO.
//!
//! # What this is
//!
//! `crates/cine-color` is **not** ACES-compatible and **not** OpenColorIO-
//! compatible (see the crate docs and
//! `docs/blueprint/research/13-color-management.md`). The size of that gap was
//! once a one-off number in a document. This test turns it into a measurement
//! that runs on every change.
//!
//! It reads reference values produced by a real OpenColorIO
//! (`tests/data/ocio-reference.tsv`, whose header records the OCIO version, the
//! config, and how to regenerate it) and reports, per transform: `max|Δ|`,
//! `mean|Δ|` and the worst offending sample. OpenColorIO itself is **not**
//! needed to run this — only to regenerate the data — so the measurement is a
//! permanent gate rather than something that works on one machine.
//!
//! Three figures are printed per transform, because collapsing them into one
//! would misattribute the blame:
//!
//! * **all samples** — includes inputs outside the transform's domain;
//! * **in-domain** — negative scene light and code values outside `[0, 1]`
//!   dropped. Outside the domain OpenColorIO clamps several curves (Canon Log
//!   2/3 saturate at code 1.0) where `cine-color` extrapolates. That is a
//!   difference in domain handling, not in the curve, and it is a different
//!   finding;
//! * **in-domain neutral axis** — separates a wrong curve from a wrong matrix.
//!   A transform whose neutrals agree but whose saturated samples do not has
//!   the right tone response in the wrong colour space.
//!
//! Two documented convention changes are applied on the `cine-color` side
//! rather than reported as error, both measured rather than assumed: OCIO's
//! linear domain for ST-2084 is display-referred (1.0 = 100 nits), and its HLG
//! curve uses a linear domain exactly 3× BT.2100's.
//!
//! # What it asserts
//!
//! **It does not fail on the gap.** The gap is known, large, and closing it is
//! a port of the ACES 2.0 output transform, not a tolerance tweak. Failing here
//! today would mean deleting the test tomorrow.
//!
//! What it *does* assert is what must hold whatever the rendering is:
//!
//! * the reference data is present and non-empty, and every transform in it is
//!   either mapped to a `cine-color` entry point or listed in
//!   [`UNMAPPED`] with a reason — so coverage cannot silently shrink;
//! * neutrals stay neutral through every output transform;
//! * the display-referred output transform is monotone along the neutral ramp;
//! * `Transfer::decode(encode(x)) == x` where that is meant to be exact;
//! * `OutputTransform::apply_inverse(apply(x)) == x` in-gamut, below peak;
//! * the neutral axis of the ACES 2.0 SDR path agrees with OpenColorIO to
//!   [`NEUTRAL_GATE`] — the one conformance claim this crate can actually make,
//!   and the one that would regress silently if the tonescale were touched.
//!
//! Run `cargo test -p cine-color --test ocio_conformance -- --nocapture` to see
//! the table.

use cine_color::aces::{
    aces2065_to_acescg, acescg_to_aces2065, idt_ap0, Camera, Display, OutputTransform,
};
use cine_color::space::{
    bradford_adaptation, rgb_to_rgb_matrix, Mat3, Primaries, WHITE_ACES, WHITE_D65,
};
use cine_color::Transfer;

/// Where the committed reference values live, relative to the crate root.
const REFERENCE_TSV: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/data/ocio-reference.tsv");

/// Neutral-axis agreement gate for the ACES 2.0 SDR output transforms.
///
/// The crate's one defensible conformance claim is that its tonescale *is* the
/// ACES 2.0 tonescale, so the achromatic axis matches OpenColorIO. Measured
/// agreement on the neutral ramp is far tighter than this; the gate is loose
/// enough to survive float32 round-off in the oracle's CPU path and tight
/// enough that swapping the tonescale for something else fails it.
const NEUTRAL_GATE: f64 = 2e-5;

/// Transforms present in the reference data with no `cine-color` counterpart.
/// Each entry is a documented absence, not an oversight. A transform id in the
/// data that is neither mapped nor listed here fails the coverage assertion.
const UNMAPPED: &[(&str, &str)] = &[(
    "out/aces2.0_sdr_g22",
    "cine-color's Display enum has no gamma-2.2 Rec.709 display: Display::Rec709 \
     is BT.1886 (gamma 2.4) and Display::SRgb is the piecewise sRGB curve. \
     Synthesising one here would measure a transform the crate does not offer.",
)];

// ---------------------------------------------------------------------------
// Reference data
// ---------------------------------------------------------------------------

struct Row {
    group: String,
    transform: String,
    sample: String,
    input: [f64; 3],
    reference: [f64; 3],
}

fn parse_triple(s: &str) -> [f64; 3] {
    let mut it = s.split_whitespace().map(|v| {
        v.parse::<f64>()
            .unwrap_or_else(|e| panic!("bad number {v:?} in reference data: {e}"))
    });
    let t = [
        it.next().expect("triple needs 3 values"),
        it.next().expect("triple needs 3 values"),
        it.next().expect("triple needs 3 values"),
    ];
    assert!(it.next().is_none(), "triple has more than 3 values: {s:?}");
    t
}

fn load_reference() -> Vec<Row> {
    let text = std::fs::read_to_string(REFERENCE_TSV).unwrap_or_else(|e| {
        panic!(
            "conformance reference data missing at {REFERENCE_TSV}: {e}\n\
             This file is committed to the repository. It is not optional and the \
             harness must not pass without it. Regenerate it with the recipe in \
             its own header comment (see git history for the file)."
        )
    });
    let rows: Vec<Row> = text
        .lines()
        .filter(|l| !l.trim_start().starts_with('#') && !l.trim().is_empty())
        .map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            assert_eq!(f.len(), 5, "malformed reference row: {l:?}");
            Row {
                group: f[0].to_string(),
                transform: f[1].to_string(),
                sample: f[2].to_string(),
                input: parse_triple(f[3]),
                reference: parse_triple(f[4]),
            }
        })
        .collect();
    assert!(
        rows.len() > 500,
        "reference data has only {} rows — it is truncated or corrupt; the \
         harness would be measuring almost nothing",
        rows.len()
    );
    rows
}

// ---------------------------------------------------------------------------
// The cine-color side of each transform
// ---------------------------------------------------------------------------

/// Which channels of a reference row are meaningful.
#[derive(Clone, Copy, PartialEq)]
enum Channels {
    /// Compare all three.
    All,
    /// Compare the Y (middle) channel only. Used for the display-EOTF rows,
    /// which isolate the curve by reading luminance off the neutral axis of an
    /// XYZ-D65 result — X and Z carry only the white point, not the curve.
    LuminanceOnly,
}

/// The domain a transform's *input* lives in. Used to separate disagreement
/// inside the transform's nominal domain from different extrapolation outside
/// it — those are different findings and must not be reported as one number.
#[derive(Clone, Copy, PartialEq)]
enum Domain {
    /// Scene-linear: any non-negative value is in domain, including above 1.0.
    SceneLinear,
    /// Code values: in domain is `[0, 1]`. Beyond it OpenColorIO clamps some
    /// curves (Canon Log 2/3 saturate) where `cine-color` extrapolates.
    CodeValue,
}

struct Mapping {
    /// `cine-color`'s answer for the same input.
    eval: Box<dyn Fn([f64; 3]) -> [f64; 3]>,
    channels: Channels,
    domain: Domain,
    /// One line saying what is being compared, printed with the table.
    note: &'static str,
}

impl Domain {
    fn contains(self, v: [f64; 3]) -> bool {
        match self {
            Domain::SceneLinear => v.iter().all(|c| *c >= 0.0),
            Domain::CodeValue => v.iter().all(|c| (0.0..=1.0).contains(c)),
        }
    }
}

fn per_channel(f: impl Fn(f64) -> f64 + 'static) -> Box<dyn Fn([f64; 3]) -> [f64; 3]> {
    Box::new(move |c: [f64; 3]| c.map(&f))
}

fn matrix_eval(m: Mat3) -> Box<dyn Fn([f64; 3]) -> [f64; 3]> {
    Box::new(move |c| m.apply(c))
}

fn ap0_to_xyz_d65() -> Mat3 {
    bradford_adaptation(WHITE_ACES, WHITE_D65).mul(&Primaries::ACES_AP0.rgb_to_xyz())
}

fn ap1_to_xyz_d65() -> Mat3 {
    bradford_adaptation(WHITE_ACES, WHITE_D65).mul(&Primaries::ACES_AP1.rgb_to_xyz())
}

fn display_eval(d: Display) -> Box<dyn Fn([f64; 3]) -> [f64; 3]> {
    let ot = OutputTransform::for_display(d);
    Box::new(move |c| ot.display.encode_nits(ot.to_display_nits(c)))
}

/// AP0 in, CIE XYZ D65 out with 1.0 = 100 nits — the OCIO display-reference
/// convention, and the form `ocio.rs` uses for `ACES-OUTPUT` builtin styles.
fn display_xyz_eval(d: Display) -> Box<dyn Fn([f64; 3]) -> [f64; 3]> {
    let op = OutputTransform::for_display(d).to_xyz_d65_op();
    Box::new(move |c| op.apply(aces2065_to_acescg(c)))
}

fn idt_eval(cam: Camera) -> Box<dyn Fn([f64; 3]) -> [f64; 3]> {
    let p = idt_ap0(cam);
    Box::new(move |c| p.apply_f64(c))
}

/// `cine-color`'s counterpart for a `group/transform` id, or `None` when the
/// crate has no such entry point.
fn mapping(group: &str, transform: &str) -> Option<Mapping> {
    use Channels::{All, LuminanceOnly};
    use Domain::{CodeValue, SceneLinear};
    let m = |eval, channels, domain, note| {
        Some(Mapping {
            eval,
            channels,
            domain,
            note,
        })
    };
    match (group, transform) {
        // OCIO's linear side of the ST-2084 curve is display-referred
        // (1.0 = 100 nits); cine-color's Transfer::Pq takes a fraction of the
        // 10 000-nit PQ ceiling. The /100 and *100 are that convention change.
        ("tf", "pq_encode") => m(
            per_channel(|x| Transfer::Pq.encode(x / 100.0)),
            All,
            SceneLinear,
            "Transfer::Pq.encode; OCIO linear is 1.0 = 100 nits",
        ),
        ("tf", "pq_decode") => m(
            per_channel(|x| Transfer::Pq.decode(x) * 100.0),
            All,
            CodeValue,
            "Transfer::Pq.decode; OCIO linear is 1.0 = 100 nits",
        ),
        // Measured, not assumed: OCIO's HLG curve puts scene light on a domain
        // exactly 3x BT.2100's normalised one (OCIO decode(1.0) = 3.0 where
        // BT.2100 gives 1.0; the ratio is 3.0000005 across the whole curve,
        // i.e. float32 noise). That is a convention difference, not a curve
        // difference, so it is divided out here rather than reported as error.
        ("tf", "hlg_encode") => m(
            per_channel(|x| Transfer::Hlg.encode(x / 3.0)),
            All,
            SceneLinear,
            "Transfer::Hlg.encode; OCIO linear domain is 3x BT.2100's",
        ),
        ("tf", "hlg_decode") => m(
            per_channel(|x| Transfer::Hlg.decode(x) * 3.0),
            All,
            CodeValue,
            "Transfer::Hlg.decode; OCIO linear domain is 3x BT.2100's",
        ),
        ("tf", "acescct_encode") => m(
            per_channel(|x| Transfer::ACEScct.encode(x)),
            All,
            SceneLinear,
            "Transfer::ACEScct.encode",
        ),
        ("tf", "acescct_decode") => m(
            per_channel(|x| Transfer::ACEScct.decode(x)),
            All,
            CodeValue,
            "Transfer::ACEScct.decode",
        ),
        ("tf", "canonlog2_encode") => m(
            per_channel(|x| Transfer::CanonLog2.encode(x)),
            All,
            SceneLinear,
            "Transfer::CanonLog2.encode (OCIO saturates the code at 1.0; this does not)",
        ),
        ("tf", "canonlog2_decode") => m(
            per_channel(|x| Transfer::CanonLog2.decode(x)),
            All,
            CodeValue,
            "Transfer::CanonLog2.decode (OCIO clamps code >1.0; this extrapolates)",
        ),
        ("tf", "canonlog3_encode") => m(
            per_channel(|x| Transfer::CanonLog3.encode(x)),
            All,
            SceneLinear,
            "Transfer::CanonLog3.encode (OCIO saturates the code at 1.0; this does not)",
        ),
        ("tf", "canonlog3_decode") => m(
            per_channel(|x| Transfer::CanonLog3.decode(x)),
            All,
            CodeValue,
            "Transfer::CanonLog3.decode (OCIO clamps code >1.0; this extrapolates)",
        ),
        ("tf", "srgb_display_decode_Y") => m(
            per_channel(|x| Transfer::SRgb.decode(x)),
            LuminanceOnly,
            CodeValue,
            "Transfer::SRgb.decode vs OCIO sRGB display EOTF, neutral axis",
        ),
        ("tf", "g24_rec709_decode_Y") => m(
            per_channel(|x| Transfer::Gamma(2.4).decode(x)),
            LuminanceOnly,
            CodeValue,
            "Transfer::Gamma(2.4).decode vs OCIO BT.1886 Rec.709, neutral axis",
        ),
        ("tf", "g22_rec709_decode_Y") => m(
            per_channel(|x| Transfer::Gamma(2.2).decode(x)),
            LuminanceOnly,
            CodeValue,
            "Transfer::Gamma(2.2).decode vs OCIO gamma-2.2 Rec.709, neutral axis",
        ),
        ("tf", "p3d65_display_decode_Y") => m(
            per_channel(|x| Transfer::SRgb.decode(x)),
            LuminanceOnly,
            CodeValue,
            "Transfer::SRgb.decode vs OCIO Display P3 EOTF, neutral axis",
        ),
        ("tf", "pq_rec2100_decode_Y") => m(
            per_channel(|x| Transfer::Pq.decode(x) * 100.0),
            LuminanceOnly,
            CodeValue,
            "Transfer::Pq.decode vs OCIO Rec.2100-PQ display EOTF, neutral axis",
        ),
        ("tf", "hlg_rec2100_decode_Y") => m(
            Box::new(|c: [f64; 3]| {
                cine_color::transfer::hlg_eotf_nits(c, 1000.0).map(|n| n / 100.0)
            }),
            LuminanceOnly,
            CodeValue,
            "hlg_eotf_nits(1000) vs OCIO Rec.2100-HLG 1000-nit display EOTF, neutral axis",
        ),

        ("mtx", "ap0_to_xyz_d65") => m(
            matrix_eval(ap0_to_xyz_d65()),
            All,
            SceneLinear,
            "Bradford(ACES->D65) * AP0.rgb_to_xyz()",
        ),
        ("mtx", "ap1_to_xyz_d65") => m(
            matrix_eval(ap1_to_xyz_d65()),
            All,
            SceneLinear,
            "Bradford(ACES->D65) * AP1.rgb_to_xyz()",
        ),
        ("mtx", "ap1_to_lin_rec709") => m(
            matrix_eval(rgb_to_rgb_matrix(&Primaries::ACES_AP1, &Primaries::REC709)),
            All,
            SceneLinear,
            "rgb_to_rgb_matrix(AP1, Rec709)",
        ),
        ("mtx", "lin_rec709_to_ap1") => m(
            matrix_eval(rgb_to_rgb_matrix(&Primaries::REC709, &Primaries::ACES_AP1)),
            All,
            SceneLinear,
            "rgb_to_rgb_matrix(Rec709, AP1)",
        ),
        ("mtx", "acescg_to_aces2065") => m(
            Box::new(acescg_to_aces2065),
            All,
            SceneLinear,
            "aces::acescg_to_aces2065",
        ),
        ("mtx", "aces2065_to_acescg") => m(
            Box::new(aces2065_to_acescg),
            All,
            SceneLinear,
            "aces::aces2065_to_acescg",
        ),

        ("idt", "arri_logc4") => m(idt_eval(Camera::LogC4), All, CodeValue, "idt_ap0(LogC4)"),
        ("idt", "arri_logc3_ei800") => m(idt_eval(Camera::LogC3), All, CodeValue, "idt_ap0(LogC3)"),
        ("idt", "sony_slog3_sgamut3") => m(
            idt_eval(Camera::SLog3SGamut3),
            All,
            CodeValue,
            "idt_ap0(SLog3SGamut3)",
        ),
        ("idt", "sony_slog3_sgamut3cine") => m(
            idt_eval(Camera::SLog3SGamut3Cine),
            All,
            CodeValue,
            "idt_ap0(SLog3SGamut3Cine)",
        ),
        ("idt", "red_log3g10_rwg") => m(
            idt_eval(Camera::Log3G10Rwg),
            All,
            CodeValue,
            "idt_ap0(Log3G10Rwg)",
        ),
        ("idt", "panasonic_vlog_vgamut") => {
            m(idt_eval(Camera::VLog), All, CodeValue, "idt_ap0(VLog)")
        }
        ("idt", "canon_clog2_cgamut") => m(
            idt_eval(Camera::CanonLog2),
            All,
            CodeValue,
            "idt_ap0(CanonLog2)",
        ),
        ("idt", "canon_clog3_cgamut") => m(
            idt_eval(Camera::CanonLog3),
            All,
            CodeValue,
            "idt_ap0(CanonLog3)",
        ),

        ("out", "aces2.0_sdr_srgb") => m(
            display_eval(Display::SRgb),
            All,
            SceneLinear,
            "OutputTransform::for_display(SRgb)",
        ),
        ("out", "aces2.0_sdr_rec1886") => m(
            display_eval(Display::Rec709),
            All,
            SceneLinear,
            "OutputTransform::for_display(Rec709)",
        ),
        ("out", "aces2.0_sdr_p3d65") => m(
            display_eval(Display::P3D65),
            All,
            SceneLinear,
            "OutputTransform::for_display(P3D65)",
        ),
        ("out", "aces2.0_hdr_pq_rec2020") => m(
            display_eval(Display::Rec2020Pq1000),
            All,
            SceneLinear,
            "OutputTransform::for_display(Rec2020Pq1000)",
        ),
        ("out", "aces2.0_hdr_hlg_rec2020_ap0") => {
            let inner = display_eval(Display::Rec2020Hlg);
            m(
                Box::new(move |c| inner(aces2065_to_acescg(c))),
                All,
                SceneLinear,
                "OutputTransform::for_display(Rec2020Hlg), AP0 input",
            )
        }
        ("out", "aces2.0_sdr_rec709_xyz") => m(
            display_xyz_eval(Display::Rec709),
            All,
            SceneLinear,
            "OutputTransform::for_display(Rec709).to_xyz_d65_op()",
        ),
        // Not a like-for-like comparison but the most consequential row in the
        // table: `ocio.rs::builtin_ops` recognises this ACES *1.0* style name
        // ACES 1.0's output transform is NOT implemented, and the builtin style
        // naming it is now refused rather than answered with ACES 2.0. This row
        // no longer measures a substitution — it measures the distance between
        // OCIO's ACES 1.0 and this build's ACES 2.0, which is the evidence for
        // refusing: 0.18 apart on the neutral axis, where every other output
        // path here matches OCIO's neutrals to 5e-6.
        ("out", "aces1.0_sdr_video_xyz") => m(
            display_xyz_eval(Display::Rec709),
            All,
            SceneLinear,
            "ACES 1.0 (OCIO) vs ACES 2.0 (this build) — why the substitution is refused",
        ),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Measurement
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Stats {
    n: usize,
    max: f64,
    sum: f64,
    worst_sample: String,
    worst_input: [f64; 3],
    worst_ref: [f64; 3],
    worst_got: [f64; 3],
}

impl Stats {
    fn push(&mut self, row: &Row, got: [f64; 3], channels: Channels) {
        let idx: &[usize] = match channels {
            Channels::All => &[0, 1, 2],
            Channels::LuminanceOnly => &[1],
        };
        let mut d: f64 = 0.0;
        for &i in idx {
            if !row.reference[i].is_finite() || !got[i].is_finite() {
                d = f64::INFINITY;
                break;
            }
            d = d.max((row.reference[i] - got[i]).abs());
        }
        self.n += 1;
        self.sum += d;
        if d > self.max || self.n == 1 {
            self.max = d;
            self.worst_sample = row.sample.clone();
            self.worst_input = row.input;
            self.worst_ref = row.reference;
            self.worst_got = got;
        }
    }
    fn mean(&self) -> f64 {
        if self.n == 0 {
            0.0
        } else {
            self.sum / self.n as f64
        }
    }
}

fn fmt3(v: [f64; 3]) -> String {
    format!("{:.6} {:.6} {:.6}", v[0], v[1], v[2])
}

/// The measurement. Prints the table; never fails on disagreement.
#[test]
fn ocio_conformance_report() {
    let rows = load_reference();

    // Group ids in first-seen order so the table is stable.
    let mut order: Vec<(String, String)> = Vec::new();
    for r in &rows {
        let key = (r.group.clone(), r.transform.clone());
        if !order.contains(&key) {
            order.push(key);
        }
    }

    println!();
    println!("cine-color vs OpenColorIO — conformance measurement");
    println!("reference: {REFERENCE_TSV}");
    println!(
        "{} transforms, {} samples. Delta is per-channel absolute, in output units.",
        order.len(),
        rows.len()
    );
    println!(
        "Each transform gets two lines. The first is every sample. The second,\n\
         `in-domain`, drops inputs outside the transform's nominal domain\n\
         (negative scene light; code values outside [0,1]) — outside it OCIO\n\
         clamps several curves where cine-color extrapolates, and that is a\n\
         different finding from disagreeing about the curve itself."
    );
    println!();
    println!(
        "{:<5} {:<28} {:>4} {:>11} {:>11}  worst sample",
        "grp", "transform", "n", "max|d|", "mean|d|"
    );
    println!("{}", "-".repeat(112));

    let mut unmapped_seen: Vec<String> = Vec::new();
    let mut last_group = String::new();
    let mut neutral_gate_failures: Vec<String> = Vec::new();
    let mut mapped = 0usize;

    for (group, transform) in &order {
        let id = format!("{group}/{transform}");
        let Some(mp) = mapping(group, transform) else {
            unmapped_seen.push(id);
            continue;
        };
        mapped += 1;
        if *group != last_group {
            if !last_group.is_empty() {
                println!();
            }
            last_group = group.clone();
        }
        let mut stats = Stats::default();
        let mut in_domain = Stats::default();
        // The neutral axis is tracked separately: it is the crate's only
        // defensible conformance claim, so it gets its own gate.
        let mut neutral = Stats::default();
        for r in rows
            .iter()
            .filter(|r| r.group == *group && r.transform == *transform)
        {
            let got = (mp.eval)(r.input);
            stats.push(r, got, mp.channels);
            if mp.domain.contains(r.input) {
                in_domain.push(r, got, mp.channels);
                if r.input[0] == r.input[1] && r.input[1] == r.input[2] {
                    neutral.push(r, got, mp.channels);
                }
            }
        }
        let line = |label: &str, s: &Stats| {
            println!(
                "{:<5} {:<28} {:>4} {:>11.6} {:>11.6}  {} in=({}) ocio=({}) cine=({})",
                group,
                label,
                s.n,
                s.max,
                s.mean(),
                s.worst_sample,
                fmt3(s.worst_input),
                fmt3(s.worst_ref),
                fmt3(s.worst_got),
            )
        };
        line(transform, &stats);
        line("  \u{21b3} in-domain", &in_domain);
        // The neutral axis separates a wrong curve from a wrong matrix: a
        // transform whose neutral agrees but whose saturated samples do not
        // has the right tone response in the wrong colour space.
        println!(
            "{:<5} {:<28} {:>4} {:>11.6} {:>11.6}",
            group,
            "  \u{21b3} in-domain neutral axis",
            neutral.n,
            neutral.max,
            neutral.mean()
        );
        println!("      {}", mp.note);

        // Gate: SDR neutral axis. See NEUTRAL_GATE.
        if group == "out"
            && matches!(
                transform.as_str(),
                "aces2.0_sdr_srgb" | "aces2.0_sdr_rec1886"
            )
            && neutral.max > NEUTRAL_GATE
        {
            neutral_gate_failures.push(format!(
                "{id}: neutral-axis max|d| = {:.9} > {NEUTRAL_GATE} (worst {} in=({}) ocio=({}) cine=({}))",
                neutral.max,
                neutral.worst_sample,
                fmt3(neutral.worst_input),
                fmt3(neutral.worst_ref),
                fmt3(neutral.worst_got),
            ));
        }
    }

    println!();
    println!(
        "mapped {mapped} transforms; {} in the data have no cine-color counterpart",
        unmapped_seen.len()
    );
    for u in &unmapped_seen {
        println!("  UNMAPPED {u}");
    }
    println!();
    println!(
        "This table is a measurement, not a pass/fail. cine-color is not \
         ACES-compatible and not OpenColorIO-compatible; the `out` rows are the \
         missing JMh / chroma-compress / gamut-compress stages of the ACES 2.0 \
         output transform, quantified."
    );

    // Coverage: an unmapped transform must be a documented absence, so that
    // renaming or deleting a cine-color entry point cannot quietly reduce what
    // is measured.
    for u in &unmapped_seen {
        assert!(
            UNMAPPED.iter().any(|(id, _)| id == u),
            "reference data contains transform `{u}` with no cine-color mapping \
             and no entry in UNMAPPED. Either map it or record why it cannot be \
             mapped — an unexplained gap in the measurement is how a regression \
             hides."
        );
    }
    assert!(
        mapped >= 30,
        "only {mapped} transforms measured; the harness has lost coverage"
    );

    // The one agreement gate. Everything else above is reported, not enforced.
    assert!(
        neutral_gate_failures.is_empty(),
        "the ACES 2.0 neutral axis no longer agrees with OpenColorIO. This is \
         the crate's only conformance claim — its tonescale is, constant for \
         constant, the ACES 2.0 tonescale. Something changed it:\n  {}",
        neutral_gate_failures.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Invariants that must hold whatever the rendering is
// ---------------------------------------------------------------------------

/// Neutral in must be neutral out of every output transform. The AP1→display
/// matrix carries a chromatic adaptation, so this is a real property of the
/// matrices, not a tautology.
#[test]
fn neutrals_stay_neutral() {
    for d in [
        Display::SRgb,
        Display::Rec709,
        Display::P3D65,
        Display::Rec2020Pq1000,
        Display::Rec2020Hlg,
    ] {
        let ot = OutputTransform::for_display(d);
        for v in [0.0, 1e-4, 0.01, 0.05, 0.18, 0.5, 0.9, 1.0, 2.0, 8.0, 100.0] {
            let code = ot.display.encode_nits(ot.to_display_nits([v, v, v]));
            let spread = code[0].max(code[1]).max(code[2]) - code[0].min(code[1]).min(code[2]);
            assert!(
                spread < 1e-6,
                "{}: neutral {v} left the achromatic axis: {code:?} (spread {spread:e})",
                d.name()
            );
        }
    }
}

/// The output transform must be monotone along the neutral ramp: more scene
/// light can never produce a darker display value.
#[test]
fn output_transform_is_monotone_on_neutrals() {
    let ramp: Vec<f64> = (0..400).map(|i| (i as f64 / 40.0).exp2() * 1e-4).collect();
    for d in [
        Display::SRgb,
        Display::Rec709,
        Display::P3D65,
        Display::Rec2020Pq1000,
        Display::Rec2020Hlg,
    ] {
        let ot = OutputTransform::for_display(d);
        let mut prev = f64::NEG_INFINITY;
        for &v in &ramp {
            let y = ot.display.encode_nits(ot.to_display_nits([v, v, v]))[1];
            assert!(
                y >= prev - 1e-9,
                "{}: not monotone at {v}: {y} < {prev}",
                d.name()
            );
            prev = y;
        }
    }
}

/// Every transfer function must round-trip. This is exact by construction —
/// each `decode` is the analytic inverse of its `encode` — so a failure is a
/// real defect, not a tolerance question.
#[test]
fn transfer_functions_round_trip() {
    let curves = [
        Transfer::SRgb,
        Transfer::Rec709,
        Transfer::Gamma(2.2),
        Transfer::Gamma(2.4),
        Transfer::Pq,
        Transfer::Hlg,
        Transfer::LogC3,
        Transfer::LogC4,
        Transfer::SLog3,
        Transfer::Log3G10,
        Transfer::CanonLog2,
        Transfer::CanonLog3,
        Transfer::VLog,
        Transfer::ACEScc,
        Transfer::ACEScct,
    ];
    for t in curves {
        let peak = t.linear_peak();
        for i in 0..=200 {
            let x = peak * i as f64 / 200.0;
            let back = t.decode(t.encode(x));
            let tol = 1e-9 * x.max(1.0) + 1e-9;
            assert!(
                (back - x).abs() < tol.max(1e-7),
                "{:?}: encode/decode round trip failed at {x}: got {back}",
                t
            );
        }
    }
}

/// The output transform must invert exactly inside the display gamut and below
/// the peak. Outside it the forward clamp is lossy and no inverse exists —
/// that is documented behaviour, and this test stays inside the domain where
/// exactness is claimed.
#[test]
fn output_transform_round_trips_in_gamut() {
    for d in [Display::SRgb, Display::Rec709, Display::P3D65] {
        let ot = OutputTransform::for_display(d);
        // Neutral and lightly-tinted values, all comfortably inside Rec.709 and
        // below the tonescale roof.
        let inputs = [
            [0.02, 0.02, 0.02],
            [0.18, 0.18, 0.18],
            [0.5, 0.5, 0.5],
            [0.2, 0.18, 0.16],
            [0.1, 0.14, 0.12],
            [0.35, 0.30, 0.25],
        ];
        for input in inputs {
            let code = ot.apply(input.map(|v| v as f32));
            let back = ot.apply_inverse(code);
            for i in 0..3 {
                assert!(
                    (back[i] as f64 - input[i]).abs() < 2e-4,
                    "{}: round trip {input:?} -> {code:?} -> {back:?}",
                    d.name()
                );
            }
        }
    }
}
