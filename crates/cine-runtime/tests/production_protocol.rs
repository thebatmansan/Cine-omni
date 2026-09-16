//! The production office through the protocol: the breakdown, stripboard and
//! call sheets the screenplay implies, and a constraint change rescheduling
//! the film with its consequences named.

mod common;

use common::Harness;
use serde_json::{json, Value};

fn day_count(p: &Value) -> usize {
    p["schedule"]["days"].as_array().map(Vec::len).unwrap_or(0)
}

#[test]
fn the_screenplay_is_broken_down_scheduled_and_given_call_sheets() {
    let h = Harness::start();
    let mut c = h.client();
    let p = c.call("production", json!({}));
    let breakdowns = p["breakdowns"].as_array().unwrap();
    assert_eq!(breakdowns.len(), 2, "one breakdown per scene");
    assert!(breakdowns[0]["heading"]
        .as_str()
        .unwrap()
        .contains("HARBOUR"));
    assert!(breakdowns[0]["cast"]
        .as_array()
        .unwrap()
        .iter()
        .any(|n| n == "MAREN"));
    assert!(day_count(&p) >= 1);
    assert_eq!(
        p["call_sheets"].as_array().unwrap().len(),
        day_count(&p),
        "a call sheet per day"
    );
    let strips: usize = p["schedule"]["days"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["strips"].as_array().unwrap().len())
        .sum();
    assert_eq!(strips, 2, "every scene is on the stripboard exactly once");
}

#[test]
fn tightening_the_constraints_reschedules_and_says_what_moved() {
    let h = Harness::start();
    let mut c = h.client();
    let start = c.call("production", json!({}));
    let document = start["document"].as_str().unwrap().to_string();
    let set = |c: &mut common::Client, pages: f64| {
        let mut k = start["constraints"].clone();
        k["max_pages_per_day"] = json!(pages);
        k["group_by_location"] = json!(false);
        k["night_shoots_together"] = json!(false);
        c.call(
            "schedule_constraints",
            json!({ "document": document, "constraints": k }),
        )
    };

    // Loose: both scenes fit one day.
    set(&mut c, 10.0);
    let loose = c.call("production", json!({}));
    assert_eq!(day_count(&loose), 1, "{}", loose["schedule"]);

    // Tight: an eighth of a page a day. Neither scene fits; each gets its own
    // day, and the scheduler says it had to break the limit.
    let r = set(&mut c, 0.125);
    let departments: Vec<&str> = r["impact"]["departments"]
        .as_array()
        .unwrap()
        .iter()
        .map(|d| d["department"].as_str().unwrap())
        .collect();
    assert!(
        departments.contains(&"Production"),
        "the reschedule is in the impact: {departments:?}"
    );
    let tight = c.call("production", json!({}));
    assert_eq!(day_count(&tight), 2);
    assert!(
        !tight["schedule"]["warnings"].as_array().unwrap().is_empty(),
        "the overrun is reported"
    );
    assert_eq!(
        tight["call_sheets"].as_array().unwrap().len(),
        2,
        "a call sheet for the new day"
    );

    // Undo restores the schedule it replaced.
    c.call("undo", json!({}));
    assert_eq!(day_count(&c.call("production", json!({}))), 1);
}

#[test]
fn impossible_constraints_are_refused_and_change_nothing() {
    let h = Harness::start();
    let mut c = h.client();
    let p = c.call("production", json!({}));
    let document = p["document"].as_str().unwrap().to_string();
    let revision = c.call("project", json!({}))["revision"].clone();
    let mut raw = h.client();
    let mut bad = p["constraints"].clone();
    bad["shooting_days_per_week"] = json!(9);
    raw.send_masked(
        &json!({ "jsonrpc": "2.0", "id": 3, "method": "schedule_constraints",
                 "params": { "document": document, "constraints": bad } })
        .to_string(),
    );
    assert!(raw.read_reply(3)["error"].is_object());
    assert_eq!(c.call("project", json!({}))["revision"], revision);
}

/// Day Out of Days: the cast side of the stripboard, and the hold days that
/// make re-ordering it worth money.
#[test]
fn the_day_out_of_days_follows_the_board_and_counts_hold_days() {
    let h = Harness::start();
    let mut c = h.client();
    let p = c.call("production", json!({}));
    let dood = &p["dood"];
    assert!(!dood.is_null(), "a schedule has a DOOD");
    let days = dood["days"].as_array().unwrap();
    assert_eq!(
        days.len(),
        p["schedule"]["days"].as_array().unwrap().len(),
        "a column per shooting day"
    );
    let cast = dood["cast"].as_array().unwrap();
    assert!(!cast.is_empty(), "the screenplay has cast");
    for row in cast {
        let codes: Vec<&str> = row["days"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap_or_default())
            .collect();
        assert_eq!(codes.len(), days.len(), "every row spans every day");
        let worked = codes.iter().filter(|c| **c != "H" && !c.is_empty()).count();
        assert_eq!(
            worked as u64,
            row["worked"].as_u64().unwrap(),
            "the codes and the count agree for {}",
            row["name"]
        );
        // A row starts with a start code and ends with a finish code.
        let engaged: Vec<&&str> = codes.iter().filter(|c| !c.is_empty()).collect();
        assert!(
            engaged.first().is_some_and(|c| **c == "SW" || **c == "SWF"),
            "{}: first engaged day is a start: {codes:?}",
            row["name"]
        );
        assert!(
            engaged.last().is_some_and(|c| **c == "WF" || **c == "SWF"),
            "{}: last engaged day is a finish: {codes:?}",
            row["name"]
        );
    }
    let held: u64 = cast.iter().map(|r| r["held"].as_u64().unwrap()).sum();
    assert_eq!(held, dood["total_held"].as_u64().unwrap());

    // And it is a function of the board: the markdown export says the same.
    let text = c.call("export", json!({ "what": "dood" }));
    let text = text.as_str().unwrap_or_default();
    assert!(text.contains("# Day Out of Days"), "{text}");
    assert!(
        text.contains(&format!("{} days on hold", dood["total_held"])),
        "the export and the projection agree: {text}"
    );
}

/// Moving a strip: the board is a person's, not the scheduler's.
#[test]
fn a_strip_moved_to_a_day_stays_there_until_it_is_released() {
    let h = Harness::start();
    let mut c = h.client();
    let p = c.call("production", json!({}));
    let document = p["document"].as_str().unwrap().to_string();
    let day_of = |p: &Value, scene: &str| -> Option<u64> {
        p["schedule"]["days"].as_array()?.iter().find_map(|d| {
            d["strips"]
                .as_array()?
                .iter()
                .any(|s| s["scene"] == json!(scene))
                .then(|| d["index"].as_u64().unwrap_or(0))
        })
    };
    let scene = p["breakdowns"][0]["scene"].as_str().unwrap().to_string();
    let was = day_of(&p, &scene).expect("the scene is on the board");

    // Put it on a later day.
    let moved = c.call(
        "move_strip",
        json!({ "document": document, "scene": scene, "day": was + 2 }),
    );
    assert!(
        !moved["impact"]["departments"]
            .as_array()
            .unwrap()
            .is_empty(),
        "a move is a change with consequences"
    );
    let after = c.call("production", json!({}));
    assert_eq!(day_of(&after, &scene), Some(was + 2), "the pin is honoured");
    assert_eq!(
        after["constraints"]["pinned"][&scene].as_u64(),
        Some(was + 2),
        "and it is stored as a constraint, so a reschedule keeps it"
    );

    // Every scene is still on the board exactly once.
    let strips: Vec<String> = after["schedule"]["days"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|d| d["strips"].as_array().unwrap().iter())
        .map(|s| s["scene"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        strips.len(),
        after["breakdowns"].as_array().unwrap().len(),
        "nothing was lost or duplicated by the move: {strips:?}"
    );

    // Release it and the scheduler decides again.
    c.call(
        "move_strip",
        json!({ "document": document, "scene": scene, "day": null }),
    );
    let released = c.call("production", json!({}));
    assert_eq!(
        day_of(&released, &scene),
        Some(was),
        "back where it would be"
    );
    assert!(released["constraints"]["pinned"][&scene].is_null());

    // A day number that is not a day, and a scene that is not a scene.
    assert!(c
        .call_result(
            "move_strip",
            json!({ "document": document, "scene": scene, "day": 0 })
        )
        .is_err());
    assert!(c
        .call_result(
            "move_strip",
            json!({ "document": document, "scene": document, "day": 1 })
        )
        .is_err());

    // And undo puts the board back, like any other edit.
    c.call(
        "move_strip",
        json!({ "document": document, "scene": scene, "day": was + 1 }),
    );
    c.call("undo", json!({}));
    let undone = c.call("production", json!({}));
    assert_eq!(day_of(&undone, &scene), Some(was));
}

/// The call sheet a production actually uses: a PDF, served to the browser and
/// readable by something that is not this build.
#[test]
fn the_call_sheet_is_served_as_a_pdf_a_reader_can_open() {
    let h = Harness::start();
    let (status, headers, body) = h.get("/callsheet?day=1");
    assert!(status.contains("200"), "{status}");
    assert!(
        headers
            .iter()
            .any(|h| h.to_ascii_lowercase().contains("application/pdf")),
        "{headers:?}"
    );
    assert!(
        headers
            .iter()
            .any(|h| h.to_ascii_lowercase().contains("call-sheet-day-1.pdf")),
        "it downloads under a name that says what it is: {headers:?}"
    );
    assert!(body.starts_with(b"%PDF-1.4"), "it is a PDF");
    assert!(body.ends_with(b"%%EOF\n"));

    // A day that is not a shooting day is a 404 that says so, not an empty PDF.
    let (status, _, body) = h.get("/callsheet?day=999");
    assert!(status.contains("404"), "{status}");
    assert!(
        String::from_utf8_lossy(&body).contains("no call sheet"),
        "{}",
        String::from_utf8_lossy(&body)
    );

    // And what the PDF says is what the projection says.
    if std::process::Command::new("which")
        .arg("pdftotext")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
    {
        let dir = std::env::temp_dir().join(format!("cine-rt-callsheet-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("day1.pdf");
        let (_, _, body) = h.get("/callsheet?day=1");
        std::fs::write(&path, &body).unwrap();
        let out = std::process::Command::new("pdftotext")
            .args([path.to_str().unwrap(), "-"])
            .output()
            .expect("pdftotext");
        let text = String::from_utf8_lossy(&out.stdout).to_string();
        let mut c = h.client();
        let production = c.call("production", json!({}));
        let sheet = &production["call_sheets"][0];
        for scene in sheet["scenes"].as_array().unwrap() {
            let heading = scene["heading"].as_str().unwrap_or_default();
            assert!(text.contains(heading), "the PDF has {heading}: {text}");
        }
        std::fs::remove_dir_all(&dir).ok();
    }
}
