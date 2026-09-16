//! Colour through the protocol: what the project's config offers, what every
//! media file's colorspace is and who decided it, and choosing what everything
//! is looked at through.

mod common;

use common::Harness;
use serde_json::{json, Value};
use std::time::Duration;

fn media_rows(p: &Value) -> Vec<&Value> {
    p["media"].as_array().expect("media").iter().collect()
}

#[test]
fn the_project_reports_its_config_and_what_each_file_is_in() {
    let h = Harness::start();
    let mut c = h.client();
    let p = c.call("color", json!({}));
    let source = p["config"]["source"].as_str().unwrap_or_default();
    assert!(
        source.contains("cg-config-v4.0.0_aces-v2.0_ocio-v2.5"),
        "the config says where it came from: {source}"
    );
    let displays = p["displays"].as_array().expect("displays");
    assert!(displays.len() >= 5, "{} displays", displays.len());
    let srgb = displays
        .iter()
        .find(|d| d["name"] == "sRGB - Display")
        .expect("an sRGB display");
    assert!(
        srgb["views"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v["name"].as_str().unwrap_or_default().contains("ACES 2.0")),
        "the ACES view is offered"
    );
    assert!(
        p["colorspaces"]
            .as_array()
            .unwrap()
            .iter()
            .any(|c| c["name"] == "ACEScct"),
        "the config's colorspaces are listed for anyone choosing one"
    );

    // Every media file the timeline brought in has a colorspace and a reason.
    let media = media_rows(&p);
    assert!(!media.is_empty(), "the imported timeline has media");
    for m in media {
        let cs = m["input_colorspace"].as_str().unwrap_or_default();
        let why = m["source"].as_str().unwrap_or_default();
        assert!(!cs.is_empty(), "{m} has no input colorspace");
        assert!(
            why.starts_with("rule:"),
            "{m} does not say which rule decided"
        );
    }
}

#[test]
fn choosing_a_view_reaches_every_surface_and_a_wrong_one_is_refused() {
    let h = Harness::start();
    let mut a = h.client();
    let mut b = h.client();
    assert!(a.call("color", json!({}))["view"].is_null(), "none chosen");

    let set = a.call(
        "color_view",
        json!({ "display": "Rec.1886 Rec.709 - Display", "view": "ACES 2.0 - SDR 100 nits (Rec.709)" }),
    );
    assert_eq!(set["view"]["display"], "Rec.1886 Rec.709 - Display");

    // The other client is told, without asking.
    let mut heard = None;
    b.stream_messages(Duration::from_millis(500), |m| {
        if m["method"] == "view" {
            heard = Some(m["params"].clone());
        }
    });
    let heard = heard.expect("the session told the other client");
    assert_eq!(heard["view"], "ACES 2.0 - SDR 100 nits (Rec.709)");
    assert_eq!(
        b.call("color", json!({}))["view"]["display"],
        "Rec.1886 Rec.709 - Display"
    );

    // A display that does not exist, and a view that display does not have.
    let bad_display = a.call_result(
        "color_view",
        json!({ "display": "Nonexistent", "view": "ACES 2.0 - SDR 100 nits (Rec.709)" }),
    );
    assert!(bad_display.is_err(), "{bad_display:?}");
    let bad_view = a.call_result(
        "color_view",
        json!({ "display": "sRGB - Display", "view": "Kodak 2383" }),
    );
    assert!(bad_view.is_err(), "{bad_view:?}");

    // And it can be returned to the config's default.
    let cleared = a.call("color_view", json!({ "display": null, "view": null }));
    assert!(cleared["view"].is_null());
}

#[test]
fn someone_can_say_what_a_file_really_is_and_undo_it() {
    let h = Harness::start();
    let mut c = h.client();
    let before = c.call("color", json!({}));
    let first = media_rows(&before)[0].clone();
    let id = first["id"].as_str().unwrap().to_string();
    let was = first["input_colorspace"].as_str().unwrap().to_string();

    let set = c.call(
        "set_input_colorspace",
        json!({ "media": id, "colorspace": "ACEScct" }),
    );
    let departments: Vec<String> = set["impact"]["departments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["department"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        departments.iter().any(|d| d == "Colour" || d == "Color"),
        "a colour change is reported as colour work: {departments:?}"
    );

    let after = c.call("color", json!({}));
    let row = media_rows(&after)
        .into_iter()
        .find(|m| m["id"] == json!(id))
        .unwrap()
        .clone();
    assert_eq!(row["input_colorspace"], "ACEScct");
    assert_eq!(row["source"], "user", "a person decided, not a rule");

    // A colorspace the config does not have is refused, not stored.
    assert!(c
        .call_result(
            "set_input_colorspace",
            json!({ "media": id, "colorspace": "Kodak Vision3" })
        )
        .is_err());

    c.call("undo", json!({}));
    let restored = c.call("color", json!({}));
    let row = media_rows(&restored)
        .into_iter()
        .find(|m| m["id"] == json!(id))
        .unwrap()
        .clone();
    assert_eq!(row["input_colorspace"], json!(was));
    assert!(row["source"].as_str().unwrap().starts_with("rule:"));
}

/// Grading a clip: the numbers go in, the projection says the clip is graded,
/// and a refusal is a refusal rather than a flat frame.
#[test]
fn a_clip_can_be_graded_and_the_grade_is_part_of_what_it_is() {
    let h = Harness::start();
    let mut c = h.client();
    let colour = c.call("color", json!({}));
    let clips = colour["clips"].as_array().expect("clips");
    if clips.is_empty() {
        eprintln!("SKIP: the fixture timeline has no clips bound to media");
        return;
    }
    let clip = clips[0]["clip"].as_str().unwrap().to_string();
    assert_eq!(clips[0]["graded"], json!(false), "nothing is graded yet");

    let graded = c.call(
        "set_grade",
        json!({
            "clip": clip,
            "slope": [1.1, 1.0, 0.9],
            "offset": [0.0, 0.0, 0.02],
            "power": [1.0, 1.0, 1.0],
            "saturation": 0.9
        }),
    );
    let departments: Vec<String> = graded["impact"]["departments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["department"].as_str().unwrap_or_default().to_string())
        .collect();
    assert!(
        departments
            .iter()
            .any(|d| d.eq_ignore_ascii_case("colour") || d.eq_ignore_ascii_case("color")),
        "a grade is colour work: {departments:?}"
    );

    let after = c.call("color", json!({}));
    let row = after["clips"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["clip"] == json!(clip))
        .unwrap()
        .clone();
    assert_eq!(row["graded"], json!(true));
    assert_eq!(row["cdl"]["slope"][0], json!(1.1));
    assert_eq!(row["cdl"]["saturation"], json!(0.9));

    // A power of zero is a flat frame, not a grade.
    assert!(c
        .call_result(
            "set_grade",
            json!({ "clip": clip, "power": [0.0, 1.0, 1.0] })
        )
        .is_err());
    // And so is a value that is not a number.
    assert!(c
        .call_result("set_grade", json!({ "clip": clip, "slope": [1.0, 1.0] }))
        .is_err());

    // Undo takes the grade off.
    c.call("undo", json!({}));
    let undone = c.call("color", json!({}));
    let row = undone["clips"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["clip"] == json!(clip))
        .unwrap()
        .clone();
    assert_eq!(row["graded"], json!(false));
}
