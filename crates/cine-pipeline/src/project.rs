//! High-level project façade used by the CLI and UI: create/open a project,
//! install reactors, import documents, and query the derived state.

use crate::{PipelineError, Result};
use std::path::Path;
use std::sync::Arc;
use ucg_core::prelude::*;
use ucg_core::Project;
use ucg_schema::{attrs, kinds, Component};

#[derive(Clone, Debug)]
pub struct ProjectSetup {
    pub name: String,
    pub frame_rate: f64,
    pub working_colorspace: String,
}

impl Default for ProjectSetup {
    fn default() -> Self {
        ProjectSetup {
            name: "Untitled".into(),
            frame_rate: 24.0,
            working_colorspace: "ACEScg".into(),
        }
    }
}

/// A CINE-OMNI project: the engine with every reactor installed, plus the
/// root `project` entity.
pub struct OmniProject {
    pub engine: Arc<Engine>,
    pub root: EntityId,
    pub disk: Option<Project>,
    /// What every surface of the open application is looking at. Not saved.
    pub session: crate::session::ProjectSession,
    /// Background work for this project, once a runtime has started it with
    /// the runners that need media. Unset in a bare project.
    pub jobs: std::sync::OnceLock<cine_jobs::JobSystem>,
    /// The directory the application runs this project in: where its jobs
    /// journal, derived media and caches live. Set by the runtime; unset in a
    /// bare project, which has nowhere to put derived media.
    pub directory: std::sync::OnceLock<std::path::PathBuf>,
}

impl OmniProject {
    /// In-memory project with all reactors.
    pub fn new(setup: ProjectSetup) -> Result<Self> {
        let engine = Arc::new(Engine::new());
        let root = Self::init_root(&engine, &setup)?;
        crate::reactors::install_all(&engine);
        Self::ensure_color_config(&engine, root)?;
        Ok(OmniProject {
            engine,
            root,
            disk: None,
            session: Default::default(),
            jobs: Default::default(),
            directory: Default::default(),
        })
    }

    /// Create an on-disk project directory.
    pub fn create(path: impl AsRef<Path>, setup: ProjectSetup) -> Result<Self> {
        let disk = Project::create(path, &setup.name)?;
        let engine = disk.engine.clone();
        let root = Self::init_root(&engine, &setup)?;
        crate::reactors::install_all(&engine);
        Self::ensure_color_config(&engine, root)?;
        disk.enable_autosave();
        Ok(OmniProject {
            engine,
            root,
            disk: Some(disk),
            session: Default::default(),
            jobs: Default::default(),
            directory: Default::default(),
        })
    }

    /// Open an on-disk project. Reactors are installed after replay so the
    /// stored derived state is trusted rather than recomputed.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let disk = Project::open(path)?;
        let engine = disk.engine.clone();
        let root = engine
            .read()
            .of_kind(kinds::PROJECT)
            .first()
            .map(|n| n.id)
            .ok_or_else(|| PipelineError::Script("project has no root entity".into()))?;
        crate::reactors::install_all(&engine);
        disk.enable_autosave();
        let project = OmniProject {
            engine,
            root,
            disk: Some(disk),
            session: Default::default(),
            jobs: Default::default(),
            directory: Default::default(),
        };
        project.migrate()?;
        Ok(project)
    }

    /// Structural corrections to data written by earlier versions, committed as
    /// system changes so they are persisted once and never undone.
    fn migrate(&self) -> Result<()> {
        let mut moved = 0;
        self.engine.commit_system(
            "migrate: captured takes out from under planned shots",
            |tx| {
                moved = cine_logistics::graph::migrate_takes_out_of_shots(tx)
                    .map_err(|e| crate::engine_err("logistics", e))?;
                Ok(())
            },
        )?;
        if moved > 0 {
            eprintln!("      migrated {moved} take(s) into the production log");
        }
        Self::ensure_color_config(&self.engine, self.root)?;
        // Media ingested before projects had a config has no input colorspace.
        let unassigned: Vec<EntityId> = {
            let g = self.engine.read();
            g.of_kind(kinds::ASSET_MEDIA)
                .iter()
                .filter(|n| n.get_str(cine_color::ucg::ATTR_INPUT_SOURCE).is_none())
                .map(|n| n.id)
                .collect()
        };
        if !unassigned.is_empty() {
            self.engine
                .commit_system("migrate: input colorspaces by file rule", |tx| {
                    let config = cine_color::ucg::project_config(tx.graph(), self.root)
                        .map(|c| cine_color::ucg::load_config(tx.graph(), c))
                        .transpose()
                        .map_err(|e| crate::engine_err("color", e))?;
                    if let Some(config) = config {
                        for media in &unassigned {
                            cine_color::ucg::assign_input_colorspace(tx, *media, &config)
                                .map_err(|e| crate::engine_err("color", e))?;
                        }
                    }
                    Ok(())
                })?;
        }
        Ok(())
    }

    /// A project without a colour config gets the bundled one. A system change:
    /// it is how the project is set up, not an edit anyone would undo.
    fn ensure_color_config(engine: &Engine, root: EntityId) -> Result<()> {
        engine.commit_system("colour config", |tx| {
            cine_color::ucg::ensure_project_config(tx, root)
                .map_err(|e| crate::engine_err("color", e))?;
            Ok(())
        })?;
        Ok(())
    }

    fn init_root(engine: &Engine, setup: &ProjectSetup) -> Result<EntityId> {
        let root = EntityId::from_key("project:root");
        engine.transact("init project", |tx| {
            tx.ensure(root, kinds::PROJECT)?;
            tx.set(root, attrs::NAME, setup.name.as_str())?;
            tx.set(root, attrs::RATE, setup.frame_rate)?;
            tx.set(root, attrs::COLORSPACE, setup.working_colorspace.as_str())
        })?;
        Ok(root)
    }

    /// What replaying the op log had to tolerate when this project was opened.
    ///
    /// A torn tail means the previous session ended mid-write. Callers should
    /// report it rather than let a project quietly open having discarded a
    /// partial record.
    pub fn recovery(&self) -> ucg_core::OplogReport {
        self.disk
            .as_ref()
            .map(|d| d.recovery.clone())
            .unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        if let Some(d) = &self.disk {
            d.save()?;
        }
        Ok(())
    }

    /// Import (or re-import) a Fountain screenplay. Returns the document id.
    pub fn import_fountain(&self, text: &str, source_key: &str) -> Result<EntityId> {
        let screenplay = cine_script::fountain::parse_fountain(text);
        self.import_screenplay(&screenplay, source_key)
    }

    /// Import (or re-import) a Final Draft document.
    pub fn import_fdx(&self, xml: &str, source_key: &str) -> Result<EntityId> {
        let screenplay =
            cine_script::fdx::parse_fdx(xml).map_err(|e| PipelineError::Script(e.to_string()))?;
        self.import_screenplay(&screenplay, source_key)
    }

    pub fn import_screenplay(
        &self,
        screenplay: &cine_script::model::Screenplay,
        source_key: &str,
    ) -> Result<EntityId> {
        let root = self.root;
        let mut doc = EntityId::nil();
        self.engine
            .transact(&format!("import screenplay {source_key}"), |tx| {
                doc = cine_script::graph::import_screenplay(tx, root, screenplay, source_key)
                    .map_err(|e| crate::engine_err("script", e))?;
                Ok(())
            })?;
        Ok(doc)
    }

    /// Import an OpenTimelineIO JSON document as a timeline.
    pub fn import_otio(&self, json: &str, source_key: &str) -> Result<EntityId> {
        let timeline = cine_timeline::otio_json::read_otio(json)
            .map_err(|e| PipelineError::Timeline(e.to_string()))?;
        let root = self.root;
        let mut id = EntityId::nil();
        self.engine
            .transact(&format!("import timeline {source_key}"), |tx| {
                id = cine_timeline::graph::import_timeline(tx, root, &timeline, source_key)
                    .map_err(|e| crate::engine_err("timeline", e))?;
                Ok(())
            })?;
        Ok(id)
    }

    /// Rewrite a screenplay from Fountain text, in place.
    ///
    /// The document keeps its source key, so unchanged scenes are unchanged
    /// nodes and the cascade touches only what moved. A scene that disappears
    /// takes its planned shots with it — and cuts loose the clips bound to it
    /// and the takes recorded against its shots. That is refused unless
    /// `allow_orphans`, and the refusal names every scene and what it holds, so
    /// a writer who only renamed an unnumbered scene can number it instead.
    pub fn rewrite_fountain(
        &self,
        document: EntityId,
        text: &str,
        allow_orphans: bool,
    ) -> Result<Rewrite> {
        let source_key = {
            let g = self.engine.read();
            let node = g
                .node(document)
                .filter(|n| n.kind.as_str() == kinds::SCRIPT_DOCUMENT)
                .ok_or_else(|| PipelineError::Script(format!("{document} is not a screenplay")))?;
            node.get_str(attrs::SOURCE_KEY)
                .ok_or_else(|| PipelineError::Script("the screenplay has no source key".into()))?
                .to_string()
        };
        let sp = cine_script::fountain::parse_fountain(text);
        let planned: std::collections::HashSet<EntityId> =
            cine_script::graph::scene_identities(&sp, &source_key)
                .into_iter()
                .map(|(_, _, id)| id)
                .collect();
        let orphans: Vec<Orphan> = {
            let g = self.engine.read();
            g.children(document)
                .into_iter()
                .filter(|c| {
                    g.node(*c)
                        .is_some_and(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                        && !planned.contains(c)
                })
                .filter_map(|scene| {
                    let clips = cine_timeline::graph::clips_synced_to(&g, scene).len();
                    let takes = cine_logistics::graph::read_shot_list(&g, scene)
                        .iter()
                        .map(|shot| {
                            cine_logistics::graph::takes_of_shot(
                                &g,
                                cine_logistics::graph::shot_id(shot.camera),
                            )
                            .len()
                        })
                        .sum();
                    (clips > 0 || takes > 0).then(|| Orphan {
                        scene,
                        heading: g
                            .node(scene)
                            .and_then(|n| n.get_str(attrs::HEADING))
                            .unwrap_or_default()
                            .to_string(),
                        clips,
                        takes,
                    })
                })
                .collect()
        };
        if !orphans.is_empty() && !allow_orphans {
            return Ok(Rewrite::Refused(orphans));
        }
        let root = self.root;
        let group = self.engine.transact("rewrite screenplay", |tx| {
            cine_script::graph::import_screenplay(tx, root, &sp, &source_key)
                .map_err(|e| crate::engine_err("script", e))?;
            Ok(())
        })?;
        Ok(Rewrite::Applied { group, orphans })
    }

    /// Export the screenplay back to Fountain text (bidirectional sync).
    pub fn export_fountain(&self, document: EntityId) -> Result<String> {
        let g = self.engine.read();
        let sp = cine_script::graph::export_screenplay(&g, document)
            .map_err(|e| PipelineError::Script(e.to_string()))?;
        Ok(cine_script::fountain::write_fountain(&sp))
    }

    pub fn export_fdx(&self, document: EntityId) -> Result<String> {
        let g = self.engine.read();
        let sp = cine_script::graph::export_screenplay(&g, document)
            .map_err(|e| PipelineError::Script(e.to_string()))?;
        Ok(cine_script::fdx::write_fdx(&sp))
    }

    /// Export a timeline as OTIO JSON.
    pub fn export_otio(&self, timeline: EntityId) -> Result<String> {
        let g = self.engine.read();
        let t = cine_timeline::graph::export_timeline(&g, timeline)
            .map_err(|e| PipelineError::Timeline(e.to_string()))?;
        Ok(cine_timeline::otio_json::write_otio(&t))
    }

    /// Export the previs stage of a script scene as USDA text.
    pub fn export_usda(&self, script_scene: EntityId) -> Result<String> {
        let g = self.engine.read();
        let previs = EntityId::derived(script_scene, kinds::PREVIS_SCENE);
        let stage = cine_previs::graph::stage_from_graph(&g, previs)
            .map_err(|e| PipelineError::Previs(e.to_string()))?;
        Ok(stage.to_usda())
    }

    /// Edit one dialogue line's text — the canonical thesis action.
    pub fn edit_dialogue(&self, line: EntityId, text: &str) -> Result<UndoGroup> {
        Ok(self
            .engine
            .transact("edit dialogue", |tx| tx.set(line, attrs::TEXT, text))?)
    }

    /// Scenes of a document in order.
    pub fn scenes(&self, document: EntityId) -> Vec<EntityId> {
        let g = self.engine.read();
        g.children(document)
            .into_iter()
            .filter(|c| {
                g.node(*c)
                    .map(|n| n.kind.as_str() == kinds::SCRIPT_SCENE)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Dialogue lines of a scene in order.
    pub fn dialogue_lines(&self, scene: EntityId) -> Vec<EntityId> {
        let g = self.engine.read();
        g.children(scene)
            .into_iter()
            .filter(|c| {
                g.node(*c)
                    .map(|n| n.kind.as_str() == kinds::SCRIPT_DIALOGUE)
                    .unwrap_or(false)
            })
            .collect()
    }

    /// Human-readable report of everything derived from a scene (for the CLI).
    pub fn scene_report(&self, scene: EntityId) -> String {
        let g = self.engine.read();
        let mut out = String::new();
        let Some(s) = g.node(scene) else {
            return "missing scene".into();
        };
        out.push_str(&format!(
            "SCENE {} {}\n  words={} eighths={} est={:.1}s characters={:?}\n",
            s.get_str(attrs::SCENE_NUMBER).unwrap_or("-"),
            s.get_str(attrs::HEADING).unwrap_or(""),
            s.get_int(attrs::WORDS).unwrap_or(0),
            s.get_int(attrs::PAGE_EIGHTHS).unwrap_or(0),
            s.get_float(attrs::EST_DURATION_S).unwrap_or(0.0),
            s.get(attrs::CHARACTERS)
                .map(|v| v
                    .as_list()
                    .map(|l| l
                        .iter()
                        .filter_map(|x| x.as_str().map(str::to_string))
                        .collect::<Vec<_>>())
                    .unwrap_or_default())
                .unwrap_or_default()
        ));
        let previs = EntityId::derived(scene, kinds::PREVIS_SCENE);
        if let Some(p) = g.node(previs) {
            let cams = cine_previs::graph::cameras_of(&g, previs).len();
            let chars = g
                .children(previs)
                .iter()
                .filter(|c| {
                    g.node(**c)
                        .map(|n| n.kind.as_str() == kinds::PREVIS_CHARACTER)
                        .unwrap_or(false)
                })
                .count();
            out.push_str(&format!(
                "  previs: {} cameras={} characters={}\n",
                p.name(),
                cams,
                chars
            ));
        }
        let shot_list = EntityId::derived(scene, kinds::LOGISTICS_SHOT_LIST);
        if g.contains(shot_list) {
            let shots = g.children(shot_list);
            out.push_str(&format!("  shot list: {} shots\n", shots.len()));
            for sh in shots {
                if let Some(spec) = ucg_schema::ShotSpec::read_from(&g, sh) {
                    let comp = EntityId::derived(sh, kinds::COMP_GRAPH);
                    let stale = g
                        .node(comp)
                        .and_then(|n| n.get_bool(attrs::STALE))
                        .unwrap_or(false);
                    out.push_str(&format!(
                        "    shot {} {:?} {:?} {}mm est={:.1}s comp={} stale={}\n",
                        spec.number,
                        spec.size,
                        spec.movement,
                        spec.focal_length_mm,
                        spec.est_duration_s,
                        if g.contains(comp) { "yes" } else { "no" },
                        stale
                    ));
                }
            }
        }
        let clips = cine_timeline::graph::clips_synced_to(&g, scene);
        if !clips.is_empty() {
            out.push_str(&format!("  clips synced: {}\n", clips.len()));
            for c in clips {
                let n = g.node(c).unwrap();
                out.push_str(&format!(
                    "    clip {} needs_conform={} grade={}\n",
                    n.name(),
                    n.get_bool(attrs::NEEDS_CONFORM).unwrap_or(false),
                    if g.contains(EntityId::derived(c, kinds::COLOR_GRADE)) {
                        "yes"
                    } else {
                        "no"
                    }
                ));
            }
        }
        let track = EntityId::derived(scene, kinds::AUDIO_TRACK);
        if g.contains(track) {
            out.push_str(&format!(
                "  audio: dialogue placeholders={}\n",
                g.children(track).len()
            ));
        }
        out
    }
}

/// A scene a rewrite would remove that holds production work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Orphan {
    pub scene: EntityId,
    pub heading: String,
    /// Clips bound to the scene.
    pub clips: usize,
    /// Takes recorded against the scene's shots.
    pub takes: usize,
}

/// What [`OmniProject::rewrite_fountain`] did.
#[derive(Debug)]
pub enum Rewrite {
    Applied {
        group: UndoGroup,
        orphans: Vec<Orphan>,
    },
    Refused(Vec<Orphan>),
}
