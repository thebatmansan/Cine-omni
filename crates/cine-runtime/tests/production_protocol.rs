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
