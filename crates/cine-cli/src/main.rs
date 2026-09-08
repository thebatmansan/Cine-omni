//! `cine` — the CINE-OMNI command line.
//!
//! ```text
//! cine new <dir> [--name NAME]                 create a project
//! cine import <dir> <file.fountain|.fdx|.otio> import a document (re-import syncs)
//! cine report <dir>                            print every derived artifact per scene
//! cine export <dir> <fountain|fdx|usda|otio|breakdown|schedule|callsheets> [--scene N] [--out FILE]
//! cine edit <dir> <scene-index> <line-index> "new text"   edit a dialogue line (runs the cascade)
//! cine undo <dir> | cine redo <dir>
//! cine stats <dir>
//! cine demo                                    in-memory walkthrough of the thesis
//! cine serve <dir>                             JSON-RPC 2.0 over stdio for UIs
//! cine gpu                                     probe GPU support and kernel registry
//! ```

use cine_pipeline::{OmniProject, ProjectSetup};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use ucg_core::prelude::*;
use ucg_schema::{attrs, kinds};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = match run(&args) {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            2
        }
    };
    std::process::exit(code);
}

type CliResult = std::result::Result<(), Box<dyn std::error::Error>>;

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .map(String::as_str)
}

fn run(args: &[String]) -> CliResult {
    let Some(cmd) = args.first() else {
        print_usage();
        return Ok(());
    };
    match cmd.as_str() {
        "new" => {
            let dir = args.get(1).ok_or("missing <dir>")?;
            let name = flag(args, "--name").unwrap_or("Untitled").to_string();
            let p = OmniProject::create(
                dir,
                ProjectSetup {
                    name,
                    ..Default::default()
                },
            )?;
            p.save()?;
            println!("created {dir} (root {})", p.root);
        }
        "import" => {
            let dir = args.get(1).ok_or("missing <dir>")?;
            let file = PathBuf::from(args.get(2).ok_or("missing <file>")?);
            let p = OmniProject::open(dir)?;
            let text = std::fs::read_to_string(&file)?;
            let key = file
                .file_name()
                .map(|s| s.to_string_lossy().to_string())
                .unwrap_or_default();
            let rev_before = p.engine.revision();
            let id = match file.extension().and_then(|e| e.to_str()).unwrap_or("") {
                "fountain" | "txt" => p.import_fountain(&text, &key)?,
                "fdx" => p.import_fdx(&text, &key)?,
                "otio" => p.import_otio(&text, &key)?,
                other => return Err(format!("unsupported extension `{other}`").into()),
            };
            p.save()?;
            let stats = p.engine.stats();
            println!(
                "imported {key} as {id}; {} transactions, graph now {} nodes / {} edges",
                p.engine.revision() - rev_before,
                stats.nodes,
                stats.edges
            );
        }
        "report" => {
            let p = OmniProject::open(args.get(1).ok_or("missing <dir>")?)?;
            print!("{}", full_report(&p));
        }
        "export" => {
            let dir = args.get(1).ok_or("missing <dir>")?;
            let what = args.get(2).ok_or("missing export kind")?;
            let p = OmniProject::open(dir)?;
            let text = export(&p, what, flag(args, "--scene").and_then(|s| s.parse().ok()))?;
            match flag(args, "--out") {
                Some(path) => {
                    std::fs::write(path, text)?;
                    println!("wrote {path}");
                }
                None => print!("{text}"),
            }
        }
        "edit" => {
            let dir = args.get(1).ok_or("missing <dir>")?;
            let scene_idx: usize = args.get(2).ok_or("missing scene index")?.parse()?;
            let line_idx: usize = args.get(3).ok_or("missing line index")?.parse()?;
            let text = args.get(4).ok_or("missing text")?;
            let p = OmniProject::open(dir)?;
            let doc = first_document(&p).ok_or("no screenplay imported")?;
            let scene = *p
                .scenes(doc)
                .get(scene_idx)
                .ok_or("scene index out of range")?;
            let line = *p
                .dialogue_lines(scene)
                .get(line_idx)
                .ok_or("line index out of range")?;
            let group = p.edit_dialogue(line, text)?;
            print_cascade(&group);
            p.save()?;
        }
        "undo" | "redo" => {
            let p = OmniProject::open(args.get(1).ok_or("missing <dir>")?)?;
            let g = if cmd == "undo" {
                p.engine.undo()?
            } else {
                p.engine.redo()?
            };
            let ops: usize = g.deltas.iter().map(|d| d.ops.len()).sum();
            println!("{cmd}: {} ({ops} ops)", g.label);
            p.save()?;
        }
        "stats" => {
            let p = OmniProject::open(args.get(1).ok_or("missing <dir>")?)?;
            println!("{}", serde_json::to_string_pretty(&p.engine.stats())?);
        }
        "demo" => demo()?,
        "serve" => serve(args.get(1).ok_or("missing <dir>")?)?,
        "gpu" => gpu_probe(),
        "help" | "--help" | "-h" => print_usage(),
        other => return Err(format!("unknown command `{other}`").into()),
    }
    Ok(())
}

fn print_usage() {
    println!(
        "{}",
        include_str!("main.rs")
            .lines()
            .skip(2)
            .take_while(|l| l.starts_with("//!"))
            .map(|l| l.trim_start_matches("//! ").trim_start_matches("//!"))
            .collect::<Vec<_>>()
            .join("\n")
    );
}

fn first_document(p: &OmniProject) -> Option<EntityId> {
    p.engine
        .read()
        .of_kind(kinds::SCRIPT_DOCUMENT)
        .first()
        .map(|n| n.id)
}

fn full_report(p: &OmniProject) -> String {
    let mut out = String::new();
    let stats = p.engine.stats();
    out.push_str(&format!(
        "revision {} — {} nodes, {} edges, {} reactors\n",
        stats.revision,
        stats.nodes,
        stats.edges,
        p.engine.reactor_names().len()
    ));
    let mut kinds: Vec<_> = stats.by_kind.iter().collect();
    kinds.sort();
    for (k, n) in kinds {
        out.push_str(&format!("  {k:<28}{n:>6}\n"));
    }
    if let Some(doc) = first_document(p) {
        for scene in p.scenes(doc) {
            out.push_str(&p.scene_report(scene));
        }
        let g = p.engine.read();
        if let Some(schedule) = cine_logistics::graph::read_schedule(&g, doc) {
            out.push_str("\nSCHEDULE\n");
            out.push_str(&schedule.to_markdown());
        }
    }
    out
}

fn export(
    p: &OmniProject,
    what: &str,
    scene: Option<usize>,
) -> std::result::Result<String, Box<dyn std::error::Error>> {
    let doc = first_document(p).ok_or("no screenplay imported")?;
    Ok(match what {
        "fountain" => p.export_fountain(doc)?,
        "fdx" => p.export_fdx(doc)?,
        "usda" => {
            let s = *p
                .scenes(doc)
                .get(scene.unwrap_or(0))
                .ok_or("scene index out of range")?;
            p.export_usda(s)?
        }
        "otio" => {
            let t = p
                .engine
                .read()
                .of_kind(kinds::NLE_TIMELINE)
                .first()
                .map(|n| n.id)
                .ok_or("no timeline imported")?;
            p.export_otio(t)?
        }
        "breakdown" => {
            let g = p.engine.read();
            cine_logistics::breakdown::to_markdown(&cine_logistics::breakdown::breakdown_document(
                &g, doc,
            ))
        }
        "schedule" => {
            let g = p.engine.read();
            cine_logistics::graph::read_schedule(&g, doc)
                .map(|s| s.to_markdown())
                .unwrap_or_else(|| "no schedule".into())
        }
        "callsheets" => {
            let g = p.engine.read();
            let schedule = EntityId::derived(doc, kinds::LOGISTICS_SCHEDULE);
            let mut out = String::new();
            for day in g.children(schedule) {
                if let Some(sheet) = cine_logistics::graph::read_call_sheet(&g, day) {
                    out.push_str(&sheet.to_markdown());
                    out.push('\n');
                }
            }
            out
        }
        other => return Err(format!("unknown export `{other}`").into()),
    })
}

fn print_cascade(group: &UndoGroup) {
    println!(
        "transaction `{}` → {} deltas",
        group.label,
        group.deltas.len()
    );
    for d in &group.deltas {
        println!(
            "  {:>4}  depth {}  {:<48} {:>3} ops",
            d.revision,
            d.depth,
            d.label,
            d.ops.len()
        );
    }
}

/// In-memory walkthrough of the thesis: import, edit, show the cascade, undo.
fn demo() -> CliResult {
    let p = OmniProject::new(ProjectSetup {
        name: "Demo".into(),
        ..Default::default()
    })?;
    let script = include_str!("../../../tests/fixtures/sample.fountain");
    let doc = p.import_fountain(script, "sample.fountain")?;
    println!("== imported sample.fountain ==");
    print!("{}", full_report(&p));
    let scenes = p.scenes(doc);
    let (scene_index, scene) = scenes
        .iter()
        .enumerate()
        .find(|(_, s)| !p.dialogue_lines(**s).is_empty())
        .map(|(i, s)| (i, *s))
        .ok_or("screenplay has no dialogue")?;
    let line = p.dialogue_lines(scene)[0];
    let old = p
        .engine
        .read()
        .node(line)
        .unwrap()
        .get_str(attrs::TEXT)
        .unwrap_or("")
        .to_string();
    println!("\n== editing first dialogue line of scene {scene_index} ==\n  was: {old:?}");
    let group = p.edit_dialogue(line, "I have been thinking about this for a very long time, and I have decided that we leave tonight, all of us, before anyone can talk us out of it.")?;
    print_cascade(&group);
    print!("{}", p.scene_report(scene));
    println!("\n== undo ==");
    let g = p.engine.undo()?;
    let ops: usize = g.deltas.iter().map(|d| d.ops.len()).sum();
    println!("undid `{}` in one step ({ops} inverse ops)", g.label);
    print!("{}", p.scene_report(scene));
    Ok(())
}

fn gpu_probe() {
    println!("kernels:");
    for k in cine_gpu::KERNELS {
        println!("  {:<8} {:?} bindings={:?}", k.name, k.stage, k.bindings);
    }
    #[cfg(feature = "wgpu")]
    match cine_gpu::device::GpuContext::new() {
        Ok(ctx) => println!("adapter: {}", ctx.adapter_name()),
        Err(e) => println!("no GPU: {e}"),
    }
    #[cfg(not(feature = "wgpu"))]
    println!("built without the `wgpu` feature; CPU reference kernels only");
}

// ---------------------------------------------------------------------------
// JSON-RPC daemon (line-delimited JSON-RPC 2.0 on stdin/stdout)
// ---------------------------------------------------------------------------

fn serve(dir: &str) -> CliResult {
    let p = OmniProject::open(dir)?;
    let rx = p.engine.subscribe_channel();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let req: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                writeln!(
                    stdout,
                    "{}",
                    rpc_error(serde_json::Value::Null, -32700, &e.to_string())
                )?;
                continue;
            }
        };
        let id = req.get("id").cloned().unwrap_or(serde_json::Value::Null);
        let method = req.get("method").and_then(|m| m.as_str()).unwrap_or("");
        let params = req
            .get("params")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let result = handle_rpc(&p, method, &params);
        let msg = match result {
            Ok(v) => serde_json::json!({ "jsonrpc": "2.0", "id": id, "result": v }).to_string(),
            Err(e) => rpc_error(id, -32000, &e.to_string()),
        };
        writeln!(stdout, "{msg}")?;
        // Push change notifications produced by the call.
        while let Ok(cs) = rx.try_recv() {
            let n = serde_json::json!({
                "jsonrpc": "2.0",
                "method": "changed",
                "params": { "revision": cs.revision(), "label": cs.delta.label, "depth": cs.depth(), "touched": cs.touched, "dirty": cs.dirty.len() }
            });
            writeln!(stdout, "{n}")?;
        }
        stdout.flush()?;
        if method == "shutdown" {
            break;
        }
    }
    Ok(())
}

fn rpc_error(id: serde_json::Value, code: i64, message: &str) -> String {
    serde_json::json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
        .to_string()
}

fn handle_rpc(
    p: &OmniProject,
    method: &str,
    params: &serde_json::Value,
) -> std::result::Result<serde_json::Value, Box<dyn std::error::Error>> {
    let entity = |key: &str| -> std::result::Result<EntityId, Box<dyn std::error::Error>> {
        params
            .get(key)
            .and_then(|v| v.as_str())
            .and_then(EntityId::parse)
            .ok_or_else(|| format!("missing entity param `{key}`").into())
    };
    Ok(match method {
        "stats" => serde_json::to_value(p.engine.stats())?,
        "node" => {
            let id = entity("id")?;
            let g = p.engine.read();
            let n = g.require(id)?;
            serde_json::json!({ "id": n.id, "kind": n.kind, "attrs": n.attrs.iter().map(|(k, v)| (k.clone(), v.to_json())).collect::<serde_json::Map<_, _>>(),
                "children": g.children(id), "edges_out": g.out_edges(id), "edges_in": g.in_edges(id) })
        }
        "kind" => {
            let kind = params
                .get("kind")
                .and_then(|v| v.as_str())
                .ok_or("missing kind")?;
            let g = p.engine.read();
            serde_json::to_value(
                g.of_kind(kind)
                    .iter()
                    .map(|n| serde_json::json!({ "id": n.id, "name": n.name() }))
                    .collect::<Vec<_>>(),
            )?
        }
        "scenes" => {
            let doc = first_document(p).ok_or("no screenplay")?;
            let g = p.engine.read();
            serde_json::to_value(
                p.scenes(doc)
                    .into_iter()
                    .map(|s| {
                        let n = g.node(s).unwrap();
                        serde_json::json!({ "id": s, "heading": n.get_str(attrs::HEADING), "number": n.get_str(attrs::SCENE_NUMBER),
                            "words": n.get_int(attrs::WORDS), "eighths": n.get_int(attrs::PAGE_EIGHTHS), "est_duration_s": n.get_float(attrs::EST_DURATION_S),
                            "lines": p.dialogue_lines(s) })
                    })
                    .collect::<Vec<_>>(),
            )?
        }
        "report" => serde_json::Value::String(full_report(p)),
        "scene_report" => serde_json::Value::String(p.scene_report(entity("scene")?)),
        "set" => {
            let id = entity("id")?;
            let key = params
                .get("key")
                .and_then(|v| v.as_str())
                .ok_or("missing key")?
                .to_string();
            let value = Value::from_json(
                params
                    .get("value")
                    .cloned()
                    .unwrap_or(serde_json::Value::Null),
            );
            let group = p
                .engine
                .transact(&format!("set {key}"), |tx| tx.set(id, &key, value))?;
            cascade_json(&group)
        }
        "edit_dialogue" => {
            let line = entity("line")?;
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("missing text")?;
            cascade_json(&p.edit_dialogue(line, text)?)
        }
        "import_fountain" => {
            let text = params
                .get("text")
                .and_then(|v| v.as_str())
                .ok_or("missing text")?;
            let key = params
                .get("key")
                .and_then(|v| v.as_str())
                .unwrap_or("script.fountain");
            serde_json::to_value(p.import_fountain(text, key)?)?
        }
        "export" => {
            let what = params
                .get("what")
                .and_then(|v| v.as_str())
                .ok_or("missing what")?;
            serde_json::Value::String(export(
                p,
                what,
                params
                    .get("scene")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize),
            )?)
        }
        "undo" => cascade_json(&p.engine.undo()?),
        "redo" => cascade_json(&p.engine.redo()?),
        "save" => {
            p.save()?;
            serde_json::Value::Bool(true)
        }
        "validate" => serde_json::to_value(p.engine.read().validate())?,
        "shutdown" => serde_json::Value::Bool(true),
        other => return Err(format!("unknown method `{other}`").into()),
    })
}

fn cascade_json(group: &UndoGroup) -> serde_json::Value {
    serde_json::json!({
        "label": group.label,
        "deltas": group.deltas.iter().map(|d| serde_json::json!({ "revision": d.revision, "label": d.label, "depth": d.depth, "ops": d.ops.len() })).collect::<Vec<_>>()
    })
}
