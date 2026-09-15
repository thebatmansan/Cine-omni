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
}

impl OmniProject {
    /// In-memory project with all reactors.
    pub fn new(setup: ProjectSetup) -> Result<Self> {
        let engine = Arc::new(Engine::new());
        let root = Self::init_root(&engine, &setup)?;
        crate::reactors::install_all(&engine);
        Ok(OmniProject {
            engine,
            root,
            disk: None,
            session: Default::default(),
            jobs: Default::default(),
        })
    }

    /// Create an on-disk project directory.
    pub fn create(path: impl AsRef<Path>, setup: ProjectSetup) -> Result<Self> {
        let disk = Project::create(path, &setup.name)?;
        let engine = disk.engine.clone();
        let root = Self::init_root(&engine, &setup)?;
        crate::reactors::install_all(&engine);
        disk.enable_autosave();
        Ok(OmniProject {
            engine,
            root,
            disk: Some(disk),
            session: Default::default(),
            jobs: Default::default(),
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
