//! OCIO display styles against OpenColorIO 2.5.2 (`tests/data/display-styles.tsv`).

use cine_color::ocio::builtin_ops;
use cine_color::processor::Direction;
use cine_color::processor::Processor;

#[test]
fn display_styles_match_opencolorio_in_and_out_of_range() {
    let text = include_str!("data/display-styles.tsv");
    let mut worst: std::collections::BTreeMap<String, f64> = Default::default();
    let mut rows = 0;
    for line in text
        .lines()
        .filter(|l| !l.starts_with('#') && !l.starts_with("style"))
    {
        let f: Vec<&str> = line.split('\t').collect();
        let n = |i: usize| f[i].parse::<f64>().unwrap();
        let style = f[0];
        let p = Processor::new(
            builtin_ops(style, Direction::Forward).unwrap_or_else(|e| panic!("{style}: {e}")),
        );
        let got = p.apply_f64([n(1), n(2), n(3)]);
        let want = [n(4), n(5), n(6)];
        let d = (0..3).map(|i| (got[i] - want[i]).abs()).fold(0.0, f64::max);
        let w = worst.entry(style.to_string()).or_default();
        *w = w.max(d);
        rows += 1;
        // Round trip through the inverse.
        let inv = Processor::new(builtin_ops(style, Direction::Inverse).unwrap());
        let back = inv.apply_f64(got);
        for i in 0..3 {
            assert!(
                (back[i] - [n(1), n(2), n(3)][i]).abs() < 1e-9,
                "{style} does not invert"
            );
        }
    }
    assert_eq!(rows, 70);
    for (style, d) in &worst {
        println!("MEASURED {style}: max |Δ| vs OpenColorIO {d:.2e}");
        // OpenColorIO's CPU path is 32-bit float.
        assert!(*d < 2e-5, "{style} differs from OpenColorIO by {d}");
    }
}
