//! The camera's numbering, all the way out to an EDL.
//!
//! The point of reading a timecode track is that the numbers leave the
//! building. An EDL's source columns are what a facility looks for on the
//! camera original; if they count from zero, every event points at the head of
//! the tape instead of at the take. These tests import real media with real
//! timecode, cut with it, and read the EDL the runtime writes.

mod common;

use common::{Client, Harness};
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use ucg_core::Rate;

fn project(dir: &Path) -> PathBuf {
    let root = dir.join("project");
    std::fs::create_dir_all(&root).unwrap();
    root
}

fn wait_for(c: &mut Client, id: u64) -> serde_json::Value {
    let deadline = Instant::now() + Duration::from_secs(600);
    loop {
        let jobs = c.call("jobs", json!({}));
        if let Some(job) = jobs.as_array().unwrap().iter().find(|j| j["id"] == id) {
            if matches!(
                job["state"].as_str().unwrap(),
                "succeeded" | "failed" | "cancelled"
            ) {
                return job.clone();
            }
        }
        assert!(Instant::now() < deadline, "job {id} did not finish");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn import(c: &mut Client, path: &Path) -> String {
    let job = c.call(
        "job_submit",
        json!({ "kind": "media.ingest", "path": path.display().to_string(),
                "thumbnail": false, "waveform": false }),
    );
    let done = wait_for(c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    done["output"]["id"].as_str().unwrap().to_string()
}

/// Cut `frames` of `media` starting `from` frames into it, at `rate`.
fn cut(c: &mut Client, media: &str, from: i64, frames: i64, rate: Rate) -> String {
    let made = c.call(
        "new_timeline",
        json!({ "name": "Conform", "rate_num": rate.num(), "rate_den": rate.den() }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    let at = |n: i64| json!({ "ticks": n, "rate_num": rate.num(), "rate_den": rate.den() });
    c.call(
        "place",
        json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": at(0),
                "clip": { "media": media,
                          "source_range": { "start_time": at(from), "duration": at(frames) } } }),
    );
    c.call("focus", json!({ "timeline": tl }));
    tl
}

fn edl(c: &mut Client) -> String {
    c.call("export", json!({ "what": "edl" }))
        .as_str()
        .expect("an EDL")
        .to_string()
}

#[test]
fn an_edls_source_columns_are_the_cameras_numbers_not_an_offset_from_zero() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    // `bars_and_pops` writes a roll starting at one hour.
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 2).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    // Twelve frames, starting a second in: 01:00:01:00 to 01:00:01:12.
    cut(&mut c, &media, 24, 12, Rate::F24);
    let text = edl(&mut c);
    let event = text
        .lines()
        .find(|l| l.starts_with("001"))
        .unwrap_or_else(|| panic!("no event in:\n{text}"));
    assert!(
        event.contains("01:00:01:00 01:00:01:12"),
        "the source columns are the camera's: {event}\n{text}"
    );
    assert!(
        event.contains("00:00:00:00 00:00:00:12"),
        "and the record columns are the cut's own, which starts at zero: {event}"
    );
}

#[test]
fn media_with_no_timecode_counts_from_zero_and_says_nothing_it_does_not_know() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("untimed.mov");
    cine_media::testsignal::bars_and_pops_from(&source, 160, 90, Rate::F24, 2, None).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    assert!(
        c.call("media", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["id"] == json!(media))
            .unwrap()["timecode"]
            .is_null(),
        "a file with no timecode track has none, rather than zero"
    );

    cut(&mut c, &media, 24, 12, Rate::F24);
    let text = edl(&mut c);
    let event = text.lines().find(|l| l.starts_with("001")).unwrap();
    assert!(
        event.contains("00:00:01:00 00:00:01:12"),
        "source columns count from zero, which is all anybody knows: {event}"
    );
}

#[test]
fn a_drop_frame_take_is_labelled_drop_frame_all_the_way_out() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("B001C001.mov");
    let rate = Rate::new(30000, 1001).unwrap();
    cine_media::testsignal::bars_and_pops(&source, 160, 90, rate, 2).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    let view = c
        .call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(media))
        .unwrap()
        .clone();
    assert_eq!(view["timecode"]["start"], "01:00:00;00", "{view}");
    assert_eq!(view["timecode"]["drop_frame"], true);
    assert_eq!(view["timecode"]["rate_num"], 30000);
    assert_eq!(view["timecode"]["rate_den"], 1001);

    // Thirty frames in from the take's start. A drop-frame label is written
    // with a semicolon, and reading it as non-drop would be a different
    // instant — so the EDL must carry the semicolon.
    cut(&mut c, &media, 30, 15, rate);
    let text = edl(&mut c);
    let event = text.lines().find(|l| l.starts_with("001")).unwrap();
    assert!(
        event.contains("01:00:01;00 01:00:01;15"),
        "drop frame survives to the EDL: {event}\n{text}"
    );
    // Frame Code Mode tells a reader how to interpret every label that follows,
    // so it has to agree with them. NON-DROP above `01:00:01;00` would have a
    // reader take it for 108030 frames when it means 107922.
    assert!(
        text.contains("FCM: DROP FRAME"),
        "the file declares the mode its labels are in:\n{text}"
    );
    assert!(
        event.contains("00:00:00;00 00:00:00;15"),
        "and the record columns are in the same mode: {event}"
    );
}

#[test]
fn a_cut_at_an_exact_rate_says_non_drop_and_means_it() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("C001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 2).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    cut(&mut c, &media, 0, 12, Rate::F24);
    let text = edl(&mut c);
    assert!(text.contains("FCM: NON-DROP FRAME"), "{text}");
    // Drop-frame is meaningless at 24: there is nothing to drift against.
    assert!(
        !text.contains(';'),
        "no drop-frame label at an exact rate:\n{text}"
    );
}

/// The source-in and source-out columns of the first event, as written.
fn source_columns(text: &str) -> (String, String) {
    let event = text
        .lines()
        .find(|l| l.starts_with("001"))
        .unwrap_or_else(|| panic!("no event in:\n{text}"));
    let cols: Vec<&str> = event.split_whitespace().collect();
    let n = cols.len();
    (cols[n - 4].to_string(), cols[n - 3].to_string())
}

#[test]
fn every_rate_this_build_claims_conforms_to_the_frame() {
    // A clip that begins at a non-zero offset inside a take whose timecode is
    // not zero: the case where every way of getting this wrong shows up.
    //
    // The labels are read back and turned into frame numbers rather than
    // compared as strings, because that is what a conform does with them: the
    // question is not what the label looks like but which frame it names.
    struct Case {
        rate: Rate,
        start: &'static str,
        offset: i64,
        frames: i64,
    }
    let cases = [
        Case {
            rate: Rate::F24,
            start: "01:00:00:00",
            offset: 24,
            frames: 12,
        },
        Case {
            rate: Rate::whole(25).unwrap(),
            start: "10:11:12:13",
            offset: 25,
            frames: 25,
        },
        Case {
            rate: Rate::whole(30).unwrap(),
            start: "00:00:00:00",
            offset: 1,
            frames: 29,
        },
        // 23.976: an exact-looking label on an NTSC signal.
        Case {
            rate: Rate::new(24000, 1001).unwrap(),
            start: "07:00:00:23",
            offset: 3,
            frames: 20,
        },
        // 29.97 non-drop and drop-frame, which name the same frames differently.
        Case {
            rate: Rate::new(30000, 1001).unwrap(),
            start: "01:00:00:00",
            offset: 30,
            frames: 15,
        },
        Case {
            rate: Rate::new(30000, 1001).unwrap(),
            start: "01:00:00;00",
            offset: 30,
            frames: 15,
        },
        // 59.94 drop-frame drops four labels a minute, not two.
        Case {
            rate: Rate::new(60000, 1001).unwrap(),
            start: "00:09:59;59",
            offset: 60,
            frames: 30,
        },
    ];

    for (i, c) in cases.iter().enumerate() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join(format!("R{i:03}.mov"));
        let tc = cine_media::time::Timecode::parse(c.start, c.rate)
            .unwrap_or_else(|e| panic!("{} at {:?}: {e}", c.start, c.rate));
        cine_media::testsignal::bars_and_pops_from(&source, 160, 90, c.rate, 3, Some(tc)).unwrap();

        let h = Harness::start_in(project(dir.path()));
        let mut c2 = h.client();
        let media = import(&mut c2, &source);
        cut(&mut c2, &media, c.offset, c.frames, c.rate);
        let text = edl(&mut c2);

        // The file says which naming its labels are in, and they are read in it.
        let drop = text.contains("FCM: DROP FRAME");
        assert_eq!(
            drop,
            c.rate.supports_drop_frame(),
            "{:?} declares the mode its labels are in:\n{text}",
            c.rate
        );
        let (in_col, out_col) = source_columns(&text);
        assert_eq!(
            in_col.contains(';'),
            drop,
            "the labels match the declared mode: {in_col}\n{text}"
        );

        let frame_of = |label: &str| -> i64 {
            cine_media::time::Timecode::parse(label, c.rate)
                .unwrap_or_else(|e| panic!("{label} at {:?}: {e}", c.rate))
                .to_frame_number()
                .unwrap()
        };
        let take_start = tc.to_frame_number().unwrap();
        assert_eq!(
            frame_of(&in_col),
            take_start + c.offset,
            "{:?} source-in names the frame the clip starts on ({} + {}): {in_col}\n{text}",
            c.rate,
            take_start,
            c.offset
        );
        assert_eq!(
            frame_of(&out_col) - frame_of(&in_col),
            c.frames,
            "{:?} source-out is exclusive and the event is {} frames long: {in_col} {out_col}",
            c.rate,
            c.frames
        );
    }
}

#[test]
fn a_non_drop_source_relabelled_drop_frame_names_the_same_frame() {
    // The two namings are two names for one instant. Re-labelling is faithful
    // as long as the file says which naming it is in — the whole purpose of
    // the FCM line. Getting this wrong by 108 frames at the hour is the
    // classic conform error.
    let rate = Rate::new(30000, 1001).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("ND.mov");
    let non_drop = cine_media::time::Timecode::parse("01:00:00:00", rate).unwrap();
    assert!(!non_drop.drop_frame);
    assert_eq!(non_drop.to_frame_number().unwrap(), 108_000);
    cine_media::testsignal::bars_and_pops_from(&source, 160, 90, rate, 2, Some(non_drop)).unwrap();

    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    // The file kept the non-drop flag, and the media list says so.
    let view = c
        .call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(media))
        .unwrap()
        .clone();
    assert_eq!(view["timecode"]["start"], "01:00:00:00", "{view}");
    assert_eq!(view["timecode"]["drop_frame"], false);

    cut(&mut c, &media, 0, 10, rate);
    let (in_col, _) = source_columns(&edl(&mut c));
    assert_eq!(in_col, "01:00:03;18", "frame 108000 in drop-frame naming");
    assert_eq!(
        cine_media::time::Timecode::parse(&in_col, rate)
            .unwrap()
            .to_frame_number()
            .unwrap(),
        108_000,
        "which is the same frame the take starts on"
    );
}

#[test]
fn a_trimmed_clip_moves_its_source_columns_and_its_record_columns_independently() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("T001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 3).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    // Two events: the second starts later in the take and later on the track.
    let tl = cut(&mut c, &media, 24, 12, Rate::F24);
    let t = c.call("timeline", json!({ "id": tl }));
    let at = |n: i64| json!({ "ticks": n, "rate_num": 24, "rate_den": 1 });
    c.call(
        "place",
        json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": at(12),
                "clip": { "media": media,
                          "source_range": { "start_time": at(48), "duration": at(6) } } }),
    );
    let text = edl(&mut c);
    let events: Vec<&str> = text.lines().filter(|l| l.starts_with("00")).collect();
    assert_eq!(events.len(), 2, "{text}");
    assert!(
        events[0].contains("01:00:01:00 01:00:01:12 00:00:00:00 00:00:00:12"),
        "{}\n{text}",
        events[0]
    );
    assert!(
        events[1].contains("01:00:02:00 01:00:02:06 00:00:00:12 00:00:00:18"),
        "the second event reads later in the take and later on the track: {}\n{text}",
        events[1]
    );
}

#[test]
fn a_take_whose_timecode_cannot_label_a_frame_says_so_in_the_edl() {
    // A counter is not a clock. The EDL counts its source columns from zero —
    // and says why, in the file, rather than handing a facility numbers that
    // point at the head of the tape.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("COUNTER.mov");
    let tc = cine_media::time::Timecode::parse("00:00:07:00", Rate::F24).unwrap();
    cine_media::testsignal::bars_and_pops_from(&source, 160, 90, Rate::F24, 2, Some(tc)).unwrap();
    // Flip the counter bit, as a camera that counts rather than clocks would.
    let mut data = std::fs::read(&source).unwrap();
    let at = counter_flag_offset(&data).expect("a tmcd entry");
    let flags = u32::from_be_bytes(data[at..at + 4].try_into().unwrap());
    data[at..at + 4].copy_from_slice(&(flags | 0x0008).to_be_bytes());
    std::fs::write(&source, &data).unwrap();

    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    let view = c
        .call("media", json!({}))
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["id"] == json!(media))
        .unwrap()
        .clone();
    assert_eq!(view["timecode"]["counter"], true, "{view}");
    assert_eq!(view["timecode"]["labels_frames"], false, "{view}");
    assert_eq!(
        view["timecode"]["start"], "00:00:07:00",
        "still reported: {view}"
    );

    cut(&mut c, &media, 24, 12, Rate::F24);
    let text = edl(&mut c);
    let (in_col, _) = source_columns(&text);
    assert_eq!(in_col, "00:00:01:00", "counted from zero: {text}");
    assert!(
        text.contains("NOT IN THIS EDL") && text.to_uppercase().contains("COUNTER"),
        "and the file says why it counted from zero:\n{text}"
    );
}

/// Byte offset of the `flags` field in the timecode sample description.
fn counter_flag_offset(data: &[u8]) -> Option<usize> {
    // Walked literally, so the test does not depend on the reader it checks.
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let size = u32::from_be_bytes(data[i..i + 4].try_into().ok()?) as usize;
        let (size, header) = if size == 1 {
            (
                u64::from_be_bytes(data[i + 8..i + 16].try_into().ok()?) as usize,
                16,
            )
        } else if size == 0 {
            (data.len() - i, 8)
        } else {
            (size, 8)
        };
        if size < header {
            return None;
        }
        if &data[i + 4..i + 8] == b"moov" {
            // Inside the moov, find the `tmcd` sample entry: it is the only
            // `tmcd` box that is preceded by a `stsd`.
            let end = i + size;
            let mut j = i + header;
            let mut last_stsd = None;
            while j + 8 <= end {
                if &data[j..j + 4] == b"stsd" {
                    last_stsd = Some(j);
                }
                if &data[j..j + 4] == b"tmcd" && last_stsd.is_some_and(|s| j > s && j - s < 32) {
                    // `tmcd` type, then 8 bytes of sample-entry preamble and
                    // 4 reserved bytes, then the flags.
                    return Some(j + 4 + 8 + 4);
                }
                j += 1;
            }
            return None;
        }
        i += size;
    }
    None
}

#[test]
fn a_retimed_events_m2_record_is_in_source_timecode_too() {
    // The M2 record names the first picture the event shows, and a reader
    // looks for that picture on the camera original — so it is the camera's
    // numbering, exactly like the source columns beside it. Counting it from
    // zero while the source columns counted from an hour would put the speed
    // record on a different frame from the event it describes.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("M001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 4).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    let tl = cut(&mut c, &media, 48, 24, Rate::F24);

    let t = c.call("timeline", json!({ "id": tl }));
    let track = t["tracks"][0]["id"].as_str().unwrap().to_string();
    let clip = c.call("timeline", json!({ "id": tl }))["tracks"][0]["items"][0]["id"]
        .as_str()
        .expect("a clip on the track")
        .to_string();
    let _ = track;
    // Half speed: 24 frames of media over 48 of track.
    let done = c.call(
        "retime",
        json!({ "clip": clip, "played": { "ticks": 48, "rate_num": 24, "rate_den": 1 } }),
    );
    assert!(done.get("error").is_none(), "{done}");

    let text = edl(&mut c);
    let m2 = text
        .lines()
        .find(|l| l.starts_with("M2"))
        .unwrap_or_else(|| panic!("no M2 record in:\n{text}"));
    assert!(
        m2.contains("01:00:02:00"),
        "the M2 record names the first picture in the camera's numbering: {m2}\n{text}"
    );
    assert!(
        m2.contains("12.00"),
        "and the speed it plays at: {m2}\n{text}"
    );
    let (in_col, _) = source_columns(&text);
    assert_eq!(
        in_col, "01:00:02:00",
        "which is the frame the event starts on: {text}"
    );
}

/// A sequence at `rate` starting at `start_timecode`, with `frames` of `media`
/// from `offset` frames into the take.
fn cut_starting(
    c: &mut Client,
    media: &str,
    start_timecode: &str,
    offset: i64,
    frames: i64,
    rate: Rate,
) -> String {
    let made = c.call(
        "new_timeline",
        json!({ "name": "Reel 1", "rate_num": rate.num(), "rate_den": rate.den(),
                "start_timecode": start_timecode }),
    );
    let tl = made["id"].as_str().unwrap().to_string();
    let t = c.call("timeline", json!({ "id": tl }));
    let at = |n: i64| json!({ "ticks": n, "rate_num": rate.num(), "rate_den": rate.den() });
    c.call(
        "place",
        json!({ "tracks": [t["tracks"][0]["id"], t["tracks"][1]["id"]], "time": at(0),
                "clip": { "media": media,
                          "source_range": { "start_time": at(offset), "duration": at(frames) } } }),
    );
    c.call("focus", json!({ "timeline": tl }));
    tl
}

#[test]
fn a_sequence_has_a_record_timecode_and_the_edls_record_columns_are_it() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 3).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    // A reel that starts at an hour, cut from a take that starts at an hour:
    // the two clocks are independent and both have to come out right.
    let tl = cut_starting(&mut c, &media, "01:00:00:00", 24, 12, Rate::F24);
    let state = c.call("sequence_start", json!({ "timeline": tl }));
    assert_eq!(state["start_timecode"], "01:00:00:00", "{state}");
    assert_eq!(state["frame"], 86_400);

    let text = edl(&mut c);
    let event = text.lines().find(|l| l.starts_with("001")).unwrap();
    assert!(
        event.contains("01:00:01:00 01:00:01:12 01:00:00:00 01:00:00:12"),
        "source columns from the take, record columns from the reel: {event}\n{text}"
    );

    // Moved to a different reel start, the record columns move and the source
    // columns do not: they are different clocks.
    let moved = c.call(
        "sequence_start",
        json!({ "timeline": tl, "start_timecode": "10:00:00:00" }),
    );
    assert_eq!(moved["start_timecode"], "10:00:00:00", "{moved}");
    let event = edl(&mut c)
        .lines()
        .find(|l| l.starts_with("001"))
        .unwrap()
        .to_string();
    assert!(
        event.contains("01:00:01:00 01:00:01:12 10:00:00:00 10:00:00:12"),
        "only the record columns moved: {event}"
    );
}

#[test]
fn a_sequence_start_is_read_at_the_sequences_own_rate() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("B001C001.mov");
    let rate = Rate::new(30000, 1001).unwrap();
    cine_media::testsignal::bars_and_pops(&source, 160, 90, rate, 2).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);

    // At 29.97 a drop-frame label and a non-drop one are different instants,
    // so the sequence is started with the one that was meant.
    let tl = cut_starting(&mut c, &media, "01:00:00;00", 0, 10, rate);
    let state = c.call("sequence_start", json!({ "timeline": tl }));
    assert_eq!(
        state["frame"], 107_892,
        "the drop-frame label was meant: {state}"
    );
    assert_eq!(state["drop_frame"], true);
    assert_eq!(state["start_timecode"], "01:00:00;00");

    // The non-drop label of the same name is 108 frames later.
    let other = c.call(
        "sequence_start",
        json!({ "timeline": tl, "start_timecode": "01:00:00:00" }),
    );
    assert_eq!(other["frame"], 108_000, "a different instant: {other}");

    // A label that does not exist in drop-frame naming is refused rather than
    // rounded to one that does.
    let refused = c
        .call_result(
            "sequence_start",
            json!({ "timeline": tl, "start_timecode": "00:01:00;00" }),
        )
        .expect_err("a label drop-frame skips is not a time");
    assert!(
        refused.contains("start_timecode"),
        "a dropped label is not a time: {refused}"
    );
}

#[test]
fn a_rendered_movie_carries_the_record_timecode_of_its_first_frame() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("C001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 3).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    let tl = cut_starting(&mut c, &media, "01:00:00:00", 0, 24, Rate::F24);

    // A ranged render is called what those frames are called in the cut, not
    // what the head of the cut is called.
    let job = c.call(
        "job_submit",
        json!({ "kind": "render.sequence", "timeline": tl, "output": "movie",
                "start_frame": 6, "end_frame": 18 }),
    );
    let done = wait_for(&mut c, job["id"].as_u64().unwrap());
    assert_eq!(done["state"], "succeeded", "{done}");
    let movie =
        std::path::PathBuf::from(done["output"]["path"].as_str().unwrap()).join("movie.mov");
    assert!(movie.is_file(), "{movie:?}");

    let found = cine_media::tmcd::source_timecode(&movie).expect("a timecode track");
    assert_eq!(found.start.to_string(), "01:00:00:06", "{found:?}");
    assert!(!found.counter);
    assert_eq!(found.runs, 1);
    assert_eq!(
        cine_media::probe::probe(&movie)
            .unwrap()
            .labelling_timecode()
            .map(|t| t.to_string()),
        Some("01:00:00:06".to_string()),
        "and it is a clock, so it labels frames"
    );
}

#[test]
fn otio_is_written_in_the_medias_own_numbering_and_read_back_into_ours() {
    // An OTIO reader takes a media reference's available range as the media's
    // own coordinates. Writing `0 … duration` for a take the camera numbered
    // 01:00:00:00 is not a simplification a reader can see — it is a statement
    // that the take begins at midnight.
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("A001C001.mov");
    cine_media::testsignal::bars_and_pops(&source, 160, 90, Rate::F24, 3).unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let media = import(&mut c, &source);
    cut_starting(&mut c, &media, "01:00:00:00", 24, 12, Rate::F24);

    let text = c
        .call("export", json!({ "what": "otio" }))
        .as_str()
        .unwrap()
        .to_string();
    let doc: Value = serde_json::from_str(&text).expect("OTIO json");
    let clip = doc["tracks"]["children"][0]["children"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["OTIO_SCHEMA"].as_str().unwrap_or("").starts_with("Clip"))
        .expect("a clip");

    let value = |v: &Value| -> i64 { v["value"].as_f64().unwrap() as i64 };
    // The take starts at an hour, and the clip plays from a second into it.
    assert_eq!(
        value(&clip["media_references"]["DEFAULT_MEDIA"]["available_range"]["start_time"]),
        86_400,
        "the available range begins where the camera says the take does: {clip}"
    );
    assert_eq!(
        value(&clip["source_range"]["start_time"]),
        86_424,
        "and the source range is in the same numbering: {clip}"
    );
    assert_eq!(
        value(&clip["source_range"]["duration"]),
        12,
        "the length is untouched"
    );
    // The cut's own clock is carried separately, and is not the camera's.
    assert_eq!(value(&doc["global_start_time"]), 86_400);

    // Read back, it lands in the coordinates this build edits in — otherwise
    // every trim would clamp against numbers from another space.
    let imported = c.call(
        "import_otio",
        json!({ "text": text, "key": "round-trip.otio" }),
    );
    assert!(imported.get("error").is_none(), "{imported}");
    let back = c.call("timeline", json!({ "id": imported["id"] }));
    let item = &back["tracks"][0]["items"][0];
    assert_eq!(
        item["source_range"]["start"]["ticks"], 24,
        "media-relative again: {item}"
    );
    assert_eq!(item["source_range"]["duration"]["ticks"], 12, "{item}");
}

#[test]
fn otio_from_another_tool_lands_in_our_coordinates() {
    // Never written by us: in timecode all along, which is what every other
    // tool produces. Importing it must not leave source ranges an hour out.
    let dir = tempfile::tempdir().unwrap();
    let h = Harness::start_in(project(dir.path()));
    let mut c = h.client();
    let doc = json!({
        "OTIO_SCHEMA": "Timeline.1",
        "name": "From elsewhere",
        "global_start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 86400.0 },
        "tracks": {
            "OTIO_SCHEMA": "Stack.1",
            "name": "tracks",
            "children": [{
                "OTIO_SCHEMA": "Track.1",
                "name": "V1",
                "kind": "Video",
                "children": [{
                    "OTIO_SCHEMA": "Clip.1",
                    "name": "A001C001",
                    "source_range": {
                        "OTIO_SCHEMA": "TimeRange.1",
                        "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 90000.0 },
                        "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 48.0 }
                    },
                    "media_reference": {
                        "OTIO_SCHEMA": "ExternalReference.1",
                        "target_url": "/card/A001C001.mov",
                        "available_range": {
                            "OTIO_SCHEMA": "TimeRange.1",
                            "start_time": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 86400.0 },
                            "duration": { "OTIO_SCHEMA": "RationalTime.1", "rate": 24.0, "value": 7200.0 }
                        }
                    }
                }]
            }]
        }
    });
    let imported = c.call(
        "import_otio",
        json!({ "text": doc.to_string(), "key": "elsewhere.otio" }),
    );
    assert!(imported.get("error").is_none(), "{imported}");
    let back = c.call("timeline", json!({ "id": imported["id"] }));
    let item = &back["tracks"][0]["items"][0];
    // 90000 - 86400 = 3600 frames into the take, and the length is unchanged.
    assert_eq!(
        item["source_range"]["start"]["ticks"], 3_600,
        "an hour of camera numbering is taken off: {item}"
    );
    assert_eq!(item["source_range"]["duration"]["ticks"], 48, "{item}");
}
