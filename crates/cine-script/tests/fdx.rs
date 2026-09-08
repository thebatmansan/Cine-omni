mod common;
use cine_script::prelude::*;
use common::*;

#[test]
fn fdx_round_trip_is_exact() {
    let sp = fixture();
    let xml = write_fdx(&sp);
    assert!(xml.starts_with("<?xml"));
    let back = parse_fdx(&xml).expect("writer output must parse");
    assert_eq!(back.title_page, sp.title_page);
    assert_eq!(back.elements, sp.elements);
}

#[test]
fn fountain_fdx_fountain_is_exact() {
    let sp = fixture();
    let via_fdx = parse_fdx(&write_fdx(&sp)).unwrap();
    let text = write_fountain(&via_fdx);
    let back = parse_fountain(&text);
    assert_eq!(back, sp);
}

#[test]
fn fdx_fixture_matches_fountain_fixture() {
    let sp = fixture();
    if std::env::var_os("CINE_SCRIPT_UPDATE_FIXTURES").is_some() {
        std::fs::write(FDX_PATH, write_fdx(&sp)).unwrap();
    }
    let xml = std::fs::read_to_string(FDX_PATH).expect("sample.fdx fixture");
    let from_fdx = parse_fdx(&xml).unwrap();
    assert_eq!(from_fdx, sp);
    assert_eq!(write_fdx(&sp), xml);
}

#[test]
fn fdx_escapes_special_characters() {
    let sp = Screenplay {
        title_page: vec![("Title".into(), "Tom & \"Jerry\" <3".into())],
        elements: vec![
            Element::scene_heading("INT. A & B - DAY", Some("2A".into())),
            Element::Action {
                text: "He types <html> & 'quotes'.".into(),
            },
            Element::Character {
                name: "A".into(),
                extension: Some("(O.S.)".into()),
                dual: false,
            },
            Element::Dialogue {
                text: "1 < 2 > 0".into(),
            },
            Element::PageBreak,
        ],
    };
    let xml = write_fdx(&sp);
    let back = parse_fdx(&xml).unwrap();
    assert_eq!(back, sp);
}
