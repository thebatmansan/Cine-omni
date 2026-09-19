//! Captions linked to the dialogue they transcribe, through the project.
//!
//! Real screenplay, real cut, real reactors: a line is rewritten through the
//! same `rewrite_fountain` the script editor uses, and what is asserted is what
//! the reactor chain did to the captions — which were flagged, why, and which
//! were not.

use cine_pipeline::project::{OmniProject, ProjectSetup};
use cine_timeline::caption_link::{self, LINE_TEXT, REVIEW};
use cine_timeline::captions::{Caption, CaptionTrack};
use cine_timeline::graph::{
    bind_clip_to_scene, caption_node, create_sequence, export_captions, place_material,
    sibling_tracks, write_captions, ClipSpec,
};
use ucg_core::prelude::*;
use ucg_core::Rate;
use ucg_schema::{attrs, kinds};

const SCRIPT: &str = "Title: Captions

INT. HARBOUR OFFICE - NIGHT #1#

MAREN crosses the room.

MAREN
We ride at dawn.

TOBIAS
Not before.

EXT. QUAY - DAWN #2#

The boats are gone.

MAREN
They left without us.
";

fn f(n: i64) -> RationalTime {
    RationalTime::exact(n, Rate::whole(24).unwrap())
}

fn aborted<E: std::fmt::Display>(e: E) -> UcgError {
    UcgError::Aborted(e.to_string())
}

struct Fixture {
    project: OmniProject,
    doc: EntityId,
    timeline: EntityId,
}

/// Scene 1 is under frames 0..96 of the cut and scene 2 under 96..192. Four
/// captions: scene 1's two lines, scene 2's line, and a caption over scene 2
/// that happens to read exactly like a scene 1 line.
fn fixture() -> Fixture {
    let project = OmniProject::new(ProjectSetup::default()).unwrap();
    let doc = project
        .import_fountain(SCRIPT, "captions.fountain")
        .unwrap();
    let scenes = project.scenes(doc);
    assert_eq!(scenes.len(), 2);
    let asset = EntityId::from_key("media:file:/footage/A001.mov");
    let mut timeline = EntityId::nil();
    project
        .engine
        .transact("set up the cut", |tx| {
            tx.ensure(asset, kinds::ASSET_MEDIA)?;
            tx.set(asset, attrs::MEDIA_PATH, "/footage/A001.mov")?;
            tx.set(asset, attrs::AVAILABLE_RANGE, TimeRange::new(f(0), f(480)))?;
            timeline = create_sequence(tx, project.root, "Cut", Rate::whole(24).unwrap(), None)
                .map_err(aborted)?;
            let stack = tx.graph().children(timeline)[0];
            let first = tx
                .graph()
                .children(stack)
                .into_iter()
                .find(|c| tx.graph().node(*c).unwrap().kind.as_str() == kinds::NLE_TRACK)
                .unwrap();
            let video = sibling_tracks(tx.graph(), first).map_err(aborted)?[0];
            for (at, scene) in [(0, scenes[0]), (96, scenes[1])] {
                let placed = place_material(
                    tx,
                    &[video],
                    f(at),
                    &ClipSpec {
                        media: asset,
                        source_range: TimeRange::new(f(at), f(96)),
                        name: None,
                    },
                    false,
                )
                .map_err(aborted)?;
                bind_clip_to_scene(tx, placed[0], scene).map_err(aborted)?;
            }
            let mut track = CaptionTrack::new("en", Rate::whole(24).unwrap());
            for (start, text) in [
                (0, "We ride at dawn."),
                (48, "Not before."),
                (96, "They left without us."),
                (144, "We ride at dawn."),
            ] {
                track
                    .captions
                    .push(Caption::new(TimeRange::new(f(start), f(48)), text));
            }
            write_captions(tx, timeline, &track).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    Fixture {
        project,
        doc,
        timeline,
    }
}

impl Fixture {
    fn caption(&self, i: usize) -> EntityId {
        let g = self.project.engine.read();
        let track = export_captions(&g, self.timeline, "en").unwrap().unwrap();
        caption_node(self.timeline, "en", track.captions[i].key.as_ref().unwrap())
    }

    fn link_exact(&self) -> usize {
        let mut n = 0;
        self.project
            .engine
            .transact("link captions", |tx| {
                n = caption_link::link_exact(tx, self.timeline, "en").map_err(aborted)?;
                Ok(())
            })
            .unwrap();
        n
    }

    fn rewrite(&self, text: &str) {
        self.project
            .rewrite_fountain(self.doc, text, false)
            .expect("rewrite");
    }

    fn review(&self, i: usize) -> Option<String> {
        let c = self.caption(i);
        self.project
            .engine
            .read()
            .node(c)
            .and_then(|n| n.get_str(REVIEW))
            .map(str::to_string)
    }

    fn line_of(&self, i: usize) -> Vec<EntityId> {
        let c = self.caption(i);
        self.project.engine.read().targets(c, EdgeKind::DerivedFrom)
    }
}

/// Only exact, unambiguous matches are linked automatically — and only against
/// the scenes actually under each caption, which is the relationship the
/// project holds, not a search of the whole script.
#[test]
fn exact_lines_under_a_caption_are_linked_and_nothing_else_is() {
    let fx = fixture();
    assert_eq!(fx.link_exact(), 3);
    assert_eq!(fx.line_of(0).len(), 1, "MAREN's scene 1 line");
    assert_eq!(fx.line_of(1).len(), 1, "TOBIAS's line");
    assert_eq!(fx.line_of(2).len(), 1, "MAREN's scene 2 line");
    assert!(
        fx.line_of(3).is_empty(),
        "reads like a scene 1 line but sits over scene 2, so it is not that line"
    );
    // Both directions: the line knows its caption.
    let line = fx.line_of(0)[0];
    assert_eq!(
        caption_link::captions_of_line(&fx.project.engine.read(), line),
        vec![fx.caption(0)]
    );
    for i in 0..4 {
        assert_eq!(fx.review(i), None);
    }
}

/// A rewrite flags the caption of that line, says what changed, and flags
/// nothing else. Undo takes the flag back with the rewrite.
#[test]
fn rewriting_a_line_flags_its_caption_and_only_its_caption() {
    let fx = fixture();
    fx.link_exact();
    fx.rewrite(&SCRIPT.replace("We ride at dawn.", "We ride at first light."));

    let why = fx.review(0).expect("MAREN's caption is flagged");
    assert!(why.contains("MAREN's line"), "{why}");
    assert!(why.contains("\"We ride at dawn.\""), "the old words: {why}");
    assert!(
        why.contains("\"We ride at first light.\""),
        "the new words: {why}"
    );
    assert!(why.contains("HARBOUR OFFICE"), "and where: {why}");
    assert_eq!(fx.review(1), None, "TOBIAS's line did not change");
    assert_eq!(fx.review(2), None, "scene 2 did not change");

    fx.project.engine.undo().unwrap();
    assert_eq!(
        fx.review(0),
        None,
        "undo takes the flag back with the rewrite"
    );
}

/// Reverting the words clears the flag without anybody accepting anything,
/// because the caption transcribes exactly what the line says again.
#[test]
fn a_rewrite_reverted_by_hand_clears_itself() {
    let fx = fixture();
    fx.link_exact();
    fx.rewrite(&SCRIPT.replace("We ride at dawn.", "We ride at first light."));
    assert!(fx.review(0).is_some());
    fx.rewrite(SCRIPT);
    assert_eq!(fx.review(0), None);
}

/// Inserting an action line above a speech gives the speech a new id. The
/// caption follows it — without a flag, because nothing it says changed.
#[test]
fn a_line_that_only_moved_is_followed_not_flagged() {
    let fx = fixture();
    fx.link_exact();
    let before = fx.line_of(0);
    fx.rewrite(&SCRIPT.replace(
        "MAREN crosses the room.\n",
        "MAREN crosses the room.\n\nThe lamp gutters.\n",
    ));
    let after = fx.line_of(0);
    assert_ne!(after, before, "the speech is a different element now");
    assert_eq!(after.len(), 1);
    let g = fx.project.engine.read();
    assert_eq!(
        g.node(after[0]).unwrap().get_str(attrs::TEXT),
        Some("We ride at dawn."),
        "and the link points at the speech, not at whatever took its old place"
    );
    drop(g);
    assert_eq!(fx.review(0), None, "moving a line is not a change to it");
    assert_eq!(fx.review(1), None);
}

#[test]
fn a_cut_line_flags_its_caption_as_cut() {
    let fx = fixture();
    fx.link_exact();
    fx.rewrite(&SCRIPT.replace("TOBIAS\nNot before.\n\n", ""));
    let why = fx.review(1).expect("TOBIAS's caption is flagged");
    assert!(why.contains("\"Not before.\" is no longer in"), "{why}");
    assert!(fx.line_of(1).is_empty(), "and it points at nothing");
    assert_eq!(fx.review(0), None, "MAREN's line is still there");
}

/// Accepting takes the line's new words as the transcription.
#[test]
fn accepting_a_rewrite_takes_the_new_words_and_clears_the_flag() {
    let fx = fixture();
    fx.link_exact();
    fx.rewrite(&SCRIPT.replace("We ride at dawn.", "We ride at first light."));
    let c = fx.caption(0);
    fx.project
        .engine
        .transact("accept", |tx| caption_link::accept(tx, c).map_err(aborted))
        .unwrap();
    assert_eq!(fx.review(0), None);
    assert_eq!(
        fx.project.engine.read().node(c).unwrap().get_str(LINE_TEXT),
        Some("We ride at first light.")
    );

    // A cut line has nothing to accept.
    fx.rewrite(
        &SCRIPT
            .replace("TOBIAS\nNot before.\n\n", "")
            .replace("We ride at dawn.", "We ride at first light."),
    );
    let t = fx.caption(1);
    let e = fx
        .project
        .engine
        .transact("accept", |tx| caption_link::accept(tx, t).map_err(aborted))
        .expect_err("nothing to accept");
    assert!(e.to_string().contains("unlink the caption"), "{e}");
}

/// A re-cut can move a caption's picture out from under it. Moving the caption
/// over the other scene flags it even though the script did not change.
#[test]
fn a_caption_no_longer_over_its_scene_is_flagged() {
    let fx = fixture();
    fx.link_exact();
    let timeline = fx.timeline;
    fx.project
        .engine
        .transact("move caption", |tx| {
            let mut track = export_captions(tx.graph(), timeline, "en")
                .map_err(aborted)?
                .unwrap();
            track.captions[1].record = TimeRange::new(f(100), f(40));
            write_captions(tx, timeline, &track).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    // Order is time order: the moved caption is now the third.
    let g = fx.project.engine.read();
    let track = export_captions(&g, timeline, "en").unwrap().unwrap();
    let moved = track
        .captions
        .iter()
        .position(|c| c.lines == vec!["Not before."])
        .unwrap();
    drop(g);
    let why = fx.review(moved).expect("flagged");
    assert!(
        why.contains("no longer sits over INT. HARBOUR OFFICE"),
        "{why}"
    );
}

/// Once everything is resolved, another pass of the reactors writes nothing:
/// the caption reactor converges.
#[test]
fn the_caption_reactor_converges() {
    let fx = fixture();
    fx.link_exact();
    fx.rewrite(&SCRIPT.replace("We ride at dawn.", "We ride at first light."));
    let g = fx.project.engine.read();
    for (caption, standing) in caption_link::review_all(&g) {
        assert!(
            caption_link::agrees(&g, caption, &standing),
            "{caption} is not where the reactor left it"
        );
    }
}

/// What resolving every linked caption costs, at feature scale.
///
/// The caption reactor re-resolves every linked caption on any script or cut
/// change, so its cost is the thing to know before calling it interactive.
///
/// ```text
/// cargo test -p cine-pipeline --test caption_script_link -- --ignored --nocapture
/// ```
#[test]
#[ignore]
fn measure_caption_review_at_feature_scale() {
    use std::time::Instant;
    const SCENES: usize = 120;
    const LINES: usize = 10;
    let mut script = String::from("Title: Scale\n\n");
    for s in 0..SCENES {
        script.push_str(&format!(
            "INT. ROOM {s} - NIGHT #{}#\n\nMAREN waits.\n\n",
            s + 1
        ));
        for l in 0..LINES {
            let who = if l % 2 == 0 { "MAREN" } else { "ELIAS" };
            script.push_str(&format!("{who}\nLine {l} of scene {s}, said once.\n\n"));
        }
    }
    let project = OmniProject::new(ProjectSetup::default()).unwrap();
    let doc = project.import_fountain(&script, "scale.fountain").unwrap();
    let scenes = project.scenes(doc);
    let asset = EntityId::from_key("media:file:/footage/scale.mov");
    let mut timeline = EntityId::nil();
    let per_scene = 240i64;
    let started = Instant::now();
    project
        .engine
        .transact("build the cut", |tx| {
            tx.ensure(asset, kinds::ASSET_MEDIA)?;
            tx.set(asset, attrs::MEDIA_PATH, "/footage/scale.mov")?;
            tx.set(
                asset,
                attrs::AVAILABLE_RANGE,
                TimeRange::new(f(0), f(per_scene * SCENES as i64)),
            )?;
            timeline = create_sequence(tx, project.root, "Cut", Rate::whole(24).unwrap(), None)
                .map_err(aborted)?;
            let stack = tx.graph().children(timeline)[0];
            let first = tx
                .graph()
                .children(stack)
                .into_iter()
                .find(|c| tx.graph().node(*c).unwrap().kind.as_str() == kinds::NLE_TRACK)
                .unwrap();
            let video = sibling_tracks(tx.graph(), first).map_err(aborted)?[0];
            let mut track = CaptionTrack::new("en", Rate::whole(24).unwrap());
            for (i, scene) in scenes.iter().enumerate() {
                let at = per_scene * i as i64;
                let placed = place_material(
                    tx,
                    &[video],
                    f(at),
                    &ClipSpec {
                        media: asset,
                        source_range: TimeRange::new(f(at), f(per_scene)),
                        name: None,
                    },
                    false,
                )
                .map_err(aborted)?;
                bind_clip_to_scene(tx, placed[0], *scene).map_err(aborted)?;
                for l in 0..LINES {
                    track.captions.push(Caption::new(
                        TimeRange::new(f(at + 24 * l as i64), f(24)),
                        &format!("Line {l} of scene {i}, said once."),
                    ));
                }
            }
            write_captions(tx, timeline, &track).map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    let built = started.elapsed();

    let t = Instant::now();
    let mut linked = 0;
    project
        .engine
        .transact("link", |tx| {
            linked = caption_link::link_exact(tx, timeline, "en").map_err(aborted)?;
            Ok(())
        })
        .unwrap();
    let link = t.elapsed();
    assert_eq!(linked, SCENES * LINES);

    let t = Instant::now();
    let g = project.engine.read();
    let reviewed = caption_link::review_all(&g);
    let review = t.elapsed();
    drop(g);
    assert_eq!(reviewed.len(), SCENES * LINES);

    let rewritten = script.replace(
        "Line 3 of scene 7, said once.",
        "Line 3 of scene 7, said twice.",
    );
    let t = Instant::now();
    project.rewrite_fountain(doc, &rewritten, false).unwrap();
    let rewrite = t.elapsed();
    let g = project.engine.read();
    let flagged = g
        .of_kind(kinds::NLE_CAPTION)
        .iter()
        .filter(|n| n.get_str(REVIEW).is_some())
        .count();
    assert_eq!(flagged, 1, "one line changed, one caption flagged");
    println!(
        "MEASURED captions at feature scale: {} scenes, {} linked captions, {} clips\n\
         \x20        build {:.1?}; link_exact {:.1?}; review_all (the reactor's work) {:.1?}; \
         rewrite of one line incl. every reactor {:.1?}; flagged {}",
        SCENES, linked, SCENES, built, link, review, rewrite, flagged
    );
}
