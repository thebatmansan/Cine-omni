//! Breakdown sheets: one [`SceneBreakdown`] per script scene, listing
//! everything production needs to schedule and shoot it.
//!
//! Reads `script.scene` nodes (heading, cast, props, page eighths, estimated
//! duration) and flags departments (vehicles, animals, SFX, VFX) from keyword
//! matches in the props list and the heading.

use serde::{Deserialize, Serialize};
use ucg_core::{EntityId, Graph, Node, Value};
use ucg_schema::{attrs, kinds, Component, IntExt, SceneHeading};

/// Everything a scene needs, as read from the script.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SceneBreakdown {
    /// The `script.scene` entity.
    pub scene: EntityId,
    /// Scene number as printed in the script (`"12"`, `"12A"`).
    pub number: String,
    /// Raw slug line.
    pub heading: String,
    pub int_ext: IntExt,
    pub location: String,
    pub time_of_day: String,
    /// Speaking / named characters present, in script order.
    pub cast: Vec<String>,
    pub props: Vec<String>,
    /// Hint that background extras are needed (keyword-based), if any.
    pub extras_hint: Option<String>,
    /// Script length in eighths of a page.
    pub page_eighths: u32,
    /// Estimated screen time in minutes.
    pub est_minutes: f64,
    pub vehicles: bool,
    pub animals: bool,
    pub sfx: bool,
    pub vfx: bool,
    /// What a person tagged by hand, as they tagged it.
    ///
    /// The lists above are the merged result — what the department actually
    /// needs — and this is how it got there, so an interface can show which
    /// elements came out of the script and which a first assistant added.
    #[serde(default)]
    pub tags: SceneTags,
}

/// A person's corrections to what the script implies.
///
/// Breakdown elements are derived: the script says a prop, or a keyword in the
/// heading suggests a vehicle. Derivation is re-run whenever the script
/// changes, so a correction cannot live in the derived data — it lives here,
/// on the scene, and is applied every time the breakdown is read. A rewrite
/// therefore never silently loses what somebody tagged.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SceneTags {
    /// Elements a person added that the script does not mention.
    #[serde(default)]
    pub props_added: Vec<String>,
    /// Script-derived elements a person says do not belong.
    #[serde(default)]
    pub props_removed: Vec<String>,
    #[serde(default)]
    pub cast_added: Vec<String>,
    #[serde(default)]
    pub cast_removed: Vec<String>,
    /// `None` leaves the keyword search to decide; `Some` overrules it.
    #[serde(default)]
    pub vehicles: Option<bool>,
    #[serde(default)]
    pub animals: Option<bool>,
    #[serde(default)]
    pub sfx: Option<bool>,
    #[serde(default)]
    pub vfx: Option<bool>,
}

impl SceneTags {
    /// Whether a person has touched this scene's breakdown at all.
    pub fn is_empty(&self) -> bool {
        self == &SceneTags::default()
    }

    /// Apply an addition or a removal to one list.
    fn set(added: &mut Vec<String>, removed: &mut Vec<String>, value: &str, present: bool) {
        let same = |a: &String| a.eq_ignore_ascii_case(value);
        added.retain(|a| !same(a));
        removed.retain(|a| !same(a));
        if present {
            added.push(value.to_string());
        } else {
            removed.push(value.to_string());
        }
    }

    /// Tag or untag an element. `category` is `prop`, `cast`, `vehicles`,
    /// `animals`, `sfx` or `vfx`; the flags take no value.
    pub fn tag(&mut self, category: &str, value: &str, present: bool) -> Result<(), String> {
        match category {
            "prop" => Self::set(
                &mut self.props_added,
                &mut self.props_removed,
                value,
                present,
            ),
            "cast" => Self::set(&mut self.cast_added, &mut self.cast_removed, value, present),
            "vehicles" => self.vehicles = Some(present),
            "animals" => self.animals = Some(present),
            "sfx" => self.sfx = Some(present),
            "vfx" => self.vfx = Some(present),
            other => {
                return Err(format!(
                    "`{other}` is not a breakdown category; there are prop, cast, vehicles, \
                     animals, sfx and vfx"
                ))
            }
        }
        Ok(())
    }

    /// Hand a category back to the script.
    pub fn clear(&mut self, category: &str) {
        match category {
            "vehicles" => self.vehicles = None,
            "animals" => self.animals = None,
            "sfx" => self.sfx = None,
            "vfx" => self.vfx = None,
            _ => {}
        }
    }

    fn apply(&self, list: &mut Vec<String>, added: &[String], removed: &[String]) {
        list.retain(|a| !removed.iter().any(|r| r.eq_ignore_ascii_case(a)));
        for value in added {
            if !list.iter().any(|a| a.eq_ignore_ascii_case(value)) {
                list.push(value.clone());
            }
        }
    }
}

impl SceneBreakdown {
    /// An empty breakdown carrying only the scene id (used when the scene is
    /// missing or has no attributes yet).
    pub fn empty(scene: EntityId) -> Self {
        SceneBreakdown {
            scene,
            number: String::new(),
            heading: String::new(),
            int_ext: IntExt::Other,
            location: String::new(),
            time_of_day: String::new(),
            cast: Vec::new(),
            props: Vec::new(),
            extras_hint: None,
            page_eighths: 0,
            est_minutes: 0.0,
            vehicles: false,
            animals: false,
            sfx: false,
            vfx: false,
            tags: SceneTags::default(),
        }
    }

    /// Night / dusk / evening scenes (drives night-shoot grouping).
    pub fn is_night(&self) -> bool {
        is_night_time(&self.time_of_day)
    }

    /// Page count as an industry string: `"1 3/8"`, `"4/8"`, `"2"`.
    pub fn pages_str(&self) -> String {
        eighths_str(self.page_eighths)
    }

    /// Department flags as short labels (`VEH`, `ANI`, `SFX`, `VFX`).
    pub fn flags(&self) -> Vec<&'static str> {
        let mut v = Vec::new();
        if self.vehicles {
            v.push("VEH");
        }
        if self.animals {
            v.push("ANI");
        }
        if self.sfx {
            v.push("SFX");
        }
        if self.vfx {
            v.push("VFX");
        }
        v
    }
}

/// Format eighths of a page the way a stripboard does (`4/8` is never reduced).
pub fn eighths_str(eighths: u32) -> String {
    let whole = eighths / 8;
    let rem = eighths % 8;
    match (whole, rem) {
        (0, 0) => "0".to_string(),
        (w, 0) => w.to_string(),
        (0, r) => format!("{r}/8"),
        (w, r) => format!("{w} {r}/8"),
    }
}

/// Whether a time-of-day string describes a night shoot.
pub fn is_night_time(time_of_day: &str) -> bool {
    let t = time_of_day.to_ascii_uppercase();
    ["NIGHT", "DUSK", "EVENING", "MIDNIGHT", "PRE-DAWN"]
        .iter()
        .any(|k| t.contains(k))
}

const VEHICLE_KEYWORDS: &[&str] = &[
    "car",
    "cars",
    "truck",
    "van",
    "bus",
    "motorcycle",
    "motorbike",
    "bike",
    "bicycle",
    "taxi",
    "cab",
    "jeep",
    "helicopter",
    "boat",
    "ship",
    "train",
    "limo",
    "limousine",
    "ambulance",
    "tractor",
    "vehicle",
    "sedan",
    "suv",
    "pickup",
    "convertible",
    "police car",
    "squad car",
    "scooter",
    "tram",
    "subway",
];
const ANIMAL_KEYWORDS: &[&str] = &[
    "dog", "dogs", "puppy", "cat", "kitten", "horse", "horses", "bird", "snake", "cow", "cattle",
    "sheep", "goat", "wolf", "animal", "animals", "pigeon", "rat", "mouse", "chicken", "pig",
    "fish", "deer", "lion", "tiger", "parrot", "rabbit", "spider",
];
const SFX_KEYWORDS: &[&str] = &[
    "explosion",
    "explosive",
    "fire",
    "flames",
    "smoke",
    "rain",
    "gunshot",
    "gun",
    "pistol",
    "rifle",
    "blood",
    "squib",
    "breakaway",
    "wind",
    "fog",
    "storm",
    "snow",
    "crash",
    "sparks",
    "pyro",
    "charge",
    "bullet",
    "shatter",
    "steam",
];
const VFX_KEYWORDS: &[&str] = &[
    "green screen",
    "greenscreen",
    "blue screen",
    "bluescreen",
    "cg",
    "cgi",
    "vfx",
    "hologram",
    "spaceship",
    "dragon",
    "alien",
    "portal",
    "wire removal",
    "composite",
    "monster",
    "laser",
    "robot",
    "matte",
    "set extension",
    "creature",
    "digital double",
];
const EXTRAS_KEYWORDS: &[&str] = &[
    "crowd",
    "extras",
    "party",
    "audience",
    "soldiers",
    "patrons",
    "pedestrians",
    "guests",
    "protesters",
    "passengers",
    "diners",
    "commuters",
    "spectators",
    "students",
    "congregation",
    "army",
    "mob",
];

/// Word-boundary keyword test over a lower-cased haystack.
fn has_keyword(haystack: &str, keywords: &[&'static str]) -> Option<&'static str> {
    let words: Vec<&str> = haystack
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .collect();
    keywords.iter().copied().find(|kw| {
        if kw.contains(' ') {
            haystack.contains(kw)
        } else {
            words.iter().any(|w| w == kw)
        }
    })
}

fn str_list(node: &Node, key: &str) -> Vec<String> {
    node.get(key)
        .and_then(Value::as_list)
        .map(|l| {
            l.iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Build the breakdown for one `script.scene`. A missing node yields
/// [`SceneBreakdown::empty`] so callers can treat the function as total.
pub fn breakdown_scene(graph: &Graph, scene: EntityId) -> SceneBreakdown {
    let Some(node) = graph.node(scene) else {
        return SceneBreakdown::empty(scene);
    };
    let heading_raw = node.get_str(attrs::HEADING).unwrap_or("").to_string();
    let parsed = SceneHeading::read(node).unwrap_or_else(|| parse_heading(&heading_raw));
    let heading = if heading_raw.is_empty() {
        parsed.raw.clone()
    } else {
        heading_raw
    };
    let number = node
        .get_str(attrs::SCENE_NUMBER)
        .map(str::to_string)
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            node.get_int(attrs::ORDER)
                .map(|o| (o + 1).to_string())
                .unwrap_or_default()
        });
    let mut cast = str_list(node, attrs::CHARACTERS);
    dedup_keep_order(&mut cast);
    let mut props = str_list(node, attrs::PROPS);
    dedup_keep_order(&mut props);
    let page_eighths = node
        .get_int(attrs::PAGE_EIGHTHS)
        .map(|e| e.max(0) as u32)
        .unwrap_or(0);
    let est_minutes = node
        .get_float(attrs::EST_DURATION_S)
        .map(|s| s.max(0.0) / 60.0)
        .unwrap_or(0.0);

    let hay = {
        let mut s = heading.to_ascii_lowercase();
        for p in &props {
            s.push(' ');
            s.push_str(&p.to_ascii_lowercase());
        }
        s
    };
    let extras_hint = has_keyword(&hay, EXTRAS_KEYWORDS).map(|k| format!("background: {k}"));

    let location = if parsed.location.is_empty() {
        str_list(node, attrs::LOCATIONS)
            .into_iter()
            .next()
            .unwrap_or_default()
    } else {
        parsed.location.clone()
    };

    // What a person tagged, applied over what the script implies.
    let tags: SceneTags = node
        .get(ATTR_BREAKDOWN_TAGS)
        .and_then(|v| v.to_serde().ok())
        .unwrap_or_default();
    tags.apply(&mut props, &tags.props_added, &tags.props_removed);
    tags.apply(&mut cast, &tags.cast_added, &tags.cast_removed);

    SceneBreakdown {
        scene,
        number,
        heading,
        int_ext: parsed.int_ext,
        location,
        time_of_day: parsed.time_of_day,
        cast,
        props,
        extras_hint,
        page_eighths,
        est_minutes,
        vehicles: tags
            .vehicles
            .unwrap_or_else(|| has_keyword(&hay, VEHICLE_KEYWORDS).is_some()),
        animals: tags
            .animals
            .unwrap_or_else(|| has_keyword(&hay, ANIMAL_KEYWORDS).is_some()),
        sfx: tags
            .sfx
            .unwrap_or_else(|| has_keyword(&hay, SFX_KEYWORDS).is_some()),
        vfx: tags
            .vfx
            .unwrap_or_else(|| has_keyword(&hay, VFX_KEYWORDS).is_some()),
        tags,
    }
}

/// Attribute on a `script.scene` holding [`SceneTags`].
pub const ATTR_BREAKDOWN_TAGS: &str = "breakdown_tags";

/// Breakdowns for every `script.scene` contained in `document`, in `order`.
pub fn breakdown_document(graph: &Graph, document: EntityId) -> Vec<SceneBreakdown> {
    let mut scenes: Vec<(i64, usize, EntityId)> = graph
        .children(document)
        .into_iter()
        .enumerate()
        .filter(|(_, id)| {
            graph
                .node(*id)
                .map(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                .unwrap_or(false)
        })
        .map(|(i, id)| {
            (
                graph
                    .attr(id, attrs::ORDER)
                    .and_then(Value::as_int)
                    .unwrap_or(i as i64),
                i,
                id,
            )
        })
        .collect();
    scenes.sort();
    scenes
        .into_iter()
        .map(|(_, _, id)| breakdown_scene(graph, id))
        .collect()
}

/// Minimal slug-line parser used when a scene has no [`SceneHeading`] component.
pub fn parse_heading(raw: &str) -> SceneHeading {
    let up = raw.trim().to_ascii_uppercase();
    let (int_ext, rest) = if let Some(r) = up
        .strip_prefix("INT./EXT.")
        .or_else(|| up.strip_prefix("INT/EXT"))
    {
        (IntExt::IntExt, r)
    } else if let Some(r) = up.strip_prefix("I/E.").or_else(|| up.strip_prefix("I/E")) {
        (IntExt::IntExt, r)
    } else if let Some(r) = up.strip_prefix("INT.").or_else(|| up.strip_prefix("INT ")) {
        (IntExt::Int, r)
    } else if let Some(r) = up.strip_prefix("EXT.").or_else(|| up.strip_prefix("EXT ")) {
        (IntExt::Ext, r)
    } else {
        (IntExt::Other, up.as_str())
    };
    let rest = rest.trim();
    let (location, time_of_day) = match rest.rsplit_once(" - ") {
        Some((l, t)) => (l.trim().to_string(), t.trim().to_string()),
        None => (rest.to_string(), String::new()),
    };
    SceneHeading {
        int_ext,
        location,
        time_of_day,
        raw: raw.trim().to_string(),
    }
}

fn dedup_keep_order(v: &mut Vec<String>) {
    let mut seen = std::collections::HashSet::new();
    v.retain(|s| seen.insert(s.clone()));
}

fn int_ext_str(ie: IntExt) -> &'static str {
    match ie {
        IntExt::Int => "INT",
        IntExt::Ext => "EXT",
        IntExt::IntExt => "INT/EXT",
        IntExt::Other => "",
    }
}

/// Markdown table of breakdown sheets.
pub fn to_markdown(sheets: &[SceneBreakdown]) -> String {
    let mut out = String::from("| Sc | I/E | Location | D/N | Pages | Cast | Props | Flags |\n|---|---|---|---|---|---|---|---|\n");
    for s in sheets {
        let mut flags = s.flags().join(" ");
        if let Some(h) = &s.extras_hint {
            if !flags.is_empty() {
                flags.push(' ');
            }
            flags.push_str(h);
        }
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            s.number,
            int_ext_str(s.int_ext),
            s.location,
            s.time_of_day,
            s.pages_str(),
            s.cast.join(", "),
            s.props.join(", "),
            flags
        ));
    }
    let total: u32 = sheets.iter().map(|s| s.page_eighths).sum();
    out.push_str(&format!(
        "\nTotal: {} scenes, {} pages\n",
        sheets.len(),
        eighths_str(total)
    ));
    out
}

/// Quote a CSV field when needed.
pub(crate) fn csv_field(s: &str) -> String {
    if s.contains([',', '"', '\n']) {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

/// CSV export (one row per scene, header included).
pub fn to_csv(sheets: &[SceneBreakdown]) -> String {
    let mut out = String::from(
        "scene,number,int_ext,location,time_of_day,page_eighths,est_minutes,cast,props,extras,vehicles,animals,sfx,vfx\n",
    );
    for s in sheets {
        let row = [
            s.scene.to_string(),
            s.number.clone(),
            int_ext_str(s.int_ext).to_string(),
            s.location.clone(),
            s.time_of_day.clone(),
            s.page_eighths.to_string(),
            format!("{:.2}", s.est_minutes),
            s.cast.join("; "),
            s.props.join("; "),
            s.extras_hint.clone().unwrap_or_default(),
            s.vehicles.to_string(),
            s.animals.to_string(),
            s.sfx.to_string(),
            s.vfx.to_string(),
        ];
        out.push_str(
            &row.iter()
                .map(|f| csv_field(f))
                .collect::<Vec<_>>()
                .join(","),
        );
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eighths_formatting() {
        assert_eq!(eighths_str(0), "0");
        assert_eq!(eighths_str(4), "4/8");
        assert_eq!(eighths_str(8), "1");
        assert_eq!(eighths_str(11), "1 3/8");
    }

    #[test]
    fn heading_parser() {
        let h = parse_heading("INT. KITCHEN - NIGHT");
        assert_eq!(h.int_ext, IntExt::Int);
        assert_eq!(h.location, "KITCHEN");
        assert_eq!(h.time_of_day, "NIGHT");
        let h = parse_heading("EXT. STREET - CONTINUOUS");
        assert_eq!(h.int_ext, IntExt::Ext);
        let h = parse_heading("INT./EXT. CAR - DAY");
        assert_eq!(h.int_ext, IntExt::IntExt);
        assert_eq!(h.location, "CAR");
        let h = parse_heading("Somewhere");
        assert_eq!(h.int_ext, IntExt::Other);
        assert_eq!(h.time_of_day, "");
    }

    #[test]
    fn keyword_flags_use_word_boundaries() {
        assert!(has_keyword("a red car", VEHICLE_KEYWORDS).is_some());
        assert!(has_keyword("a carpet", VEHICLE_KEYWORDS).is_none());
        assert!(has_keyword("green screen", VFX_KEYWORDS).is_some());
        assert!(has_keyword("hotdog stand", ANIMAL_KEYWORDS).is_none());
        assert!(is_night_time("NIGHT"));
        assert!(is_night_time("Dusk"));
        assert!(!is_night_time("DAY"));
    }

    #[test]
    fn csv_quoting() {
        assert_eq!(csv_field("plain"), "plain");
        assert_eq!(csv_field("a,b"), "\"a,b\"");
        assert_eq!(csv_field("say \"hi\""), "\"say \"\"hi\"\"\"");
    }
}
