mod common;
use cine_script::prelude::*;
use common::*;
use ucg_schema::IntExt;

#[test]
fn parses_fixture_with_expected_counts() {
    let sp = fixture();
    assert_eq!(sp.title_page.len(), 5);
    assert_eq!(sp.title(), Some("THE LAST FERRY"));
    assert_eq!(
        sp.title_value("Contact"),
        Some("CINE-OMNI Engineering\nfixtures@example.invalid")
    );

    assert_eq!(count(&sp, "scene_heading"), 6);
    assert_eq!(count(&sp, "action"), 23);
    assert_eq!(count(&sp, "character"), 27);
    assert_eq!(count(&sp, "dialogue"), 27);
    assert_eq!(count(&sp, "parenthetical"), 4);
    assert_eq!(count(&sp, "transition"), 3);
    assert_eq!(count(&sp, "centered"), 1);
    assert_eq!(count(&sp, "section"), 1);
    assert_eq!(count(&sp, "synopsis"), 1);
    assert_eq!(count(&sp, "note"), 1);
    assert_eq!(count(&sp, "lyric"), 1);
    assert_eq!(count(&sp, "page_break"), 1);
    assert_eq!(count(&sp, "boneyard"), 1);
    assert_eq!(sp.elements.len(), 97);
}

#[test]
fn headings_are_parsed() {
    let sp = fixture();
    let headings: Vec<&Element> = sp
        .elements
        .iter()
        .filter(|e| matches!(e, Element::SceneHeading { .. }))
        .collect();
    let Element::SceneHeading {
        text,
        scene_number,
        heading,
    } = headings[0]
    else {
        unreachable!()
    };
    assert_eq!(text, "INT. HARBOR OFFICE - NIGHT");
    assert_eq!(scene_number.as_deref(), Some("1"));
    assert_eq!(heading.int_ext, IntExt::Int);
    assert_eq!(heading.location, "HARBOR OFFICE");
    assert_eq!(heading.time_of_day, "NIGHT");

    let Element::SceneHeading { heading, .. } = headings[2] else {
        unreachable!()
    };
    assert_eq!(heading.location, "ALBATROSS - WHEELHOUSE");
    assert_eq!(heading.time_of_day, "CONTINUOUS");

    let Element::SceneHeading {
        text,
        heading,
        scene_number,
    } = headings[3]
    else {
        unreachable!()
    };
    assert_eq!(text, "ON THE STRAIT - NIGHT");
    assert_eq!(heading.int_ext, IntExt::Other);
    assert_eq!(heading.location, "ON THE STRAIT");
    assert!(scene_number.is_none());

    let Element::SceneHeading { heading, .. } = headings[5] else {
        unreachable!()
    };
    assert_eq!(heading.int_ext, IntExt::Ext);
    assert_eq!(heading.time_of_day, "DAWN");
}

#[test]
fn dual_dialogue_extensions_and_forced_elements() {
    let sp = fixture();
    let cues: Vec<&Element> = sp
        .elements
        .iter()
        .filter(|e| matches!(e, Element::Character { .. }))
        .collect();
    let duals: Vec<&&Element> = cues
        .iter()
        .filter(|c| matches!(c, Element::Character { dual: true, .. }))
        .collect();
    assert_eq!(duals.len(), 1);
    assert_eq!(
        **duals[0],
        Element::Character {
            name: "ELIAS".into(),
            extension: None,
            dual: true
        }
    );
    assert!(cues.contains(&&Element::Character {
        name: "McALLISTER".into(),
        extension: Some("(V.O.)".into()),
        dual: false
    }));
    assert!(cues.contains(&&Element::Character {
        name: "MAREN".into(),
        extension: Some("(CONT'D)".into()),
        dual: false
    }));

    assert!(sp.elements.contains(&Element::Action {
        text: "WHAM. WHAM. WHAM.\nSomething heavy pounds against the hull from below.".into()
    }));
    assert!(sp.elements.contains(&Element::Transition {
        text: "FADE OUT.".into()
    }));
    assert!(sp.elements.contains(&Element::Centered {
        text: "THE CROSSING".into()
    }));
    assert!(sp.elements.contains(&Element::Section {
        level: 1,
        text: "ACT ONE".into()
    }));
    assert!(sp.elements.contains(&Element::Dialogue {
        text: "Evening, Maren.\n\nLong time.".into()
    }));
    assert!(sp.elements.contains(&Element::Dialogue {
        text: "Tonight isn't a *good* night.".into()
    }));
    assert!(sp.elements.contains(&Element::Lyric {
        text:
            "Down by the water where the grey gulls cry\nShe waits for the boat that never comes by"
                .into()
    }));
    assert!(sp.elements.contains(&Element::Boneyard {
        text: "Cut for length: the coffee-pot exchange from the first draft.".into()
    }));
}

#[test]
fn scenes_group_correctly() {
    let sp = fixture();
    let scenes = sp.scenes();
    assert_eq!(scenes.len(), 7, "preamble + 6 headed scenes");
    assert!(scenes[0].heading.is_none());
    assert_eq!(scenes[0].elements.len(), 2);
    assert_eq!(scenes[1].number, Some("1"));
    assert_eq!(scenes[1].characters(), vec!["MAREN", "ELIAS"]);
    assert_eq!(scenes[3].characters(), vec!["MCALLISTER", "ELIAS", "MAREN"]);
    assert_eq!(scenes[5].characters(), vec!["MAREN", "ELIAS", "JONAH"]);
    assert_eq!(scenes[5].dialogue_lines().len(), 14);
}

#[test]
fn fountain_round_trip_is_exact() {
    let sp = fixture();
    let text = write_fountain(&sp);
    let back = parse_fountain(&text);
    assert_eq!(back.title_page, sp.title_page);
    assert_eq!(back.elements, sp.elements);
    // And the writer is stable: writing again yields identical text.
    assert_eq!(write_fountain(&back), text);
}
