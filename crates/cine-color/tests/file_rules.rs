//! File rules against OpenColorIO 2.5.2 (`tests/data/file-rules.tsv`, from
//! `generate_file_rules.py`): a config exercising every rule kind, and the
//! released cg config's own rules.

use cine_color::ocio::Config;
use std::path::Path;

fn load(name: &str) -> Config {
    Config::load(
        &Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/data")
            .join(name),
    )
    .unwrap_or_else(|e| panic!("{name}: {e}"))
}

#[test]
fn every_path_is_assigned_the_colorspace_opencolorio_assigns() {
    let text = include_str!("data/file-rules.tsv");
    let mut checked = 0;
    for line in text.lines().filter(|l| !l.starts_with('#')).skip(1) {
        let f: Vec<&str> = line.split('\t').collect();
        let config = load(f[0]);
        assert!(
            !config.unimplemented.iter().any(|k| k == "file_rules"),
            "file rules are implemented"
        );
        let got = config
            .colorspace_for_path(f[1])
            .unwrap()
            .unwrap_or_else(|| panic!("{}: no rule matched {}", f[0], f[1]));
        assert_eq!(
            (got.colorspace.as_str(), got.rule.as_str()),
            (f[2], f[3]),
            "{} {}",
            f[0],
            f[1]
        );
        checked += 1;
    }
    assert_eq!(checked, 42);
}

#[test]
fn file_rules_survive_a_round_trip_through_yaml() {
    let config = load("file-rules/rules.ocio");
    let again = Config::parse(&config.to_yaml()).unwrap();
    assert_eq!(again.file_rules, config.file_rules);
}
