//! OpenColorIO v2 configs: a parser for the YAML subset used by ACES configs,
//! a compiler from transform definitions to [`Processor`]s, a built-in ACES
//! config ([`Config::aces_default`]) and a writer ([`Config::to_yaml`]).
//!
//! Reference spaces follow OCIO v2: scene colorspaces convert to/from the
//! scene reference (ACES2065-1 in the ACES config), display colorspaces
//! convert to/from the display reference (CIE XYZ D65, 1.0 = 100 cd/m²), and
//! view transforms bridge the two.

use crate::error::{Error, Result};
use crate::file_rules::{match_path, FileRule, FileRuleKind, RuleMatch};
use crate::lut::{parse_cube, Interpolation, Lut};
use crate::processor::{Direction, LogParams, NegativeStyle, Op, Processor};
use crate::space::{
    bradford_adaptation, rgb_to_rgb_matrix, rgb_to_rgb_matrix_cat02, Mat3, Primaries, WHITE_ACES,
    WHITE_D65,
};
use crate::transfer::Transfer;
use crate::yaml::{self, Yaml};
use indexmap::IndexMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use ucg_schema::Cdl;

/// A transform as written in a config (not yet compiled).
#[derive(Clone, Debug, PartialEq)]
pub enum TransformDef {
    Group(Vec<TransformDef>),
    Matrix {
        matrix: [f64; 16],
        offset: [f64; 4],
        direction: Direction,
    },
    File {
        src: String,
        interpolation: String,
        direction: Direction,
    },
    Exponent {
        value: [f64; 4],
        direction: Direction,
        negative: NegativeStyle,
    },
    ExponentWithLinear {
        gamma: [f64; 4],
        offset: [f64; 4],
        direction: Direction,
    },
    Log {
        base: f64,
        direction: Direction,
    },
    LogAffine {
        params: LogParams,
        direction: Direction,
    },
    LogCamera {
        params: LogParams,
        direction: Direction,
    },
    Cdl {
        cdl: Cdl,
        direction: Direction,
    },
    Range {
        min_in: Option<f64>,
        max_in: Option<f64>,
        min_out: Option<f64>,
        max_out: Option<f64>,
        clamp: bool,
        direction: Direction,
    },
    Builtin {
        style: String,
        direction: Direction,
    },
    ColorSpace {
        src: String,
        dst: String,
    },
    DisplayView {
        src: String,
        display: String,
        view: String,
        direction: Direction,
    },
}

/// A colorspace definition.
#[derive(Clone, Debug, PartialEq)]
pub struct ColorSpaceDef {
    pub name: String,
    pub family: String,
    pub aliases: Vec<String>,
    pub encoding: String,
    pub bitdepth: String,
    pub description: String,
    pub isdata: bool,
    /// Defined under `display_colorspaces:` (relative to the display reference).
    pub is_display: bool,
    pub to_reference: Option<TransformDef>,
    pub from_reference: Option<TransformDef>,
}

impl ColorSpaceDef {
    /// Minimal definition.
    pub fn new(name: &str) -> Self {
        ColorSpaceDef {
            name: name.into(),
            family: String::new(),
            aliases: Vec::new(),
            encoding: String::new(),
            bitdepth: "32f".into(),
            description: String::new(),
            isdata: false,
            is_display: false,
            to_reference: None,
            from_reference: None,
        }
    }

    /// True when `name` is the name or an alias (case-insensitive).
    pub fn matches(&self, name: &str) -> bool {
        self.name.eq_ignore_ascii_case(name)
            || self.aliases.iter().any(|a| a.eq_ignore_ascii_case(name))
    }
}

/// A view of a display.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct View {
    pub name: String,
    pub colorspace: Option<String>,
    pub view_transform: Option<String>,
    pub display_colorspace: Option<String>,
    pub looks: Option<String>,
}

/// A look.
#[derive(Clone, Debug, PartialEq)]
pub struct Look {
    pub name: String,
    pub process_space: String,
    pub transform: Option<TransformDef>,
    pub inverse_transform: Option<TransformDef>,
}

/// A view transform: scene reference → display reference, or — when it is
/// written with `from_display_reference` / `to_display_reference` — display
/// reference → display reference.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ViewTransformDef {
    pub name: String,
    pub description: String,
    pub from_scene_reference: Option<TransformDef>,
    pub to_scene_reference: Option<TransformDef>,
    pub from_display_reference: Option<TransformDef>,
    pub to_display_reference: Option<TransformDef>,
}

impl ViewTransformDef {
    /// Written against the display reference rather than the scene reference.
    pub fn is_display_referred(&self) -> bool {
        (self.from_display_reference.is_some() || self.to_display_reference.is_some())
            && self.from_scene_reference.is_none()
            && self.to_scene_reference.is_none()
    }
}

/// An OCIO config.
#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    pub version: String,
    pub name: String,
    pub description: String,
    pub search_path: Vec<String>,
    pub roles: IndexMap<String, String>,
    pub displays: IndexMap<String, Vec<View>>,
    pub active_displays: Vec<String>,
    pub active_views: Vec<String>,
    pub looks: Vec<Look>,
    pub view_transforms: Vec<ViewTransformDef>,
    pub default_view_transform: Option<String>,
    pub colorspaces: Vec<ColorSpaceDef>,
    /// Which colorspace a file is in, by its path ([`Config::colorspace_for_path`]).
    pub file_rules: Vec<FileRule>,
    /// Directory the config was loaded from (for `FileTransform` resolution).
    pub base_dir: Option<PathBuf>,
    /// Top-level keys present in the loaded text that this parser does not
    /// implement. A config loads without them; what they would have changed is
    /// not applied, and a caller showing the config must say so.
    pub unimplemented: Vec<String>,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            version: "2.1".into(),
            name: String::new(),
            description: String::new(),
            search_path: Vec::new(),
            roles: IndexMap::new(),
            displays: IndexMap::new(),
            active_displays: Vec::new(),
            active_views: Vec::new(),
            looks: Vec::new(),
            view_transforms: Vec::new(),
            default_view_transform: None,
            colorspaces: Vec::new(),
            file_rules: Vec::new(),
            base_dir: None,
            unimplemented: Vec::new(),
        }
    }
}

const MAX_DEPTH: usize = 16;

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

fn parse_direction(y: &Yaml) -> Direction {
    match y.get("direction").and_then(Yaml::as_str) {
        Some(d) if d.eq_ignore_ascii_case("inverse") => Direction::Inverse,
        _ => Direction::Forward,
    }
}

fn floats_n<const N: usize>(y: Option<&Yaml>, default: [f64; N], what: &str) -> Result<[f64; N]> {
    let Some(y) = y else { return Ok(default) };
    let v = y.floats().ok_or_else(|| Error::Parse {
        line: 0,
        message: format!("`{what}` must be numeric"),
    })?;
    let mut out = default;
    if v.len() == 1 {
        out = [v[0]; N];
    } else if v.len() == N {
        out.copy_from_slice(&v);
    } else if v.len() == 3 && N == 4 {
        out[..3].copy_from_slice(&v);
    } else {
        return Err(Error::Parse {
            line: 0,
            message: format!("`{what}` expects {N} numbers, got {}", v.len()),
        });
    }
    Ok(out)
}

fn three(v: [f64; 4]) -> [f64; 3] {
    [v[0], v[1], v[2]]
}

fn log_params(y: &Yaml, camera: bool) -> Result<LogParams> {
    let base = y.get("base").and_then(Yaml::as_f64).unwrap_or(2.0);
    let p = LogParams {
        base,
        log_side_slope: three(floats_n(
            y.get("log_side_slope"),
            [1.0; 4],
            "log_side_slope",
        )?),
        log_side_offset: three(floats_n(
            y.get("log_side_offset"),
            [0.0; 4],
            "log_side_offset",
        )?),
        lin_side_slope: three(floats_n(
            y.get("lin_side_slope"),
            [1.0; 4],
            "lin_side_slope",
        )?),
        lin_side_offset: three(floats_n(
            y.get("lin_side_offset"),
            [0.0; 4],
            "lin_side_offset",
        )?),
        lin_side_break: if camera {
            y.get("lin_side_break")
                .map(|b| floats_n(Some(b), [0.0; 4], "lin_side_break").map(three))
                .transpose()?
        } else {
            None
        },
        linear_slope: if camera {
            y.get("linear_slope")
                .map(|b| floats_n(Some(b), [1.0; 4], "linear_slope").map(three))
                .transpose()?
        } else {
            None
        },
    };
    Ok(p)
}

fn parse_transform(y: &Yaml) -> Result<TransformDef> {
    let tag = y.tag().ok_or_else(|| Error::Parse {
        line: 0,
        message: "transform without a !<Tag>".into(),
    })?;
    let dir = parse_direction(y);
    let s = |k: &str| y.get(k).and_then(Yaml::as_str).map(str::to_string);
    Ok(match tag {
        "GroupTransform" => {
            let children = y.get("children").and_then(Yaml::as_seq).unwrap_or(&[]);
            TransformDef::Group(
                children
                    .iter()
                    .map(parse_transform)
                    .collect::<Result<Vec<_>>>()?,
            )
        }
        "MatrixTransform" => {
            let mut ident = [0.0; 16];
            for i in 0..4 {
                ident[i * 5] = 1.0;
            }
            TransformDef::Matrix {
                matrix: floats_n(y.get("matrix"), ident, "matrix")?,
                offset: floats_n(y.get("offset"), [0.0; 4], "offset")?,
                direction: dir,
            }
        }
        "FileTransform" => TransformDef::File {
            src: s("src").ok_or_else(|| Error::Parse {
                line: 0,
                message: "FileTransform needs `src`".into(),
            })?,
            interpolation: s("interpolation").unwrap_or_else(|| "tetrahedral".into()),
            direction: dir,
        },
        "ExponentTransform" => TransformDef::Exponent {
            value: floats_n(y.get("value"), [1.0; 4], "value")?,
            direction: dir,
            negative: match y.get("style").and_then(Yaml::as_str) {
                None | Some("linear") => NegativeStyle::Clamp,
                Some("mirror") => NegativeStyle::Mirror,
                Some("pass_thru") => NegativeStyle::PassThru,
                Some(other) => {
                    return Err(Error::Unsupported(format!(
                        "ExponentTransform style `{other}`"
                    )))
                }
            },
        },
        "ExponentWithLinearTransform" => TransformDef::ExponentWithLinear {
            gamma: floats_n(y.get("gamma"), [1.0; 4], "gamma")?,
            offset: floats_n(y.get("offset"), [0.0; 4], "offset")?,
            direction: dir,
        },
        "LogTransform" => TransformDef::Log {
            base: y.get("base").and_then(Yaml::as_f64).unwrap_or(2.0),
            direction: dir,
        },
        "LogAffineTransform" => TransformDef::LogAffine {
            params: log_params(y, false)?,
            direction: dir,
        },
        "LogCameraTransform" => TransformDef::LogCamera {
            params: log_params(y, true)?,
            direction: dir,
        },
        "CDLTransform" => TransformDef::Cdl {
            cdl: Cdl {
                slope: three(floats_n(y.get("slope"), [1.0; 4], "slope")?),
                offset: three(floats_n(y.get("offset"), [0.0; 4], "offset")?),
                power: three(floats_n(y.get("power"), [1.0; 4], "power")?),
                saturation: y
                    .get("sat")
                    .or_else(|| y.get("saturation"))
                    .and_then(Yaml::as_f64)
                    .unwrap_or(1.0),
            },
            direction: dir,
        },
        "RangeTransform" => {
            let f = |k: &str| y.get(k).and_then(Yaml::as_f64);
            let clamp = !s("style")
                .map(|st| st.eq_ignore_ascii_case("noClamp"))
                .unwrap_or(false);
            TransformDef::Range {
                min_in: f("min_in_value"),
                max_in: f("max_in_value"),
                min_out: f("min_out_value"),
                max_out: f("max_out_value"),
                clamp,
                direction: dir,
            }
        }
        "BuiltinTransform" => TransformDef::Builtin {
            style: s("style").ok_or_else(|| Error::Parse {
                line: 0,
                message: "BuiltinTransform needs `style`".into(),
            })?,
            direction: dir,
        },
        "ColorSpaceTransform" => {
            let (src, dst) = (
                s("src").ok_or_else(|| Error::Parse {
                    line: 0,
                    message: "ColorSpaceTransform needs `src`".into(),
                })?,
                s("dst").ok_or_else(|| Error::Parse {
                    line: 0,
                    message: "ColorSpaceTransform needs `dst`".into(),
                })?,
            );
            match dir {
                Direction::Forward => TransformDef::ColorSpace { src, dst },
                Direction::Inverse => TransformDef::ColorSpace { src: dst, dst: src },
            }
        }
        "DisplayViewTransform" => TransformDef::DisplayView {
            src: s("src").unwrap_or_default(),
            display: s("display").unwrap_or_default(),
            view: s("view").unwrap_or_default(),
            direction: dir,
        },
        other => return Err(Error::Unsupported(format!("transform type `{other}`"))),
    })
}

fn parse_colorspace(y: &Yaml, is_display: bool) -> Result<ColorSpaceDef> {
    let s = |k: &str| {
        y.get(k)
            .and_then(Yaml::as_str)
            .map(str::to_string)
            .unwrap_or_default()
    };
    let name = y
        .get("name")
        .and_then(Yaml::as_str)
        .ok_or_else(|| Error::Parse {
            line: 0,
            message: "colorspace without `name`".into(),
        })?;
    let tr = |keys: &[&str]| -> Result<Option<TransformDef>> {
        for k in keys {
            if let Some(t) = y.get(k) {
                if !t.is_null() {
                    return Ok(Some(parse_transform(t)?));
                }
            }
        }
        Ok(None)
    };
    Ok(ColorSpaceDef {
        name: name.to_string(),
        family: s("family"),
        aliases: y.get("aliases").map(Yaml::strings).unwrap_or_default(),
        encoding: s("encoding"),
        bitdepth: s("bitdepth"),
        description: s("description").trim_end().to_string(),
        isdata: y.get("isdata").and_then(Yaml::as_bool).unwrap_or(false),
        is_display,
        to_reference: tr(&["to_reference", "to_scene_reference", "to_display_reference"])?,
        from_reference: tr(&[
            "from_reference",
            "from_scene_reference",
            "from_display_reference",
        ])?,
    })
}

impl Config {
    /// Parse config text.
    pub fn parse(text: &str) -> Result<Config> {
        let doc = yaml::parse(text)?;
        let mut cfg = Config::default();
        let s = |k: &str| doc.get(k).and_then(Yaml::as_str).map(str::to_string);
        cfg.version = s("ocio_profile_version").unwrap_or_else(|| "1".into());
        cfg.name = s("name").unwrap_or_default();
        cfg.description = s("description").unwrap_or_default().trim_end().to_string();
        cfg.search_path = doc
            .get("search_path")
            .map(|y| {
                y.strings()
                    .iter()
                    .flat_map(|s| s.split(':').map(str::to_string))
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        if let Some(roles) = doc.get("roles").and_then(Yaml::as_map) {
            for (k, v) in roles {
                if let Some(v) = v.as_str() {
                    cfg.roles.insert(k.clone(), v.to_string());
                }
            }
        }
        // OCIO v2 shared views: defined once, referenced from displays with
        // `!<Views> [a, b]`, and `<USE_DISPLAY_NAME>` standing for the display
        // that references them. Every released ACES config uses them.
        let parse_view = |v: &Yaml, owner: &str| -> Result<View> {
            let g = |k: &str| v.get(k).and_then(Yaml::as_str).map(str::to_string);
            Ok(View {
                name: g("name").ok_or_else(|| Error::Parse {
                    line: 0,
                    message: format!("view without name in `{owner}`"),
                })?,
                colorspace: g("colorspace"),
                view_transform: g("view_transform"),
                display_colorspace: g("display_colorspace"),
                looks: g("looks"),
            })
        };
        let mut shared: IndexMap<String, View> = IndexMap::new();
        for v in doc
            .get("shared_views")
            .and_then(Yaml::as_seq)
            .unwrap_or(&[])
        {
            let view = parse_view(v, "shared_views")?;
            shared.insert(view.name.clone(), view);
        }
        if let Some(displays) = doc.get("displays").and_then(Yaml::as_map) {
            for (name, views) in displays {
                let mut list = Vec::new();
                for v in views.as_seq().unwrap_or(&[]) {
                    if v.tag() == Some("Views") {
                        for referenced in v.strings() {
                            let mut view =
                                shared
                                    .get(&referenced)
                                    .cloned()
                                    .ok_or_else(|| Error::Parse {
                                        line: 0,
                                        message: format!(
                                        "display `{name}` references shared view `{referenced}`, \
                                         which is not defined"
                                    ),
                                    })?;
                            if view.display_colorspace.as_deref() == Some("<USE_DISPLAY_NAME>") {
                                view.display_colorspace = Some(name.clone());
                            }
                            list.push(view);
                        }
                        continue;
                    }
                    list.push(parse_view(v, name)?);
                }
                cfg.displays.insert(name.clone(), list);
            }
        }
        cfg.active_displays = doc
            .get("active_displays")
            .map(Yaml::strings)
            .unwrap_or_default();
        cfg.active_views = doc
            .get("active_views")
            .map(Yaml::strings)
            .unwrap_or_default();
        for l in doc.get("looks").and_then(Yaml::as_seq).unwrap_or(&[]) {
            let g = |k: &str| l.get(k).and_then(Yaml::as_str).map(str::to_string);
            cfg.looks.push(Look {
                name: g("name").ok_or_else(|| Error::Parse {
                    line: 0,
                    message: "look without name".into(),
                })?,
                process_space: g("process_space").unwrap_or_default(),
                transform: l
                    .get("transform")
                    .filter(|t| !t.is_null())
                    .map(parse_transform)
                    .transpose()?,
                inverse_transform: l
                    .get("inverse_transform")
                    .filter(|t| !t.is_null())
                    .map(parse_transform)
                    .transpose()?,
            });
        }
        for vt in doc
            .get("view_transforms")
            .and_then(Yaml::as_seq)
            .unwrap_or(&[])
        {
            let g = |k: &str| vt.get(k).and_then(Yaml::as_str).map(str::to_string);
            cfg.view_transforms.push(ViewTransformDef {
                name: g("name").ok_or_else(|| Error::Parse {
                    line: 0,
                    message: "view transform without name".into(),
                })?,
                description: g("description").unwrap_or_default().trim_end().to_string(),
                from_scene_reference: vt
                    .get("from_scene_reference")
                    .filter(|t| !t.is_null())
                    .map(parse_transform)
                    .transpose()?,
                to_scene_reference: vt
                    .get("to_scene_reference")
                    .filter(|t| !t.is_null())
                    .map(parse_transform)
                    .transpose()?,
                from_display_reference: vt
                    .get("from_display_reference")
                    .filter(|t| !t.is_null())
                    .map(parse_transform)
                    .transpose()?,
                to_display_reference: vt
                    .get("to_display_reference")
                    .filter(|t| !t.is_null())
                    .map(parse_transform)
                    .transpose()?,
            });
        }
        cfg.default_view_transform = s("default_view_transform");
        // `display_colorspaces:` and `colorspaces:` are appended in the order
        // the blocks appear in the file, so `to_yaml` → `parse` keeps the
        // colorspace order of any config whose display colorspaces are all
        // before or all after the scene ones.
        for (key, block) in doc.as_map().unwrap_or(&[]) {
            let is_display = match key.as_str() {
                "display_colorspaces" => true,
                "colorspaces" => false,
                _ => continue,
            };
            for cs in block.as_seq().unwrap_or(&[]) {
                cfg.colorspaces.push(parse_colorspace(cs, is_display)?);
            }
        }
        for rule in doc.get("file_rules").and_then(Yaml::as_seq).unwrap_or(&[]) {
            cfg.file_rules.push(parse_file_rule(rule)?);
        }
        const IMPLEMENTED: &[&str] = &[
            "ocio_profile_version",
            "name",
            "description",
            "search_path",
            "roles",
            "shared_views",
            "displays",
            "active_displays",
            "active_views",
            "looks",
            "view_transforms",
            "default_view_transform",
            "display_colorspaces",
            "colorspaces",
            "file_rules",
            // Recognised, and without effect on anything this parser does:
            // `luma` is deprecated and unused by OCIO v2 processing;
            // `strictparsing` governs OCIO v1's name-in-path lookup, which a v2
            // config expresses as a file rule instead.
            "luma",
            "strictparsing",
        ];
        cfg.unimplemented = doc
            .as_map()
            .unwrap_or(&[])
            .iter()
            .map(|(k, _)| k.clone())
            .filter(|k| !IMPLEMENTED.contains(&k.as_str()))
            .collect();
        Ok(cfg)
    }

    /// Load from a file; `FileTransform` paths resolve relative to it.
    pub fn load(path: &Path) -> Result<Config> {
        let text = std::fs::read_to_string(path).map_err(|e| Error::Io {
            path: path.display().to_string(),
            message: e.to_string(),
        })?;
        let mut cfg = Config::parse(&text)?;
        cfg.base_dir = path.parent().map(Path::to_path_buf);
        Ok(cfg)
    }

    // ---- lookups -----------------------------------------------------------

    /// The colorspace name a role maps to.
    pub fn role(&self, name: &str) -> Option<&str> {
        self.roles.get(name).map(String::as_str)
    }

    /// Find a colorspace by name, alias or role.
    pub fn colorspace(&self, name: &str) -> Option<&ColorSpaceDef> {
        let n = name.trim();
        self.colorspaces.iter().find(|c| c.matches(n)).or_else(|| {
            self.roles
                .get(n)
                .and_then(|target| self.colorspaces.iter().find(|c| c.matches(target)))
        })
    }

    fn require(&self, name: &str) -> Result<&ColorSpaceDef> {
        self.colorspace(name)
            .ok_or_else(|| Error::UnknownColorSpace(name.to_string()))
    }

    /// The colorspace a file at `path` is in, by the config's file rules, and
    /// which rule decided. `None` only for a config with no rules at all (a
    /// v1 config); a v2 config always ends in `Default`.
    pub fn colorspace_for_path(&self, path: &str) -> Result<Option<RuleMatch>> {
        let names: Vec<(String, String)> = self
            .colorspaces
            .iter()
            .flat_map(|c| {
                std::iter::once((c.name.clone(), c.name.clone()))
                    .chain(c.aliases.iter().map(|a| (a.clone(), c.name.clone())))
            })
            .collect();
        match_path(&self.file_rules, path, &names)
    }

    /// Names of all colorspaces (display ones included).
    pub fn colorspace_names(&self) -> Vec<&str> {
        self.colorspaces.iter().map(|c| c.name.as_str()).collect()
    }

    /// Views of a display.
    pub fn views(&self, display: &str) -> Result<&[View]> {
        self.displays
            .get(display)
            .map(Vec::as_slice)
            .ok_or_else(|| Error::UnknownDisplay(display.to_string()))
    }

    fn view_transform(&self, name: &str) -> Option<&ViewTransformDef> {
        self.view_transforms
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(name))
    }

    /// The view transform that bridges scene and display references: the
    /// named default, or else the first one — scene-referred either way, as in
    /// OpenColorIO, since a display-referred one cannot bridge.
    fn default_view_transform(&self) -> Option<&ViewTransformDef> {
        self.default_view_transform
            .as_deref()
            .and_then(|n| self.view_transform(n))
            .filter(|vt| !vt.is_display_referred())
            .or_else(|| {
                self.view_transforms
                    .iter()
                    .find(|vt| !vt.is_display_referred())
            })
    }

    // ---- compilation -------------------------------------------------------

    fn resolve_file(&self, src: &str) -> Result<PathBuf> {
        let p = Path::new(src);
        if p.is_absolute() && p.exists() {
            return Ok(p.to_path_buf());
        }
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(b) = &self.base_dir {
            dirs.push(b.clone());
            for sp in &self.search_path {
                dirs.push(b.join(sp));
            }
        }
        for sp in &self.search_path {
            dirs.push(PathBuf::from(sp));
        }
        dirs.push(PathBuf::from("."));
        for d in dirs {
            let c = d.join(src);
            if c.exists() {
                return Ok(c);
            }
        }
        Err(Error::Io {
            path: src.to_string(),
            message: "LUT file not found in search path".into(),
        })
    }

    /// Compile a transform definition in a direction.
    pub fn compile(&self, def: &TransformDef, direction: Direction) -> Result<Vec<Op>> {
        self.compile_depth(def, direction, 0)
    }

    fn compile_depth(
        &self,
        def: &TransformDef,
        direction: Direction,
        depth: usize,
    ) -> Result<Vec<Op>> {
        if depth > MAX_DEPTH {
            return Err(Error::Invalid(
                "transform recursion too deep (cyclic ColorSpaceTransform?)".into(),
            ));
        }
        let combine = |d: Direction| {
            if direction == Direction::Forward {
                d
            } else {
                d.flipped()
            }
        };
        let ops: Vec<Op> = match def {
            TransformDef::Group(children) => {
                let mut ops = Vec::new();
                match direction {
                    Direction::Forward => {
                        for c in children {
                            ops.extend(self.compile_depth(c, Direction::Forward, depth + 1)?);
                        }
                    }
                    Direction::Inverse => {
                        for c in children.iter().rev() {
                            ops.extend(self.compile_depth(c, Direction::Inverse, depth + 1)?);
                        }
                    }
                }
                ops
            }
            TransformDef::Matrix {
                matrix,
                offset,
                direction: d,
            } => {
                let m = Mat3([
                    [matrix[0], matrix[1], matrix[2]],
                    [matrix[4], matrix[5], matrix[6]],
                    [matrix[8], matrix[9], matrix[10]],
                ]);
                let op = Op::Matrix {
                    m,
                    offset: [offset[0], offset[1], offset[2]],
                };
                vec![invert_if(op, combine(*d))?]
            }
            TransformDef::File {
                src,
                interpolation,
                direction: d,
            } => {
                if combine(*d) == Direction::Inverse {
                    return Err(Error::Unsupported(format!("inverse FileTransform `{src}`")));
                }
                let path = self.resolve_file(src)?;
                let text = std::fs::read_to_string(&path).map_err(|e| Error::Io {
                    path: path.display().to_string(),
                    message: e.to_string(),
                })?;
                if !src.to_ascii_lowercase().ends_with(".cube") {
                    return Err(Error::Unsupported(format!(
                        "LUT format of `{src}` (only .cube is supported)"
                    )));
                }
                match parse_cube(&text)? {
                    Lut::OneD(l) => vec![Op::Lut1D(Arc::new(l))],
                    Lut::ThreeD(mut l) => {
                        if interpolation.eq_ignore_ascii_case("trilinear")
                            || interpolation.eq_ignore_ascii_case("linear")
                        {
                            l.interpolation = Interpolation::Trilinear;
                        }
                        vec![Op::Lut3D(Arc::new(l))]
                    }
                }
            }
            TransformDef::Exponent {
                value,
                direction: d,
                negative,
            } => vec![Op::Exponent {
                value: three(*value),
                direction: combine(*d),
                negative: *negative,
            }],
            TransformDef::ExponentWithLinear {
                gamma,
                offset,
                direction: d,
            } => {
                vec![Op::MonCurve {
                    gamma: three(*gamma),
                    offset: three(*offset),
                    direction: combine(*d),
                }]
            }
            TransformDef::Log { base, direction: d } => {
                vec![Op::Log {
                    params: LogParams {
                        base: *base,
                        ..Default::default()
                    },
                    direction: combine(*d),
                }]
            }
            TransformDef::LogAffine {
                params,
                direction: d,
            }
            | TransformDef::LogCamera {
                params,
                direction: d,
            } => {
                vec![Op::Log {
                    params: params.clone(),
                    direction: combine(*d),
                }]
            }
            TransformDef::Cdl { cdl, direction: d } => vec![Op::Cdl {
                cdl: cdl.clone(),
                direction: combine(*d),
            }],
            TransformDef::Range {
                min_in,
                max_in,
                min_out,
                max_out,
                clamp,
                direction: d,
            } => {
                let op = Op::Range {
                    min_in: *min_in,
                    max_in: *max_in,
                    min_out: *min_out,
                    max_out: *max_out,
                    clamp: *clamp,
                };
                vec![invert_if(op, combine(*d))?]
            }
            TransformDef::Builtin {
                style,
                direction: d,
            } => builtin_ops(style, combine(*d))?,
            TransformDef::ColorSpace { src, dst } => {
                let (a, b) = match direction {
                    Direction::Forward => (src, dst),
                    Direction::Inverse => (dst, src),
                };
                self.processor_ops(a, b, depth + 1)?
            }
            TransformDef::DisplayView {
                src,
                display,
                view,
                direction: d,
            } => {
                if combine(*d) == Direction::Inverse {
                    return Err(Error::Unsupported("inverse DisplayViewTransform".into()));
                }
                self.display_ops(src, display, view, depth + 1)?
            }
        };
        Ok(ops)
    }

    /// Ops from a colorspace to its reference.
    fn ops_to_reference(&self, cs: &ColorSpaceDef, depth: usize) -> Result<Vec<Op>> {
        if cs.isdata {
            return Ok(Vec::new());
        }
        if let Some(t) = &cs.to_reference {
            return self.compile_depth(t, Direction::Forward, depth);
        }
        if let Some(t) = &cs.from_reference {
            return self.compile_depth(t, Direction::Inverse, depth);
        }
        Ok(Vec::new())
    }

    /// Ops from the reference to a colorspace.
    fn ops_from_reference(&self, cs: &ColorSpaceDef, depth: usize) -> Result<Vec<Op>> {
        if cs.isdata {
            return Ok(Vec::new());
        }
        if let Some(t) = &cs.from_reference {
            return self.compile_depth(t, Direction::Forward, depth);
        }
        if let Some(t) = &cs.to_reference {
            return self.compile_depth(t, Direction::Inverse, depth);
        }
        Ok(Vec::new())
    }

    /// Cross between the scene and display references when `from` and `to` are
    /// not the same one; nothing when they are.
    fn bridge_ops(&self, from_display: bool, to_display: bool, depth: usize) -> Result<Vec<Op>> {
        match (from_display, to_display) {
            (false, true) => self.scene_to_display_ops(depth),
            (true, false) => {
                let vt = self.default_view_transform().ok_or_else(|| {
                    Error::Unsupported(
                        "no view transform to bridge display and scene references".into(),
                    )
                })?;
                self.view_transform_ops(vt, Direction::Inverse, depth)
            }
            _ => Ok(Vec::new()),
        }
    }

    /// Scene reference → display reference through the default view transform.
    fn scene_to_display_ops(&self, depth: usize) -> Result<Vec<Op>> {
        let vt = self.default_view_transform().ok_or_else(|| {
            Error::Unsupported("no view transform to bridge scene and display references".into())
        })?;
        self.view_transform_ops(vt, Direction::Forward, depth)
    }

    fn view_transform_ops(
        &self,
        vt: &ViewTransformDef,
        direction: Direction,
        depth: usize,
    ) -> Result<Vec<Op>> {
        let (from, to) = if vt.is_display_referred() {
            (&vt.from_display_reference, &vt.to_display_reference)
        } else {
            (&vt.from_scene_reference, &vt.to_scene_reference)
        };
        match direction {
            Direction::Forward => {
                if let Some(t) = from {
                    self.compile_depth(t, Direction::Forward, depth)
                } else if let Some(t) = to {
                    self.compile_depth(t, Direction::Inverse, depth)
                } else {
                    Ok(Vec::new())
                }
            }
            Direction::Inverse => {
                if let Some(t) = to {
                    self.compile_depth(t, Direction::Forward, depth)
                } else if let Some(t) = from {
                    self.compile_depth(t, Direction::Inverse, depth)
                } else {
                    Ok(Vec::new())
                }
            }
        }
    }

    fn processor_ops(&self, src: &str, dst: &str, depth: usize) -> Result<Vec<Op>> {
        if depth > MAX_DEPTH {
            return Err(Error::Invalid(
                "colorspace conversion recursion too deep".into(),
            ));
        }
        let s = self.require(src)?;
        let d = self.require(dst)?;
        // Same space, or a data (non-colour) space on either side: no-op, as in OCIO.
        if s.name == d.name || s.isdata || d.isdata {
            return Ok(Vec::new());
        }
        let mut ops = self.ops_to_reference(s, depth)?;
        if s.is_display != d.is_display {
            if s.is_display {
                let vt = self.default_view_transform().ok_or_else(|| {
                    Error::Unsupported(
                        "no view transform to bridge display and scene references".into(),
                    )
                })?;
                ops.extend(self.view_transform_ops(vt, Direction::Inverse, depth)?);
            } else {
                ops.extend(self.scene_to_display_ops(depth)?);
            }
        }
        ops.extend(self.ops_from_reference(d, depth)?);
        Ok(ops)
    }

    fn look_ops(&self, spec: &str, depth: usize) -> Result<Vec<Op>> {
        // "look1, +look2, -look3": prefix '-' applies the inverse.
        let mut ops = Vec::new();
        for token in spec
            .split([',', ':'])
            .map(str::trim)
            .filter(|t| !t.is_empty())
        {
            let (inverse, name) = match token.strip_prefix('-') {
                Some(n) => (true, n.trim()),
                None => (false, token.strip_prefix('+').unwrap_or(token).trim()),
            };
            let look = self
                .looks
                .iter()
                .find(|l| l.name.eq_ignore_ascii_case(name))
                .ok_or_else(|| Error::Invalid(format!("unknown look `{name}`")))?;
            let ps = self.require(&look.process_space)?;
            ops.extend(self.ops_from_reference(ps, depth)?);
            let body = if inverse {
                match (&look.inverse_transform, &look.transform) {
                    (Some(t), _) => self.compile_depth(t, Direction::Forward, depth)?,
                    (None, Some(t)) => self.compile_depth(t, Direction::Inverse, depth)?,
                    (None, None) => Vec::new(),
                }
            } else {
                match &look.transform {
                    Some(t) => self.compile_depth(t, Direction::Forward, depth)?,
                    None => Vec::new(),
                }
            };
            ops.extend(body);
            ops.extend(self.ops_to_reference(ps, depth)?);
        }
        Ok(ops)
    }

    fn display_ops(&self, src: &str, display: &str, view: &str, depth: usize) -> Result<Vec<Op>> {
        let views = self.views(display)?;
        let v = views
            .iter()
            .find(|v| v.name.eq_ignore_ascii_case(view))
            .ok_or_else(|| Error::UnknownView {
                display: display.to_string(),
                view: view.to_string(),
            })?;
        let s = self.require(src)?;
        // A data source, or a view onto a data colorspace (the "Raw" view), is a no-op.
        if s.isdata
            || v.colorspace
                .as_deref()
                .and_then(|cs| self.colorspace(cs))
                .map(|d| d.isdata)
                .unwrap_or(false)
        {
            return Ok(Vec::new());
        }
        let mut ops = self.ops_to_reference(s, depth)?;
        if let Some(l) = &v.looks {
            ops.extend(self.look_ops(l, depth)?);
        }
        if let Some(cs) = &v.colorspace {
            let d = self.require(cs)?;
            ops.extend(self.bridge_ops(s.is_display, d.is_display, depth)?);
            ops.extend(self.ops_from_reference(d, depth)?);
            return Ok(ops);
        }
        let vt_name = v.view_transform.as_deref().ok_or_else(|| {
            Error::Invalid(format!(
                "view `{view}` has neither colorspace nor view_transform"
            ))
        })?;
        let vt = self
            .view_transform(vt_name)
            .ok_or_else(|| Error::Invalid(format!("unknown view transform `{vt_name}`")))?;
        // The view transform starts from one reference, the source is in the
        // other or the same: cross between them through the default view
        // transform, in whichever direction, as OpenColorIO does. Video seen
        // through an ACES view is the interesting case — a display-referred
        // source into a scene-referred view transform.
        ops.extend(self.bridge_ops(s.is_display, vt.is_display_referred(), depth)?);
        ops.extend(self.view_transform_ops(vt, Direction::Forward, depth)?);
        let dcs_name = v
            .display_colorspace
            .clone()
            .unwrap_or_else(|| display.to_string());
        let dcs = self.require(&dcs_name)?;
        ops.extend(self.ops_from_reference(dcs, depth)?);
        Ok(ops)
    }

    /// Processor converting between two colorspaces (names, aliases or roles).
    pub fn processor(&self, src: &str, dst: &str) -> Result<Processor> {
        Ok(Processor::new(self.processor_ops(src, dst, 0)?))
    }

    /// Processor from a colorspace to a display's view.
    pub fn display_processor(&self, src: &str, display: &str, view: &str) -> Result<Processor> {
        Ok(Processor::new(self.display_ops(src, display, view, 0)?))
    }

    // ---- built-in ACES config ------------------------------------------------

    /// A built-in ACES 1.3 CG-style config: ACES2065-1 reference, ACEScg /
    /// ACEScct / ACEScc, camera inputs (LogC3/4, S-Log3, Log3G10, V-Log,
    /// Canon Log 2/3), linear display gamuts, sRGB / Rec.709 / Rec.2020 /
    /// Display P3 / PQ / HLG display colorspaces and ACES SDR / HDR views.
    pub fn aces_default() -> Config {
        let mut cfg = Config {
            name: "cine-omni-aces-v1.3".into(),
            description: "Built-in ACES 1.3 CG config (CINE-OMNI subset)".into(),
            ..Default::default()
        };
        for (k, v) in [
            ("aces_interchange", "ACES2065-1"),
            ("cie_xyz_d65_interchange", "CIE-XYZ-D65"),
            ("color_picking", "sRGB - Texture"),
            ("color_timing", "ACEScct"),
            ("compositing_log", "ACEScct"),
            ("data", "Raw"),
            ("default", "ACES2065-1"),
            ("matte_paint", "sRGB - Texture"),
            ("reference", "ACES2065-1"),
            ("rendering", "ACEScg"),
            ("scene_linear", "ACEScg"),
            ("texture_paint", "sRGB - Texture"),
        ] {
            cfg.roles.insert(k.into(), v.into());
        }
        let sdr_view = |dcs: &str| View {
            name: "ACES 2.0 - SDR Video".into(),
            view_transform: Some("ACES 2.0 - SDR Video".into()),
            display_colorspace: Some(dcs.into()),
            ..Default::default()
        };
        let hdr_view = |dcs: &str| View {
            name: "ACES 2.0 - HDR Video (1000 nits, Rec.2020)".into(),
            view_transform: Some("ACES 2.0 - HDR Video (1000 nits, Rec.2020)".into()),
            display_colorspace: Some(dcs.into()),
            ..Default::default()
        };
        let un = |dcs: &str| View {
            name: "Un-tone-mapped".into(),
            view_transform: Some("Un-tone-mapped".into()),
            display_colorspace: Some(dcs.into()),
            ..Default::default()
        };
        let raw = View {
            name: "Raw".into(),
            colorspace: Some("Raw".into()),
            ..Default::default()
        };
        for d in ["sRGB", "Rec.709", "Display P3", "Rec.2020"] {
            cfg.displays
                .insert(d.into(), vec![sdr_view(d), un(d), raw.clone()]);
        }
        for d in ["Rec.2100-PQ", "Rec.2100-HLG"] {
            cfg.displays
                .insert(d.into(), vec![hdr_view(d), un(d), raw.clone()]);
        }
        cfg.active_displays = cfg.displays.keys().cloned().collect();
        cfg.active_views = vec![
            "ACES 2.0 - SDR Video".into(),
            "ACES 2.0 - HDR Video (1000 nits, Rec.2020)".into(),
            "Un-tone-mapped".into(),
            "Raw".into(),
        ];
        cfg.view_transforms = vec![
            ViewTransformDef {
                name: "ACES 2.0 - SDR Video".into(),
                description: "ACES 2.0 output transform, 100 nits, Rec.709 limited".into(),
                from_scene_reference: Some(TransformDef::Builtin {
                    style: "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-100nit-REC709_2.0".into(),
                    direction: Direction::Forward,
                }),
                ..Default::default()
            },
            ViewTransformDef {
                name: "ACES 2.0 - HDR Video (1000 nits, Rec.2020)".into(),
                description: "ACES 2.0 output transform, 1000 nits, Rec.2020 limited".into(),
                from_scene_reference: Some(TransformDef::Builtin {
                    style: "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-1000nit-REC2020_2.0"
                        .into(),
                    direction: Direction::Forward,
                }),
                ..Default::default()
            },
            ViewTransformDef {
                name: "Un-tone-mapped".into(),
                description: "Colorimetric, no tone mapping".into(),
                from_scene_reference: Some(TransformDef::Builtin {
                    style: "UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD".into(),
                    direction: Direction::Forward,
                }),
                ..Default::default()
            },
        ];
        cfg.default_view_transform = Some("Un-tone-mapped".into());

        let builtin = |style: &str| {
            Some(TransformDef::Builtin {
                style: style.into(),
                direction: Direction::Forward,
            })
        };
        let matrix_to_ap0 = |p: &Primaries| {
            let m = rgb_to_rgb_matrix(p, &Primaries::ACES_AP0).0;
            let mut f = [0.0; 16];
            for r in 0..3 {
                f[r * 4..r * 4 + 3].copy_from_slice(&m[r]);
            }
            f[15] = 1.0;
            Some(TransformDef::Matrix {
                matrix: f,
                offset: [0.0; 4],
                direction: Direction::Forward,
            })
        };
        let mut cs = |name: &str,
                      family: &str,
                      aliases: &[&str],
                      encoding: &str,
                      to_ref: Option<TransformDef>,
                      desc: &str| {
            let mut c = ColorSpaceDef::new(name);
            c.family = family.into();
            c.aliases = aliases.iter().map(|s| s.to_string()).collect();
            c.encoding = encoding.into();
            c.description = desc.into();
            c.to_reference = to_ref;
            cfg.colorspaces.push(c);
        };
        cs(
            "ACES2065-1",
            "ACES",
            &["aces2065_1", "ACES - ACES2065-1", "lin_ap0"],
            "scene-linear",
            None,
            "The Academy Color Encoding System reference colorspace (AP0, linear).",
        );
        cs(
            "ACEScg",
            "ACES",
            &["ACES - ACEScg", "lin_ap1"],
            "scene-linear",
            builtin("ACEScg_to_ACES2065-1"),
            "AP1 linear working space.",
        );
        cs(
            "ACEScct",
            "ACES",
            &["ACES - ACEScct", "acescct_ap1"],
            "log",
            builtin("ACEScct_to_ACES2065-1"),
            "AP1 log encoding with a linear toe, for grading.",
        );
        cs(
            "ACEScc",
            "ACES",
            &["ACES - ACEScc", "acescc_ap1"],
            "log",
            builtin("ACEScc_to_ACES2065-1"),
            "AP1 pure log encoding.",
        );
        cs(
            "ARRI LogC4",
            "Input/ARRI",
            &["arri_logc4", "LogC4"],
            "log",
            builtin("ARRI_LOGC4_to_ACES2065-1"),
            "ARRI LogC4 / ARRI Wide Gamut 4.",
        );
        cs(
            "ARRI LogC3 (EI800)",
            "Input/ARRI",
            &["arri_logc3_ei800", "ARRI LogC3", "LogC3"],
            "log",
            builtin("ARRI_ALEXA-LOGC-EI800-AWG_to_ACES2065-1"),
            "ARRI LogC3 at EI 800 / ARRI Wide Gamut 3.",
        );
        cs(
            "Sony S-Log3 S-Gamut3.Cine",
            "Input/Sony",
            &["slog3_sgamut3cine", "S-Log3 S-Gamut3.Cine"],
            "log",
            builtin("SONY_SLOG3_SGAMUT3.CINE_to_ACES2065-1"),
            "Sony S-Log3 / S-Gamut3.Cine.",
        );
        cs(
            "Sony S-Log3 S-Gamut3",
            "Input/Sony",
            &["slog3_sgamut3", "S-Log3 S-Gamut3"],
            "log",
            builtin("SONY_SLOG3_SGAMUT3_to_ACES2065-1"),
            "Sony S-Log3 / S-Gamut3.",
        );
        cs(
            "RED Log3G10 RWG",
            "Input/RED",
            &["log3g10_rwg", "RED Log3G10 REDWideGamutRGB"],
            "log",
            builtin("RED_LOG3G10_RWG_to_ACES2065-1"),
            "RED Log3G10 / REDWideGamutRGB.",
        );
        cs(
            "Panasonic V-Log V-Gamut",
            "Input/Panasonic",
            &["vlog_vgamut", "V-Log"],
            "log",
            builtin("PANASONIC_VLOG-VGAMUT_to_ACES2065-1"),
            "Panasonic V-Log / V-Gamut.",
        );
        cs(
            "Canon Log 2 Cinema Gamut",
            "Input/Canon",
            &["canonlog2_cgamut", "Canon Log 2"],
            "log",
            builtin("CANON_CLOG2-CGAMUT_to_ACES2065-1"),
            "Canon Log 2 / Cinema Gamut.",
        );
        cs(
            "Canon Log 3 Cinema Gamut",
            "Input/Canon",
            &["canonlog3_cgamut", "Canon Log 3"],
            "log",
            builtin("CANON_CLOG3-CGAMUT_to_ACES2065-1"),
            "Canon Log 3 / Cinema Gamut.",
        );
        cs(
            "Linear Rec.709 (sRGB)",
            "Utility",
            &[
                "lin_rec709_srgb",
                "Linear Rec.709",
                "lin_srgb",
                "Utility - Linear - sRGB",
            ],
            "scene-linear",
            matrix_to_ap0(&Primaries::REC709),
            "Linear Rec.709 / sRGB primaries, D65.",
        );
        cs(
            "Linear Rec.2020",
            "Utility",
            &["lin_rec2020"],
            "scene-linear",
            matrix_to_ap0(&Primaries::REC2020),
            "Linear Rec.2020 primaries, D65.",
        );
        cs(
            "Linear P3-D65",
            "Utility",
            &["lin_p3d65", "Linear Display P3"],
            "scene-linear",
            matrix_to_ap0(&Primaries::DISPLAY_P3),
            "Linear P3 primaries, D65.",
        );
        cs(
            "sRGB - Texture",
            "Utility",
            &["srgb_tx", "srgb_texture", "Utility - sRGB - Texture"],
            "sdr-video",
            Some(TransformDef::Group(vec![
                TransformDef::ExponentWithLinear {
                    gamma: [2.4, 2.4, 2.4, 1.0],
                    offset: [0.055, 0.055, 0.055, 0.0],
                    direction: Direction::Forward,
                },
                matrix_to_ap0(&Primaries::REC709).unwrap_or(TransformDef::Group(vec![])),
            ])),
            "sRGB encoded textures, converted colorimetrically.",
        );
        let mut raw = ColorSpaceDef::new("Raw");
        raw.family = "Utility".into();
        raw.aliases = vec!["Utility - Raw".into()];
        raw.encoding = "data".into();
        raw.isdata = true;
        raw.description = "Non-color data.".into();
        cfg.colorspaces.push(raw);

        let mut dcs =
            |name: &str, aliases: &[&str], encoding: &str, style: Option<&str>, desc: &str| {
                let mut c = ColorSpaceDef::new(name);
                c.family = "Display".into();
                c.aliases = aliases.iter().map(|s| s.to_string()).collect();
                c.encoding = encoding.into();
                c.description = desc.into();
                c.is_display = true;
                c.from_reference = style.map(|s| TransformDef::Builtin {
                    style: s.into(),
                    direction: Direction::Forward,
                });
                cfg.colorspaces.push(c);
            };
        dcs(
            "CIE-XYZ-D65",
            &["cie_xyz_d65"],
            "display-linear",
            None,
            "The display reference: CIE XYZ, D65, 1.0 = 100 cd/m².",
        );
        dcs(
            "sRGB",
            &["srgb_display", "sRGB - Display"],
            "sdr-video",
            Some("DISPLAY - CIE-XYZ-D65_to_sRGB"),
            "sRGB monitor (piecewise EOTF).",
        );
        dcs(
            "Rec.709",
            &["rec709_display", "Rec.709 - Display", "Rec.1886"],
            "sdr-video",
            Some("DISPLAY - CIE-XYZ-D65_to_REC.709"),
            "Rec.709 / BT.1886 (gamma 2.4).",
        );
        dcs(
            "Rec.2020",
            &["rec2020_display", "Rec.2020 - Display"],
            "sdr-video",
            Some("DISPLAY - CIE-XYZ-D65_to_REC.2020"),
            "Rec.2020 SDR (gamma 2.4).",
        );
        dcs(
            "Display P3",
            &[
                "displayp3_display",
                "Display P3 - Display",
                "P3-D65 - Display",
            ],
            "sdr-video",
            Some("DISPLAY - CIE-XYZ-D65_to_DISPLAY-P3"),
            "Display P3 (sRGB curve).",
        );
        dcs(
            "Rec.2100-PQ",
            &["rec2100_pq_display", "Rec.2100-PQ - Display", "ST2084"],
            "hdr-video",
            Some("DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ"),
            "Rec.2020 primaries, ST.2084 PQ.",
        );
        dcs(
            "Rec.2100-HLG",
            &["rec2100_hlg_display", "Rec.2100-HLG - Display", "HLG"],
            "hdr-video",
            Some("DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit"),
            "Rec.2020 primaries, HLG (1000-nit reference).",
        );
        cfg
    }

    // ---- writer ---------------------------------------------------------------

    /// Serialize as OCIO v2 YAML (parseable by [`Config::parse`]).
    pub fn to_yaml(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("ocio_profile_version: {}\n\n", self.version));
        if !self.name.is_empty() {
            s.push_str(&format!("name: {}\n", q(&self.name)));
        }
        if !self.description.is_empty() {
            s.push_str(&format!("description: {}\n", q(&self.description)));
        }
        s.push_str(&format!(
            "search_path: {}\n",
            q(&self.search_path.join(":"))
        ));
        s.push_str("strictparsing: true\nluma: [0.2126, 0.7152, 0.0722]\n\n");
        s.push_str("roles:\n");
        for (k, v) in &self.roles {
            s.push_str(&format!("  {}: {}\n", k, q(v)));
        }
        s.push_str("\ndisplays:\n");
        for (d, views) in &self.displays {
            s.push_str(&format!("  {}:\n", q(d)));
            for v in views {
                let mut parts = vec![format!("name: {}", q(&v.name))];
                if let Some(c) = &v.colorspace {
                    parts.push(format!("colorspace: {}", q(c)));
                }
                if let Some(c) = &v.view_transform {
                    parts.push(format!("view_transform: {}", q(c)));
                }
                if let Some(c) = &v.display_colorspace {
                    parts.push(format!("display_colorspace: {}", q(c)));
                }
                if let Some(c) = &v.looks {
                    parts.push(format!("looks: {}", q(c)));
                }
                s.push_str(&format!("    - !<View> {{{}}}\n", parts.join(", ")));
            }
        }
        s.push_str(&format!(
            "\nactive_displays: [{}]\n",
            self.active_displays
                .iter()
                .map(|d| q(d))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        s.push_str(&format!(
            "active_views: [{}]\n",
            self.active_views
                .iter()
                .map(|d| q(d))
                .collect::<Vec<_>>()
                .join(", ")
        ));
        if let Some(d) = &self.default_view_transform {
            s.push_str(&format!("default_view_transform: {}\n", q(d)));
        }
        if !self.file_rules.is_empty() {
            s.push_str("\nfile_rules:\n");
            for r in &self.file_rules {
                let mut parts = vec![format!("name: {}", q(&r.name))];
                match &r.kind {
                    FileRuleKind::Pattern {
                        colorspace,
                        pattern,
                        extension,
                    } => {
                        parts.push(format!("colorspace: {}", q(colorspace)));
                        parts.push(format!("pattern: {}", q(pattern)));
                        parts.push(format!("extension: {}", q(extension)));
                    }
                    FileRuleKind::Regex { colorspace, regex } => {
                        parts.push(format!("colorspace: {}", q(colorspace)));
                        parts.push(format!("regex: {}", q(regex)));
                    }
                    FileRuleKind::Default { colorspace } => {
                        parts.push(format!("colorspace: {}", q(colorspace)));
                    }
                    FileRuleKind::ColorSpaceNamePathSearch => {}
                }
                s.push_str(&format!("  - !<Rule> {{{}}}\n", parts.join(", ")));
            }
        }
        if !self.looks.is_empty() {
            s.push_str("\nlooks:\n");
            for l in &self.looks {
                s.push_str("  - !<Look>\n");
                s.push_str(&format!(
                    "    name: {}\n    process_space: {}\n",
                    q(&l.name),
                    q(&l.process_space)
                ));
                if let Some(t) = &l.transform {
                    s.push_str(&format!("    transform: {}\n", write_transform(t, 6)));
                }
                if let Some(t) = &l.inverse_transform {
                    s.push_str(&format!(
                        "    inverse_transform: {}\n",
                        write_transform(t, 6)
                    ));
                }
            }
        }
        if !self.view_transforms.is_empty() {
            s.push_str("\nview_transforms:\n");
            for vt in &self.view_transforms {
                s.push_str("  - !<ViewTransform>\n");
                s.push_str(&format!("    name: {}\n", q(&vt.name)));
                if !vt.description.is_empty() {
                    s.push_str(&format!("    description: {}\n", q(&vt.description)));
                }
                if let Some(t) = &vt.from_scene_reference {
                    s.push_str(&format!(
                        "    from_scene_reference: {}\n",
                        write_transform(t, 6)
                    ));
                }
                if let Some(t) = &vt.to_scene_reference {
                    s.push_str(&format!(
                        "    to_scene_reference: {}\n",
                        write_transform(t, 6)
                    ));
                }
                if let Some(t) = &vt.from_display_reference {
                    s.push_str(&format!(
                        "    from_display_reference: {}\n",
                        write_transform(t, 6)
                    ));
                }
                if let Some(t) = &vt.to_display_reference {
                    s.push_str(&format!(
                        "    to_display_reference: {}\n",
                        write_transform(t, 6)
                    ));
                }
            }
        }
        // Display colorspaces live in their own block; the block of whichever
        // kind comes first in `colorspaces` is written first so `parse`
        // restores the order (see `Config::parse`).
        let (display, scene): (Vec<&ColorSpaceDef>, Vec<&ColorSpaceDef>) =
            self.colorspaces.iter().partition(|c| c.is_display);
        let display_first = self
            .colorspaces
            .first()
            .map(|c| c.is_display)
            .unwrap_or(true);
        let write_display = |s: &mut String| {
            if !display.is_empty() {
                s.push_str("\ndisplay_colorspaces:\n");
                for c in &display {
                    write_colorspace(s, c, true);
                }
            }
        };
        let write_scene = |s: &mut String| {
            s.push_str("\ncolorspaces:\n");
            for c in &scene {
                write_colorspace(s, c, false);
            }
        };
        if display_first {
            write_display(&mut s);
            write_scene(&mut s);
        } else {
            write_scene(&mut s);
            write_display(&mut s);
        }
        s
    }
}

fn invert_if(op: Op, d: Direction) -> Result<Op> {
    match d {
        Direction::Forward => Ok(op),
        Direction::Inverse => op
            .inverse()
            .ok_or_else(|| Error::Unsupported(format!("inverse of {op:?}"))),
    }
}

/// Quote a YAML scalar when needed.
fn q(s: &str) -> String {
    let needs = s.is_empty()
        || s.contains([
            ':', '#', '[', ']', '{', '}', ',', '"', '\'', '\n', '&', '*', '!', '|', '>', '%', '@',
            '`',
        ])
        || s.starts_with(|c: char| c.is_whitespace() || c == '-' || c == '?')
        || s.ends_with(char::is_whitespace)
        || matches!(s, "true" | "false" | "null" | "yes" | "no" | "~")
        || s.parse::<f64>().is_ok();
    if needs {
        format!(
            "\"{}\"",
            s.replace('\\', "\\\\")
                .replace('"', "\\\"")
                .replace('\n', "\\n")
        )
    } else {
        s.to_string()
    }
}

fn nums(v: &[f64]) -> String {
    format!(
        "[{}]",
        v.iter().map(|x| fmt_num(*x)).collect::<Vec<_>>().join(", ")
    )
}

/// Shortest representation that parses back to the same `f64`
/// (Rust's `Display` never uses an exponent, so YAML reads it as a plain number).
fn fmt_num(x: f64) -> String {
    if x == 0.0 {
        "0".into()
    } else {
        format!("{x}")
    }
}

fn dir_str(d: Direction) -> &'static str {
    match d {
        Direction::Forward => "",
        Direction::Inverse => ", direction: inverse",
    }
}

fn opt3(name: &str, v: &[f64; 3], default: f64) -> String {
    if v.iter().all(|x| (x - default).abs() < 1e-12) {
        String::new()
    } else {
        format!(", {name}: {}", nums(v))
    }
}

/// Write a transform inline (flow style); groups become block sequences at `indent`.
fn write_transform(t: &TransformDef, indent: usize) -> String {
    let pad = " ".repeat(indent);
    match t {
        TransformDef::Group(children) => {
            let mut s = String::from("!<GroupTransform>\n");
            s.push_str(&format!("{pad}children:\n"));
            for c in children {
                s.push_str(&format!("{pad}  - {}\n", write_transform(c, indent + 4)));
            }
            s.trim_end().to_string()
        }
        TransformDef::Matrix {
            matrix,
            offset,
            direction,
        } => {
            let off = if offset.iter().all(|x| x.abs() < 1e-12) {
                String::new()
            } else {
                format!(", offset: {}", nums(offset))
            };
            format!(
                "!<MatrixTransform> {{matrix: {}{}{}}}",
                nums(matrix),
                off,
                dir_str(*direction)
            )
        }
        TransformDef::File {
            src,
            interpolation,
            direction,
        } => {
            format!(
                "!<FileTransform> {{src: {}, interpolation: {}{}}}",
                q(src),
                interpolation,
                dir_str(*direction)
            )
        }
        TransformDef::Exponent {
            value,
            direction,
            negative,
        } => format!(
            "!<ExponentTransform> {{value: {}{}{}}}",
            nums(value),
            match negative {
                NegativeStyle::Clamp => "",
                NegativeStyle::Mirror => ", style: mirror",
                NegativeStyle::PassThru => ", style: pass_thru",
            },
            dir_str(*direction)
        ),
        TransformDef::ExponentWithLinear {
            gamma,
            offset,
            direction,
        } => {
            format!(
                "!<ExponentWithLinearTransform> {{gamma: {}, offset: {}{}}}",
                nums(gamma),
                nums(offset),
                dir_str(*direction)
            )
        }
        TransformDef::Log { base, direction } => format!(
            "!<LogTransform> {{base: {}{}}}",
            fmt_num(*base),
            dir_str(*direction)
        ),
        TransformDef::LogAffine { params, direction }
        | TransformDef::LogCamera { params, direction } => {
            let camera = matches!(t, TransformDef::LogCamera { .. });
            let mut s = format!(
                "!<{}> {{base: {}{}{}{}{}",
                if camera {
                    "LogCameraTransform"
                } else {
                    "LogAffineTransform"
                },
                fmt_num(params.base),
                opt3("log_side_slope", &params.log_side_slope, 1.0),
                opt3("log_side_offset", &params.log_side_offset, 0.0),
                opt3("lin_side_slope", &params.lin_side_slope, 1.0),
                opt3("lin_side_offset", &params.lin_side_offset, 0.0)
            );
            if let Some(b) = params.lin_side_break {
                s.push_str(&format!(", lin_side_break: {}", nums(&b)));
            }
            if let Some(b) = params.linear_slope {
                s.push_str(&format!(", linear_slope: {}", nums(&b)));
            }
            s.push_str(dir_str(*direction));
            s.push('}');
            s
        }
        TransformDef::Cdl { cdl, direction } => format!(
            "!<CDLTransform> {{slope: {}, offset: {}, power: {}, sat: {}{}}}",
            nums(&cdl.slope),
            nums(&cdl.offset),
            nums(&cdl.power),
            fmt_num(cdl.saturation),
            dir_str(*direction)
        ),
        TransformDef::Range {
            min_in,
            max_in,
            min_out,
            max_out,
            clamp,
            direction,
        } => {
            let mut parts = Vec::new();
            for (k, v) in [
                ("min_in_value", min_in),
                ("max_in_value", max_in),
                ("min_out_value", min_out),
                ("max_out_value", max_out),
            ] {
                if let Some(v) = v {
                    parts.push(format!("{k}: {}", fmt_num(*v)));
                }
            }
            if !clamp {
                parts.push("style: noClamp".into());
            }
            format!(
                "!<RangeTransform> {{{}{}}}",
                parts.join(", "),
                dir_str(*direction)
            )
        }
        TransformDef::Builtin { style, direction } => format!(
            "!<BuiltinTransform> {{style: {}{}}}",
            q(style),
            dir_str(*direction)
        ),
        TransformDef::ColorSpace { src, dst } => format!(
            "!<ColorSpaceTransform> {{src: {}, dst: {}}}",
            q(src),
            q(dst)
        ),
        TransformDef::DisplayView {
            src,
            display,
            view,
            direction,
        } => {
            format!(
                "!<DisplayViewTransform> {{src: {}, display: {}, view: {}{}}}",
                q(src),
                q(display),
                q(view),
                dir_str(*direction)
            )
        }
    }
}

fn write_colorspace(s: &mut String, c: &ColorSpaceDef, display: bool) {
    s.push_str("  - !<ColorSpace>\n");
    s.push_str(&format!("    name: {}\n", q(&c.name)));
    if !c.aliases.is_empty() {
        s.push_str(&format!(
            "    aliases: [{}]\n",
            c.aliases
                .iter()
                .map(|a| q(a))
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if !c.family.is_empty() {
        s.push_str(&format!("    family: {}\n", q(&c.family)));
    }
    if !c.encoding.is_empty() {
        s.push_str(&format!("    encoding: {}\n", q(&c.encoding)));
    }
    if !c.bitdepth.is_empty() {
        s.push_str(&format!("    bitdepth: {}\n", q(&c.bitdepth)));
    }
    if !c.description.is_empty() {
        s.push_str(&format!("    description: {}\n", q(&c.description)));
    }
    if c.isdata {
        s.push_str("    isdata: true\n");
    }
    let (to_k, from_k) = if display {
        ("to_display_reference", "from_display_reference")
    } else {
        ("to_scene_reference", "from_scene_reference")
    };
    if let Some(t) = &c.to_reference {
        s.push_str(&format!("    {to_k}: {}\n", write_transform(t, 6)));
    }
    if let Some(t) = &c.from_reference {
        s.push_str(&format!("    {from_k}: {}\n", write_transform(t, 6)));
    }
}

// ---------------------------------------------------------------------------
// Builtin transforms
// ---------------------------------------------------------------------------

/// Apple Log decode (code → Rec.2020 linear), from the Academy's
/// `CSC.Apple.AppleLog_to_ACES` / `ACES_to_AppleLog` (aces-input-and-colorspaces,
/// Apache-2.0, commit 29b722bc). Below the curve's floor both directions clamp,
/// as the reference does.
fn apple_log() -> Op {
    const R_0: f64 = -0.05641088;
    const R_T: f64 = 0.01;
    const C: f64 = 47.28711236;
    const B: f64 = 0.00964052;
    const G: f64 = 0.08550479;
    const D: f64 = 0.69336945;
    let p_t = C * (R_T - R_0).powi(2);
    Op::custom_invertible(
        "Apple Log decode",
        move |c| {
            c.map(|x| {
                if x >= p_t {
                    2f64.powf((x - D) / G) - B
                } else if x >= 0.0 {
                    (x / C).sqrt() + R_0
                } else {
                    R_0
                }
            })
        },
        |c| {
            c.map(|r| {
                if r >= R_T {
                    G * (r + B).log2() + D
                } else if r >= R_0 {
                    C * (r - R_0).powi(2)
                } else {
                    0.0
                }
            })
        },
    )
}

/// ADX (Academy Density Exchange) to ACES2065-1, from the Academy's
/// `CSC.Academy.ADX10_to_ACES` / `ADX16_to_ACES` (aces-input-and-colorspaces,
/// Apache-2.0, commit 29b722bc): code to channel-dependent density, to
/// channel-independent density, to log exposure through the reference's
/// 11-point table (clamped at its ends, as CTL's `interpolate1D` is) or its
/// linear extension above 0.6, to exposure, to ACES.
///
/// There is no inverse. The Academy defines only ADX → ACES; below the table
/// the curve is flat and saturated colour has negative exposure, so any
/// ACES → ADX outside ADX's domain would be invented. OpenColorIO answers one
/// by inverting its LUT approximation, 0.5–0.75 away from what retracing the
/// reference gives; this build refuses instead.
fn adx(max_code: f64, offset: f64, scale: f64) -> Op {
    // Row-vector matrices, as the CTL writes them.
    const CDD_TO_CID: [[f64; 3]; 3] = [
        [0.75573, 0.05901, 0.16134],
        [0.22197, 0.96928, 0.07406],
        [0.02230, -0.02829, 0.76460],
    ];
    const EXP_TO_ACES: [[f64; 3]; 3] = [
        [0.72286, 0.11923, 0.01427],
        [0.12630, 0.76418, 0.08213],
        [0.15084, 0.11659, 0.90359],
    ];
    const LUT: [[f64; 2]; 11] = [
        [-0.19, -6.0],
        [0.01, -2.721718645],
        [0.028, -2.521718645],
        [0.054, -2.321718645],
        [0.095, -2.121718645],
        [0.145, -1.921718645],
        [0.22, -1.721718645],
        [0.3, -1.521718645],
        [0.4, -1.321718645],
        [0.5, -1.121718645],
        [0.6, -0.926545676714876],
    ];
    let ref_pt = (7120.0 - 1520.0) / 8000.0 * (100.0 / 55.0) - 0.18f64.log10();
    // CTL `mult_f3_f33(v, M)` is v·M; `Mat3` is column-vector, so transpose.
    let transpose = |m: [[f64; 3]; 3]| Mat3([0, 1, 2].map(|i| [0, 1, 2].map(|j| m[j][i])));
    let cdd_to_cid = transpose(CDD_TO_CID);
    let exp_to_aces = transpose(EXP_TO_ACES);
    let interpolate = |x: f64| {
        if x <= LUT[0][0] {
            return LUT[0][1];
        }
        for w in LUT.windows(2) {
            if x <= w[1][0] {
                let t = (x - w[0][0]) / (w[1][0] - w[0][0]);
                return w[0][1] + t * (w[1][1] - w[0][1]);
            }
        }
        LUT[10][1]
    };
    Op::custom("ADX to ACES2065-1", move |c| {
        let cdd = c.map(|v| (v * max_code - offset) / scale);
        let cid = cdd_to_cid.apply(cdd);
        let log_e = cid.map(|d| {
            if d <= 0.6 {
                interpolate(d)
            } else {
                (100.0 / 55.0) * d - ref_pt
            }
        });
        exp_to_aces.apply(log_e.map(|l| 10f64.powf(l)))
    })
}

/// Rec.2100 HLG for a 1000-nit display, as OpenColorIO 2.5 builds it: the
/// inverse OOTF (system gamma 1.2) followed by the OETF with negatives
/// mirrored and nothing clamped. In range this is BT.2100's inverse EOTF
/// ([`crate::transfer::hlg_inverse_eotf_nits`]); what OpenColorIO adds, and what was measured
/// against it to 8e-6, is the handling outside it: luminance is taken as
/// `|Y|` with a floor (1e-4 in its display scale of `nits · 3^1.2 / 1000`,
/// `1e-4^(1/1.2)` on the scene side), so negative and near-black colour stays
/// finite and invertible instead of collapsing to zero.
fn hlg_1000nit_display() -> Op {
    const GAMMA: f64 = 1.2;
    let lum = |c: [f64; 3]| 0.2627 * c[0] + 0.6780 * c[1] + 0.0593 * c[2];
    // Display reference (1.0 = 100 nits) to OpenColorIO's display scale.
    let to_display = 100.0 * 3f64.powf(GAMMA) / 1000.0;
    Op::custom_invertible(
        "HLG 1000nit inverse EOTF, mirrored negatives",
        move |rgb| {
            let u = rgb.map(|v| v * to_display);
            let k = lum(u).abs().max(1e-4).powf(1.0 / GAMMA - 1.0) / 3.0;
            u.map(|v| v.signum() * Transfer::Hlg.encode(v.abs() * k))
        },
        move |code| {
            let t = code.map(|v| v.signum() * Transfer::Hlg.decode(v.abs()) * 3.0);
            let k = lum(t)
                .abs()
                .max(1e-4f64.powf(1.0 / GAMMA))
                .powf(GAMMA - 1.0);
            t.map(|v| v * k / to_display)
        },
    )
}

/// One `file_rules` entry. Which keys are present decides the kind, as in
/// OpenColorIO: the reserved names `Default` and `ColorSpaceNamePathSearch`,
/// otherwise `regex`, otherwise `pattern` with `extension`.
fn parse_file_rule(y: &Yaml) -> Result<FileRule> {
    let g = |k: &str| y.get(k).and_then(Yaml::as_str).map(str::to_string);
    let name = g("name").ok_or_else(|| Error::Parse {
        line: 0,
        message: "file rule without name".into(),
    })?;
    let colorspace = || {
        g("colorspace").ok_or_else(|| Error::Parse {
            line: 0,
            message: format!("file rule `{name}` names no colorspace"),
        })
    };
    let kind = if name.eq_ignore_ascii_case("ColorSpaceNamePathSearch") {
        FileRuleKind::ColorSpaceNamePathSearch
    } else if name.eq_ignore_ascii_case("Default") {
        FileRuleKind::Default {
            colorspace: colorspace()?,
        }
    } else if let Some(regex) = g("regex") {
        FileRuleKind::Regex {
            colorspace: colorspace()?,
            regex,
        }
    } else {
        FileRuleKind::Pattern {
            colorspace: colorspace()?,
            pattern: g("pattern").unwrap_or_else(|| "*".into()),
            extension: g("extension").ok_or_else(|| Error::Parse {
                line: 0,
                message: format!("file rule `{name}` has neither regex nor extension"),
            })?,
        }
    };
    Ok(FileRule { name, kind })
}

/// A transfer encode applied to `scale · |v|` with the sign restored: what OCIO
/// calls "MIRROR NEGS". Invertible.
fn mirrored(t: Transfer, scale: f64) -> Op {
    let name = format!("{t:?} encode, mirrored negatives, scale {scale}");
    Op::custom_invertible(
        &name,
        move |c| c.map(|v| v.signum() * t.encode(v.abs() * scale)),
        move |c| c.map(|v| v.signum() * t.decode(v.abs()) / scale),
    )
}

/// An ACES 2.0 output transform style, answered by the port of the Academy's
/// reference implementation ([`crate::aces2`]). Every style it accepts is
/// checked against OpenColorIO in both directions; the tables are built once
/// per display and shared.
fn aces2_output(style: &str) -> Option<Op> {
    use std::collections::HashMap;
    use std::sync::{Mutex, OnceLock};
    static BUILT: OnceLock<Mutex<HashMap<String, Arc<crate::aces2::DisplayTransform>>>> =
        OnceLock::new();
    // This crate's earlier names for the two SDR transforms.
    let style = match style {
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_2.0" => {
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-100nit-REC709_2.0"
        }
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3lim_2.0" => {
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-100nit-P3-D65_2.0"
        }
        other => other,
    };
    let mut built = BUILT
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let t = match built.get(style) {
        Some(t) => t.clone(),
        None => {
            let t = Arc::new(crate::aces2::DisplayTransform::for_style(style)?);
            built.insert(style.to_string(), t.clone());
            t
        }
    };
    drop(built);
    let (fwd, inv) = (t.clone(), t);
    Some(Op::custom_invertible(
        &format!("ACES 2.0 output {}", fwd.display),
        move |c| fwd.to_xyz(c),
        move |c| inv.from_xyz(c),
    ))
}

/// Ops for a `BuiltinTransform` style in the given direction.
pub fn builtin_ops(style: &str, direction: Direction) -> Result<Vec<Op>> {
    let camera = |t: Transfer, p: Primaries| -> Vec<Op> {
        vec![
            Op::decode(t),
            Op::matrix(rgb_to_rgb_matrix(&p, &Primaries::ACES_AP0)),
        ]
    };
    // display reference is CIE XYZ D65 with 1.0 = 100 nits, i.e. SDR white.
    let display = |p: Primaries, encode: Op| -> Vec<Op> {
        vec![Op::matrix(p.xyz_to_rgb()), Op::clamp(0.0, 1.0), encode]
    };
    let st = style.trim();
    let forward: Vec<Op> = match st {
        "UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD" => {
            vec![Op::matrix(
                bradford_adaptation(WHITE_ACES, WHITE_D65).mul(&Primaries::ACES_AP0.rgb_to_xyz()),
            )]
        }
        "ACEScg_to_ACES2065-1" => vec![Op::matrix(rgb_to_rgb_matrix(
            &Primaries::ACES_AP1,
            &Primaries::ACES_AP0,
        ))],
        "ACEScct_to_ACES2065-1" => camera(Transfer::ACEScct, Primaries::ACES_AP1),
        "ACEScc_to_ACES2065-1" => camera(Transfer::ACEScc, Primaries::ACES_AP1),
        "ARRI_LOGC4_to_ACES2065-1" => camera(Transfer::LogC4, Primaries::ARRI_WIDE_GAMUT4),
        "ARRI_ALEXA-LOGC-EI800-AWG_to_ACES2065-1" => {
            camera(Transfer::LogC3, Primaries::ARRI_WIDE_GAMUT3)
        }
        "SONY_SLOG3_SGAMUT3.CINE_to_ACES2065-1" => {
            camera(Transfer::SLog3, Primaries::S_GAMUT3_CINE)
        }
        "SONY_SLOG3_SGAMUT3_to_ACES2065-1" => camera(Transfer::SLog3, Primaries::S_GAMUT3),
        "RED_LOG3G10_RWG_to_ACES2065-1" => camera(Transfer::Log3G10, Primaries::RED_WIDE_GAMUT),
        "PANASONIC_VLOG-VGAMUT_to_ACES2065-1" => camera(Transfer::VLog, Primaries::V_GAMUT),
        // Canon's transforms adapt the white with CAT02, not Bradford
        // (OpenColorIO's matrix reproduced to 2e-16).
        "CANON_CLOG2-CGAMUT_to_ACES2065-1" => vec![
            Op::decode(Transfer::CanonLog2),
            Op::matrix(rgb_to_rgb_matrix_cat02(&Primaries::CANON_CINEMA_GAMUT, &Primaries::ACES_AP0)),
        ],
        "CANON_CLOG3-CGAMUT_to_ACES2065-1" => vec![
            Op::decode(Transfer::CanonLog3),
            Op::matrix(rgb_to_rgb_matrix_cat02(&Primaries::CANON_CINEMA_GAMUT, &Primaries::ACES_AP0)),
        ],
        "APPLE_LOG_to_ACES2065-1" => vec![
            apple_log(),
            Op::matrix(rgb_to_rgb_matrix(&Primaries::REC2020, &Primaries::ACES_AP0)),
        ],
        "ADX10_to_ACES2065-1" => vec![adx(1023.0, 95.0, 500.0)],
        "ADX16_to_ACES2065-1" => vec![adx(65535.0, 1520.0, 8000.0)],
        "DISPLAY - CIE-XYZ-D65_to_sRGB" => display(Primaries::REC709, Op::encode(Transfer::SRgb)),
        // The OCIO 2.5 display styles of the released ACES configs. "MIRROR
        // NEGS" applies the curve to |v| and restores the sign, rather than
        // clamping negatives; Display P3 (and its HDR variant) and ST 2084 in
        // P3-D65 mirror too, and none of them clamp above 1. Each is checked
        // against OpenColorIO 2.5.2 on in- and out-of-range samples
        // (`tests/data/display-styles.tsv`).
        "DISPLAY - CIE-XYZ-D65_to_sRGB - MIRROR NEGS" => vec![
            Op::matrix(Primaries::REC709.xyz_to_rgb()),
            mirrored(Transfer::SRgb, 1.0),
        ],
        "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709 - MIRROR NEGS" => {
            vec![
                Op::matrix(Primaries::REC709.xyz_to_rgb()),
                mirrored(Transfer::Gamma(2.2), 1.0),
            ]
        }
        "DISPLAY - CIE-XYZ-D65_to_REC.1886-REC.709 - MIRROR NEGS" => {
            vec![
                Op::matrix(Primaries::REC709.xyz_to_rgb()),
                mirrored(Transfer::Gamma(2.4), 1.0),
            ]
        }
        "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65 - MIRROR NEGS" => {
            vec![
                Op::matrix(Primaries::DISPLAY_P3.xyz_to_rgb()),
                mirrored(Transfer::Gamma(2.6), 1.0),
            ]
        }
        "DISPLAY - CIE-XYZ-D65_to_DisplayP3" | "DISPLAY - CIE-XYZ-D65_to_DisplayP3-HDR" => {
            vec![
                Op::matrix(Primaries::DISPLAY_P3.xyz_to_rgb()),
                mirrored(Transfer::SRgb, 1.0),
            ]
        }
        // Display reference 1.0 = 100 nits; the PQ curve's 1.0 is 10 000 nits.
        "DISPLAY - CIE-XYZ-D65_to_ST2084-P3-D65" => {
            vec![
                Op::matrix(Primaries::DISPLAY_P3.xyz_to_rgb()),
                mirrored(Transfer::Pq, 100.0 / 10_000.0),
            ]
        }
        "DISPLAY - CIE-XYZ-D65_to_REC.709" => {
            display(Primaries::REC709, Op::encode(Transfer::Gamma(2.4)))
        }
        "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709" => {
            display(Primaries::REC709, Op::encode(Transfer::Gamma(2.2)))
        }
        "DISPLAY - CIE-XYZ-D65_to_REC.2020" => {
            display(Primaries::REC2020, Op::encode(Transfer::Gamma(2.4)))
        }
        "DISPLAY - CIE-XYZ-D65_to_DISPLAY-P3" => {
            display(Primaries::DISPLAY_P3, Op::encode(Transfer::SRgb))
        }
        "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65" => {
            display(Primaries::DISPLAY_P3, Op::encode(Transfer::Gamma(2.6)))
        }
        // Display reference 1.0 = 100 nits; PQ 1.0 = 10 000 nits. Like the
        // P3 variant, OpenColorIO 2.5 mirrors negatives and does not clamp
        // (measured through the released configs, `tests/released_configs.rs`).
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ" => vec![
            Op::matrix(Primaries::REC2020.xyz_to_rgb()),
            mirrored(Transfer::Pq, 100.0 / 10_000.0),
        ],
        "DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit" => vec![
            Op::matrix(Primaries::REC2020.xyz_to_rgb()),
            hlg_1000nit_display(),
        ],
        // ACES 1.x output transforms are the RRT + ODT pair, which this build
        // does not implement — its output transform is ACES 2.0's. Answering
        // an ACES 1.0 request with ACES 2.0 was measured against OpenColorIO
        // 2.5.2 at 0.18 wrong **on the neutral axis**: not a saturated-colour
        // edge case but visibly wrong greys, with nothing to indicate a
        // substitution had happened.
        "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-P3lim_1.1"
        | "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-REC2020lim_1.1" => {
            return Err(Error::Unsupported(format!(
                "BuiltinTransform style `{st}`: the ACES 1.0 and 1.1 output transforms \
                 (RRT + ODT) are not implemented. This build implements ACES 2.0, whose rendering differs \
                 by roughly 0.18 on the neutral axis — it is not a substitute. Use an \
                 `_2.0` style instead."
            )))
        }
        other => match aces2_output(other) {
            Some(op) => vec![op],
            None => {
                return Err(Error::Unsupported(format!(
                    "BuiltinTransform style `{other}`"
                )))
            }
        },
    };
    match direction {
        Direction::Forward => Ok(forward),
        Direction::Inverse => {
            let mut ops = Vec::with_capacity(forward.len());
            for op in forward.iter().rev() {
                ops.push(op.inverse().ok_or_else(|| {
                    Error::Unsupported(format!("inverse of BuiltinTransform style `{st}`"))
                })?);
            }
            Ok(ops)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aces_default_has_expected_shape() {
        let c = Config::aces_default();
        assert!(c.colorspace("ACEScg").is_some());
        assert!(c.colorspace("lin_ap1").is_some());
        assert_eq!(c.role("scene_linear"), Some("ACEScg"));
        assert!(c.colorspace("scene_linear").is_some());
        assert_eq!(c.views("sRGB").unwrap().len(), 3);
        assert!(c.views("nope").is_err());
        assert!(c.colorspaces.len() >= 20);
    }

    #[test]
    fn acescct_to_srgb_is_monotone() {
        let c = Config::aces_default();
        let p = c.processor("ACEScct", "sRGB").unwrap();
        let mut last = -1.0f32;
        for i in 0..=100 {
            let x = 0.1 + 0.45 * i as f32 / 100.0;
            let o = p.apply_rgb(&[x; 3]);
            assert!(o[0] > last, "not increasing at {x}: {o:?} <= {last}");
            assert!(
                (o[0] - o[1]).abs() < 1e-4 && (o[1] - o[2]).abs() < 1e-4,
                "gray drifted: {o:?}"
            );
            last = o[0];
        }
    }

    #[test]
    fn conversions_round_trip_through_reference() {
        let c = Config::aces_default();
        let fwd = c.processor("ARRI LogC4", "ACEScct").unwrap();
        let back = c.processor("ACEScct", "ARRI LogC4").unwrap();
        let v = [0.3, 0.4, 0.5];
        let o = back.apply_rgb(&fwd.apply_rgb(&v));
        for i in 0..3 {
            assert!((o[i] - v[i]).abs() < 1e-4, "{o:?}");
        }
        // Gray 18 % LogC4 → ACEScg ≈ 0.18
        let code = Transfer::LogC4.encode(0.18) as f32;
        let o = c
            .processor("ARRI LogC4", "ACEScg")
            .unwrap()
            .apply_rgb(&[code; 3]);
        for x in o {
            assert!((x - 0.18).abs() < 1e-3, "{o:?}");
        }
        // sRGB texture round trip
        let p = c
            .processor("sRGB - Texture", "Linear Rec.709 (sRGB)")
            .unwrap();
        assert!((p.apply_rgb(&[0.5; 3])[0] - 0.2140).abs() < 1e-3);
        // scene → display colorspace without a view (default view transform, colorimetric)
        let p = c.processor("Linear Rec.709 (sRGB)", "sRGB").unwrap();
        let o = p.apply_rgb(&[0.18; 3]);
        assert!((o[0] - 0.4613).abs() < 2e-3, "{o:?}");
        // display → scene (inverse bridge)
        let p = c.processor("sRGB", "ACEScg").unwrap();
        let o = p.apply_rgb(&[0.4613; 3]);
        assert!((o[0] - 0.18).abs() < 2e-3, "{o:?}");
        // data space is a no-op
        assert!(c.processor("Raw", "ACEScg").unwrap().is_identity());
    }

    #[test]
    fn display_views() {
        let c = Config::aces_default();
        let p = c
            .display_processor("ACEScg", "sRGB", "ACES 2.0 - SDR Video")
            .unwrap();
        let g = p.apply_rgb(&[0.18; 3]);
        assert!(g[0] > 0.3 && g[0] < 0.5, "{g:?}");
        let mut last = -1.0;
        for i in 0..50 {
            let v = p.apply_rgb(&[i as f32 * 0.1; 3])[0];
            assert!(v >= last);
            last = v;
        }
        let pq = c
            .display_processor(
                "ACEScg",
                "Rec.2100-PQ",
                "ACES 2.0 - HDR Video (1000 nits, Rec.2020)",
            )
            .unwrap();
        let g = pq.apply_rgb(&[0.18; 3]);
        assert!(g[0] > 0.3 && g[0] < 0.6, "{g:?}");
        let hlg = c
            .display_processor("ACEScg", "Rec.2100-HLG", "Un-tone-mapped")
            .unwrap();
        let g = hlg.apply_rgb(&[0.18; 3]);
        assert!(g[0] > 0.1 && g[0] < 0.7, "{g:?}");
        let raw = c.display_processor("ACEScg", "sRGB", "Raw").unwrap();
        assert!(raw.is_identity());
        assert!(matches!(
            c.display_processor("ACEScg", "sRGB", "nope"),
            Err(Error::UnknownView { .. })
        ));
    }

    #[test]
    fn yaml_round_trip() {
        let c = Config::aces_default();
        let text = c.to_yaml();
        let back = Config::parse(&text).unwrap();
        assert_eq!(back.colorspaces.len(), c.colorspaces.len());
        assert_eq!(back.roles, c.roles);
        assert_eq!(back.displays, c.displays);
        assert_eq!(back.view_transforms.len(), c.view_transforms.len());
        assert_eq!(back, c);
        for pair in [
            ("ARRI LogC4", "sRGB"),
            ("ACEScct", "Rec.2100-PQ"),
            ("Sony S-Log3 S-Gamut3.Cine", "ACEScg"),
        ] {
            let a = c.processor(pair.0, pair.1).unwrap();
            let b = back.processor(pair.0, pair.1).unwrap();
            let v = [0.35, 0.45, 0.55];
            let (x, y) = (a.apply_rgb(&v), b.apply_rgb(&v));
            for i in 0..3 {
                assert!((x[i] - y[i]).abs() < 1e-5, "{pair:?}: {x:?} vs {y:?}");
            }
        }
    }

    /// A style this build does not implement must be refused, never answered
    /// with a near-neighbour. Silent substitution returns colour that looks
    /// plausible and is wrong, which is worse than an error.
    #[test]
    fn an_unimplemented_output_transform_is_refused_not_substituted() {
        for style in [
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-P3lim_1.1",
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-VIDEO-1000nit-15nit-REC2020lim_1.1",
            // Parses like an ACES 2.0 display, but OpenColorIO defines no such one.
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-750nit-P3-D65_2.0",
        ] {
            match builtin_ops(style, Direction::Forward) {
                Err(Error::Unsupported(msg)) => assert!(msg.contains(style), "{msg}"),
                Err(e) => panic!("expected Unsupported, got {e}"),
                Ok(_) => panic!("`{style}` is not implemented and must not be answered"),
            }
        }
        // The ACES 2.0 transforms for those displays are.
        for style in [
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-1000nit-P3-D65_2.0",
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-1000nit-REC2020_2.0",
        ] {
            assert!(builtin_ops(style, Direction::Forward).is_ok(), "{style}");
        }
    }

    #[test]
    fn builtin_styles_invert() {
        let styles = [
            "UTILITY - ACES-AP0_to_CIE-XYZ-D65_BFD",
            "ACEScg_to_ACES2065-1",
            "ACEScct_to_ACES2065-1",
            "ACEScc_to_ACES2065-1",
            "ARRI_LOGC4_to_ACES2065-1",
            "ARRI_ALEXA-LOGC-EI800-AWG_to_ACES2065-1",
            "SONY_SLOG3_SGAMUT3.CINE_to_ACES2065-1",
            "SONY_SLOG3_SGAMUT3_to_ACES2065-1",
            "RED_LOG3G10_RWG_to_ACES2065-1",
            "PANASONIC_VLOG-VGAMUT_to_ACES2065-1",
            "CANON_CLOG2-CGAMUT_to_ACES2065-1",
            "CANON_CLOG3-CGAMUT_to_ACES2065-1",
            "DISPLAY - CIE-XYZ-D65_to_sRGB",
            "DISPLAY - CIE-XYZ-D65_to_REC.709",
            "DISPLAY - CIE-XYZ-D65_to_G2.2-REC.709",
            "DISPLAY - CIE-XYZ-D65_to_REC.2020",
            "DISPLAY - CIE-XYZ-D65_to_DISPLAY-P3",
            "DISPLAY - CIE-XYZ-D65_to_G2.6-P3-D65",
            "DISPLAY - CIE-XYZ-D65_to_REC.2100-PQ",
            "DISPLAY - CIE-XYZ-D65_to_REC.2100-HLG-1000nit",
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_2.0",
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO-P3lim_2.0",
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-1000nit-REC2020_2.0",
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - HDR-108nit-P3-D60-in-P3-D65_2.0",
        ];
        for style in styles {
            let fwd = Processor::new(builtin_ops(style, Direction::Forward).unwrap());
            let inv = Processor::new(
                builtin_ops(style, Direction::Inverse).unwrap_or_else(|e| panic!("{style}: {e}")),
            );
            // a mid-gray-ish, in-gamut input in the style's source space
            let x = if style.starts_with("DISPLAY") {
                [0.2, 0.2, 0.2]
            } else if style.starts_with("ACES-OUTPUT") {
                [0.18, 0.12, 0.08]
            } else {
                [0.3, 0.4, 0.5]
            };
            let y = fwd.apply_f64(x);
            let back = inv.apply_f64(y);
            for i in 0..3 {
                assert!(
                    (back[i] - x[i]).abs() < 1e-6,
                    "{style}: {x:?} -> {y:?} -> {back:?}"
                );
            }
        }
    }

    #[test]
    fn unsupported_builtin_is_named() {
        let e = builtin_ops("ACES-LMT - BLUE_LIGHT_ARTIFACT_FIX", Direction::Forward).unwrap_err();
        assert!(e.to_string().contains("BLUE_LIGHT_ARTIFACT_FIX"));
        let c = Config::parse("ocio_profile_version: 2\ncolorspaces:\n  - !<ColorSpace>\n    name: a\n    to_reference: !<BuiltinTransform> {style: NOPE}\n  - !<ColorSpace>\n    name: b\n").unwrap();
        assert!(matches!(c.processor("a", "b"), Err(Error::Unsupported(_))));
        assert!(matches!(
            c.processor("a", "zzz"),
            Err(Error::UnknownColorSpace(_))
        ));
    }

    #[test]
    fn cdl_and_range_and_log_transforms() {
        let text = r#"
ocio_profile_version: 2
roles:
  scene_linear: lin
colorspaces:
  - !<ColorSpace>
    name: lin
  - !<ColorSpace>
    name: graded
    from_reference: !<GroupTransform>
      children:
        - !<CDLTransform> {slope: [2, 2, 2], offset: [0.1, 0.1, 0.1], power: [1, 1, 1], sat: 1}
        - !<RangeTransform> {min_in_value: 0, max_in_value: 1, min_out_value: 0, max_out_value: 1}
  - !<ColorSpace>
    name: log2
    to_reference: !<LogTransform> {base: 2, direction: inverse}
  - !<ColorSpace>
    name: logc
    to_reference: !<LogCameraTransform> {base: 10, log_side_slope: 0.24719, log_side_offset: 0.385537, lin_side_slope: 5.555556, lin_side_offset: 0.052272, lin_side_break: 0.010591, direction: inverse}
"#;
        let c = Config::parse(text).unwrap();
        let p = c.processor("lin", "graded").unwrap();
        let o = p.apply_rgb(&[0.2, 0.6, 0.8]);
        assert!(
            (o[0] - 0.5).abs() < 1e-6 && (o[1] - 1.0).abs() < 1e-6 && (o[2] - 1.0).abs() < 1e-6,
            "{o:?}"
        );
        let p = c.processor("log2", "lin").unwrap();
        assert!((p.apply_rgb(&[3.0; 3])[0] - 8.0).abs() < 1e-5);
        let p = c.processor("logc", "lin").unwrap();
        let code = Transfer::LogC3.encode(0.18) as f32;
        assert!((p.apply_rgb(&[code; 3])[0] - 0.18).abs() < 1e-4);
        let p = c.processor("scene_linear", "log2").unwrap();
        assert!((p.apply_rgb(&[8.0; 3])[0] - 3.0).abs() < 1e-5);
    }
}

#[cfg(test)]
mod aces_version_tests {
    use super::*;

    /// ACES 1.0 and ACES 2.0 are different output transforms. Answering a
    /// request for one with the other was measured at 0.18 wrong on the
    /// **neutral axis** against OpenColorIO 2.5.2 — visibly wrong greys, not a
    /// saturated-colour edge case, and with nothing to indicate a substitution.
    #[test]
    fn an_aces_1_0_request_is_refused_rather_than_answered_with_aces_2_0() {
        let one = "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_1.0";
        match builtin_ops(one, Direction::Forward) {
            Err(Error::Unsupported(msg)) => {
                assert!(
                    msg.contains("ACES 1.0"),
                    "the error must name what is missing: {msg}"
                );
                assert!(
                    msg.contains("not a substitute"),
                    "the error must say why the near-neighbour was not used: {msg}"
                );
            }
            Err(e) => panic!("expected Unsupported, got {e}"),
            Ok(_) => panic!("ACES 1.0 is not implemented; answering it with ACES 2.0 is wrong"),
        }
    }

    /// Research 13 found a view named for ACES 2.0 delivering a per-channel
    /// tonescale without ACES 2.0's chroma compression or gamut mapping —
    /// measurably different, and marked as an approximation until the real
    /// rendering existed. It now does: the view carries no approximation, and
    /// its pixels are the port's (itself checked against OpenColorIO in
    /// `tests/aces2_oracle.rs`).
    #[test]
    fn a_view_named_for_aces_2_is_the_aces_2_rendering() {
        let c = Config::aces_default();
        let p = c
            .display_processor("ACES2065-1", "Rec.709", "ACES 2.0 - SDR Video")
            .unwrap();
        assert!(p.approximations().is_empty(), "{:?}", p.ops());
        let port = crate::aces2::DisplayTransform::new("SDR-100nit-REC709").unwrap();
        // A saturated colour, where the old approximation was 0.30 away.
        let aces = [0.4, 0.05, 0.02];
        let xyz = port.to_xyz(aces);
        let expect = Processor::new(
            builtin_ops("DISPLAY - CIE-XYZ-D65_to_REC.709", Direction::Forward).unwrap(),
        )
        .apply_f64(xyz);
        let got = p.apply_f64(aces);
        for ch in 0..3 {
            assert!((got[ch] - expect[ch]).abs() < 1e-9, "{got:?} vs {expect:?}");
        }
    }

    /// What this build does implement must still work, under its real name.
    #[test]
    fn the_aces_2_0_output_transform_is_available_and_named_honestly() {
        assert!(builtin_ops(
            "ACES-OUTPUT - ACES2065-1_to_CIE-XYZ-D65 - SDR-VIDEO_2.0",
            Direction::Forward
        )
        .is_ok());
        let c = Config::aces_default();
        assert!(
            c.views("sRGB")
                .unwrap()
                .iter()
                .any(|v| v.name.contains("ACES 2.0")),
            "the shipped config must offer the transform it actually implements"
        );
        assert!(
            !c.views("sRGB")
                .unwrap()
                .iter()
                .any(|v| v.name.contains("ACES 1.")),
            "the config must not advertise an ACES version this build does not implement"
        );
    }
}
