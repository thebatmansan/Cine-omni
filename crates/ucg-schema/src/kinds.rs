//! Canonical node kinds. Strings, not an enum, so modules can extend them.

// ---- project ----
pub const PROJECT: &str = "project";
pub const ASSET_MEDIA: &str = "asset.media";
pub const ASSET_MODEL: &str = "asset.model";

// ---- script ----
pub const SCRIPT_DOCUMENT: &str = "script.document";
pub const SCRIPT_TITLE_PAGE: &str = "script.title_page";
pub const SCRIPT_SCENE: &str = "script.scene";
pub const SCRIPT_ACTION: &str = "script.action";
pub const SCRIPT_DIALOGUE: &str = "script.dialogue";
pub const SCRIPT_PARENTHETICAL: &str = "script.parenthetical";
pub const SCRIPT_TRANSITION: &str = "script.transition";
pub const SCRIPT_CENTERED: &str = "script.centered";
pub const SCRIPT_SECTION: &str = "script.section";
pub const SCRIPT_SYNOPSIS: &str = "script.synopsis";
pub const SCRIPT_NOTE: &str = "script.note";
pub const SCRIPT_LYRIC: &str = "script.lyric";
pub const SCRIPT_PAGE_BREAK: &str = "script.page_break";
/// Cast member entity (one per character name).
pub const SCRIPT_CHARACTER: &str = "script.character";
pub const SCRIPT_LOCATION: &str = "script.location";
pub const SCRIPT_PROP: &str = "script.prop";
/// Word → time alignment attached to a dialogue line.
pub const SYNC_ALIGNMENT: &str = "sync.alignment";

// ---- storyboard (2D) ----
pub const BOARD_FRAME: &str = "board.frame";
pub const BOARD_LAYER: &str = "board.layer";
pub const BOARD_STROKE: &str = "board.stroke";

// ---- previs (3D) ----
pub const PREVIS_STAGE: &str = "previs.stage";
pub const PREVIS_SCENE: &str = "previs.scene";
pub const PREVIS_PRIM: &str = "previs.prim";
pub const PREVIS_CAMERA: &str = "previs.camera";
pub const PREVIS_LIGHT: &str = "previs.light";
pub const PREVIS_RIG: &str = "previs.rig";
pub const PREVIS_SHOT: &str = "previs.shot";
pub const PREVIS_STROKE: &str = "previs.stroke";
pub const PREVIS_CHARACTER: &str = "previs.character";
pub const PREVIS_DIALOGUE_TRACK: &str = "previs.dialogue_track";

// ---- logistics ----
pub const LOGISTICS_BREAKDOWN: &str = "logistics.breakdown";
pub const LOGISTICS_SHOT_LIST: &str = "logistics.shot_list";
pub const LOGISTICS_SHOT: &str = "logistics.shot";
pub const LOGISTICS_SCHEDULE: &str = "logistics.schedule";
pub const LOGISTICS_DAY: &str = "logistics.day";
pub const LOGISTICS_CALL_SHEET: &str = "logistics.call_sheet";
pub const LOGISTICS_LENS_GRID: &str = "logistics.lens_grid";
pub const LOGISTICS_TAKE: &str = "logistics.take";
/// What was actually shot, owned by nothing that was planned. Takes live here
/// and *reference* the shots they cover.
pub const LOGISTICS_PRODUCTION_LOG: &str = "logistics.production_log";
pub const LOGISTICS_CREW: &str = "logistics.crew";
pub const LOGISTICS_CAST: &str = "logistics.cast";

// ---- editorial ----
pub const NLE_TIMELINE: &str = "nle.timeline";
pub const NLE_STACK: &str = "nle.stack";
pub const NLE_TRACK: &str = "nle.track";
pub const NLE_CLIP: &str = "nle.clip";
pub const NLE_GAP: &str = "nle.gap";
pub const NLE_TRANSITION: &str = "nle.transition";
pub const NLE_MARKER: &str = "nle.marker";

// ---- compositing ----
pub const COMP_GRAPH: &str = "comp.graph";
pub const COMP_NODE: &str = "comp.node";

// ---- color ----
pub const COLOR_CONFIG: &str = "color.config";
pub const COLOR_SPACE: &str = "color.space";
pub const COLOR_GRADE: &str = "color.grade";
pub const COLOR_LUT: &str = "color.lut";

// ---- audio ----
pub const AUDIO_SESSION: &str = "audio.session";
pub const AUDIO_TRACK: &str = "audio.track";
pub const AUDIO_CLIP: &str = "audio.clip";
pub const AUDIO_BUS: &str = "audio.bus";
pub const AUDIO_STEM: &str = "audio.stem";

/// Script element kinds in document order (used by parsers/writers).
pub const SCRIPT_ELEMENT_KINDS: &[&str] = &[
    SCRIPT_SCENE,
    SCRIPT_ACTION,
    SCRIPT_DIALOGUE,
    SCRIPT_PARENTHETICAL,
    SCRIPT_TRANSITION,
    SCRIPT_CENTERED,
    SCRIPT_SECTION,
    SCRIPT_SYNOPSIS,
    SCRIPT_NOTE,
    SCRIPT_LYRIC,
    SCRIPT_PAGE_BREAK,
];

pub fn is_script_element(kind: &str) -> bool {
    SCRIPT_ELEMENT_KINDS.contains(&kind)
}
