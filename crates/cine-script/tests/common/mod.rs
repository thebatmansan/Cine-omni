#![allow(dead_code)]
use cine_script::prelude::*;

pub const FOUNTAIN_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/sample.fountain"
);
pub const FDX_PATH: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../tests/fixtures/sample.fdx"
);

pub fn fixture_text() -> String {
    std::fs::read_to_string(FOUNTAIN_PATH).expect("sample.fountain fixture")
}

pub fn fixture() -> Screenplay {
    parse_fountain(&fixture_text())
}

pub fn count(sp: &Screenplay, kind: &str) -> usize {
    sp.elements.iter().filter(|e| e.kind_name() == kind).count()
}
