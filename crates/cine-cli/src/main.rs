//! `cine` — the CINE-OMNI command line.
//!
//! ```text
//! cine start [dir] [--port N] [--no-ui] [--isolated]   run the application
//! cine new <dir> [--name NAME]                 create a project
//! cine import <dir> <file.fountain|.fdx|.otio> import a document (re-import syncs)
//! cine report <dir>                            print every derived artifact per scene
//! cine export <dir> <fountain|fdx|usda|otio|breakdown|schedule|callsheets> [--scene N] [--out FILE]
//! cine edit <dir> <scene-index> <line-index> "new text"   edit a dialogue line (runs the cascade)
//! cine undo <dir> | cine redo <dir>
//! cine stats <dir>
//! cine demo                                    in-memory walkthrough of the thesis
//! cine serve <dir>                             JSON-RPC 2.0 over stdio (one client)
//! cine gpu                                     probe GPU support and kernel registry
//! ```

mod start;

use cine_pipeline::{OmniProject, ProjectSetup};
use cine_runtime::rpc;
use std::io::{BufRead, Write};
use std::path::PathBuf;
use ucg_core::prelude::*;
use ucg_schema::attrs;

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
            print!("{}", rpc::full_report(&p));
        }
        "export" => {
            let dir = args.get(1).ok_or("missing <dir>")?;
            let what = args.get(2).ok_or("missing export kind")?;
            let p = OmniProject::open(dir)?;
            let text = rpc::export(&p, what, flag(args, "--scene").and_then(|s| s.parse().ok()))?;
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
            let doc = rpc::first_document(&p).ok_or("no screenplay imported")?;
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
        "start" => {
            let dir = args
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."));
            start::run(start::StartOptions {
                dir,
                port: flag(args, "--port")
                    .and_then(|p| p.parse().ok())
                    .unwrap_or(7331),
                serve_ui: !args.iter().any(|a| a == "--no-ui"),
                isolated: args.iter().any(|a| a == "--isolated"),
            })?
        }
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
    print!("{}", rpc::full_report(&p));
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
    // One client over stdio. `cine start` is the multi-client runtime; this
    // remains for scripting and for driving the engine from another process.
    let p = OmniProject::open(dir)?;
    let rx = p.engine.subscribe_channel();
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let request = match rpc::Request::parse(&line) {
            Ok(r) => r,
            Err(e) => {
                writeln!(
                    stdout,
                    "{}",
                    rpc::error_response(&serde_json::Value::Null, -32700, &e)
                )?;
                stdout.flush()?;
                continue;
            }
        };
        let method = request.method.clone();
        if let Some(id) = &request.id {
            let msg = match rpc::dispatch(&p, &method, &request.params) {
                Ok(v) => rpc::ok_response(id, v),
                Err(e) => rpc::error_response(id, -32000, &e),
            };
            writeln!(stdout, "{msg}")?;
        }
        // Push change notifications produced by the call.
        while let Ok(cs) = rx.try_recv() {
            writeln!(stdout, "{}", rpc::change_notification(&cs))?;
        }
        stdout.flush()?;
        if method == "shutdown" {
            break;
        }
    }
    Ok(())
}
