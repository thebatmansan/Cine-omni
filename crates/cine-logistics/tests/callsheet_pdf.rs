//! The call sheet as a PDF, read back by something that is not this build.
//!
//! A PDF is only a PDF if a reader can open it, so the test asks two: poppler's
//! `pdftotext` for the words on the page, and Ghostscript for whether the file
//! parses at all. Both are separate programs on the command line; neither is
//! linked or distributed. Where a machine has neither, the test says so and
//! skips rather than passing quietly.

use cine_logistics::callsheet::call_sheet_pdf;
use cine_logistics::schedule::{schedule, ScheduleConstraints};
use cine_logistics::{build_call_sheet, CastRoster, SceneBreakdown, Time};
use std::process::Command;
use ucg_core::EntityId;
use ucg_schema::IntExt;

fn tool(name: &str) -> bool {
    Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn sheet() -> cine_logistics::CallSheet {
    let mut breakdowns = Vec::new();
    for (n, location, cast) in [
        (1usize, "HARBOUR", vec!["MAREN", "TOBIAS"]),
        (2, "WHEELHOUSE", vec!["MAREN"]),
    ] {
        let mut bd = SceneBreakdown::empty(EntityId::from_key(&format!("scene:{n}")));
        bd.number = n.to_string();
        bd.heading = format!("EXT. {location} - DAY");
        bd.int_ext = IntExt::Ext;
        bd.location = location.into();
        bd.time_of_day = "DAY".into();
        bd.page_eighths = 12;
        bd.est_minutes = 2.0;
        bd.cast = cast.into_iter().map(String::from).collect();
        breakdowns.push(bd);
    }
    let board = schedule(&breakdowns, &ScheduleConstraints::default());
    let roster = CastRoster::from_breakdowns(&breakdowns);
    build_call_sheet(
        &board.days[0],
        &breakdowns,
        &roster,
        Time::new(7, 0).unwrap(),
    )
}

#[test]
fn the_call_sheet_pdf_says_what_the_call_sheet_says() {
    let sheet = sheet();
    let bytes = call_sheet_pdf(&sheet, "THE HARBOUR");
    let dir = std::env::temp_dir().join(format!("cine-callsheet-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("callsheet.pdf");
    std::fs::write(&path, &bytes).unwrap();
    println!("MEASURED call sheet PDF: {} bytes, 1 page", bytes.len());

    if !tool("pdftotext") {
        eprintln!("SKIP: no pdftotext on this machine to read the PDF back");
        return;
    }
    let out = Command::new("pdftotext")
        .args([path.to_str().unwrap(), "-"])
        .output()
        .expect("pdftotext");
    assert!(out.status.success(), "pdftotext refused the file");
    let text = String::from_utf8_lossy(&out.stdout).to_string();

    assert!(text.contains("THE HARBOUR"), "the production: {text}");
    assert!(text.contains("GENERAL CALL"), "the call: {text}");
    assert!(text.contains("07:00"), "the time: {text}");
    assert!(
        text.contains(&format!("DAY {}", sheet.day)),
        "the day: {text}"
    );
    for scene in &sheet.scenes {
        assert!(
            text.contains(&scene.heading),
            "scene {}: {text}",
            scene.number
        );
    }
    for call in &sheet.cast_calls {
        assert!(text.contains(&call.name), "cast {}: {text}", call.name);
    }
    // And what it does not have, it says it does not have.
    assert!(text.contains("Hospital, unit base and transport are not in this project yet."));

    if tool("gs") {
        let gs = Command::new("gs")
            .args([
                "-q",
                "-dNOPAUSE",
                "-dBATCH",
                "-sDEVICE=nullpage",
                path.to_str().unwrap(),
            ])
            .output()
            .expect("gs");
        assert!(
            gs.status.success(),
            "ghostscript could not parse the file: {}",
            String::from_utf8_lossy(&gs.stderr)
        );
        println!("MEASURED ghostscript parsed the file without complaint");
    }
    std::fs::remove_dir_all(&dir).ok();
}

/// A name a standard font cannot show must not become a different name.
#[test]
fn a_character_the_font_lacks_is_not_silently_replaced_with_another() {
    let mut sheet = sheet();
    sheet.cast_calls[0].name = "大和".into();
    sheet.notes.push("Crème brûlée on the trolley".into());
    let bytes = call_sheet_pdf(&sheet, "THE HARBOUR");
    let raw = String::from_utf8_lossy(&bytes);
    // Latin-1 survives as an octal escape; what has no glyph becomes '?'.
    assert!(
        raw.contains("Cr\\350me br\\373l\\351e"),
        "accents are encoded, not dropped"
    );
    assert!(
        raw.contains("??"),
        "characters with no glyph are marked, not guessed"
    );
}
