mod common;
use cine_script::prelude::*;
use cine_script::stats::dialogue_word_count;
use common::*;
use ucg_schema::IntExt;

#[test]
fn tags_fixture() {
    let sp = fixture();
    let tags = auto_tag(&sp);

    let names: Vec<&str> = tags.characters.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, vec!["MAREN", "ELIAS", "MCALLISTER", "JONAH"]);
    let maren = &tags.characters[0];
    assert_eq!(maren.scenes, vec![1, 2, 3, 5]);
    assert_eq!(maren.line_count, 14);
    assert_eq!(tags.characters[3].scenes, vec![5]);
    assert_eq!(tags.characters[3].line_count, 2);
    assert_eq!(tags.characters[2].line_count, 1);

    let locs: Vec<(&str, IntExt)> = tags
        .locations
        .iter()
        .map(|l| (l.name.as_str(), l.int_ext))
        .collect();
    assert_eq!(
        locs,
        vec![
            ("HARBOR OFFICE", IntExt::Int),
            ("FERRY DOCK", IntExt::Ext),
            ("ALBATROSS - WHEELHOUSE", IntExt::Int),
            ("ON THE STRAIT", IntExt::Other),
            ("FAR SHORE", IntExt::Ext),
        ]
    );
    assert_eq!(tags.locations[2].scenes, vec![3, 5]);

    let props: Vec<&str> = tags.props.iter().map(|p| p.name.as_str()).collect();
    for expected in [
        "MANIFESTS",
        "STAMP",
        "SATCHEL",
        "ALBATROSS",
        "LINE",
        "RADIO",
        "BRASS KEY",
        "KEY",
        "HANDSET",
        "FLASHLIGHT",
        "PHONE",
        "ROPE",
    ] {
        assert!(
            props.contains(&expected),
            "missing prop {expected} in {props:?}"
        );
    }
    for not_prop in ["MAREN", "ELIAS", "JONAH", "WHAM", "RINGS", "BANGS", "BACK"] {
        assert!(
            !props.contains(&not_prop),
            "{not_prop} should not be a prop"
        );
    }
    let satchel = tags.props.iter().find(|p| p.name == "SATCHEL").unwrap();
    assert_eq!(satchel.scenes, vec![1, 2, 5, 6]);
    // Ordering is by first appearance.
    assert_eq!(
        props[0], "LAMP",
        "the desk lamp in the first action line comes first: {props:?}"
    );
    assert!(
        props.iter().position(|p| *p == "MANIFESTS")
            < props.iter().position(|p| *p == "FLASHLIGHT")
    );
}

#[test]
fn stats_are_sane() {
    let sp = fixture();
    let scenes = sp.scenes();
    for s in &scenes {
        assert!(page_eighths(s) >= 1);
        assert!(estimate_duration_seconds(s) >= 0.0);
        if s.heading.is_some() {
            assert!(estimate_duration_seconds(s) > 0.0);
        }
    }
    // The long wheelhouse scene is the biggest; the two-paragraph strait scene the smallest.
    let eighths: Vec<u32> = scenes.iter().map(page_eighths).collect();
    let big = eighths[5];
    let small = eighths[4];
    assert!(big > small, "{eighths:?}");
    assert!(
        big >= 8,
        "a dialogue-heavy scene of this length is about a page: {eighths:?}"
    );
    let total: u32 = eighths.iter().sum();
    assert!(
        (20..=40).contains(&total),
        "fixture is about 3-4 pages, got {total}/8"
    );

    // Duration is monotone in text length: doubling the dialogue doubles the words term.
    let words = dialogue_word_count(&scenes[5]);
    assert!(words > 40, "scene 5 has {words} dialogue words");
    let mut longer = sp.clone();
    for e in longer.elements.iter_mut() {
        if let Element::Dialogue { text } = e {
            let t = text.clone();
            text.push(' ');
            text.push_str(&t);
        }
    }
    let l = longer.scenes();
    for (a, b) in scenes.iter().zip(l.iter()) {
        assert!(estimate_duration_seconds(b) >= estimate_duration_seconds(a));
        assert!(page_eighths(b) >= page_eighths(a));
    }
    assert!(estimate_duration_seconds(&l[5]) > estimate_duration_seconds(&scenes[5]));
}

#[test]
fn word_list_normalizes() {
    assert_eq!(
        word_list("Tonight isn't a *good* night."),
        vec!["tonight", "isn't", "a", "good", "night"]
    );
    assert_eq!(word_list("Give it BACK."), vec!["give", "it", "back"]);
}
