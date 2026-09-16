//! `cine` — the CINE-OMNI command line.
//!
//! ```text
//! cine start [dir] [--port N] [--no-ui] [--no-window] [--isolated]   run the application
//! cine new <dir> [--name NAME]                 create a project
//! cine import <dir> <file.fountain|.fdx|.otio> import a document (re-import syncs)
//! cine report <dir>                            print every derived artifact per scene
//! cine export <dir> <fountain|fdx|usda|otio|breakdown|schedule|dood|callsheets> [--scene N] [--out FILE]
//! cine export <dir> callsheet-pdf --day N --out FILE   one day's call sheet, printable
//! cine edit <dir> <scene-index> <line-index> "new text"   edit a dialogue line (runs the cascade)
//! cine undo <dir> | cine redo <dir>
//! cine stats <dir>
//! cine demo                                    in-memory walkthrough of the thesis
//! cine serve <dir>                             JSON-RPC 2.0 over stdio (one client)
//! cine gpu                                     probe GPU support and kernel registry
//! cine offload <card> <destination> [--hash xxh64|xxh3|md5|sha1]
//!                                              copy a card, verify it from disk, write the ASC MHL
//! cine verify <folder>                         check a folder against its ASC MHL
//! ```

mod start;
mod window;

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
            // The one export that is not text.
            if what == "callsheet-pdf" {
                let day = flag(args, "--day")
                    .and_then(|d| d.parse().ok())
                    .unwrap_or(1usize);
                let bytes = rpc::call_sheet_pdf(&p, day)?;
                let out = flag(args, "--out").ok_or("a PDF needs --out <file>")?;
                std::fs::write(out, &bytes)?;
                println!("wrote {out} ({} bytes, day {day})", bytes.len());
                return Ok(());
            }
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
                // The native surface opens when there is a display to open it
                // on, unless asked not to. Without one the application is the
                // runtime and the browser surface, and it says so.
                window_play: flag(args, "--window-play").and_then(|s| s.parse().ok()),
                window: !args.iter().any(|a| a == "--no-window")
                    && (std::env::var_os("DISPLAY").is_some()
                        || std::env::var_os("WAYLAND_DISPLAY").is_some()),
            })?
        }
        "offload" => offload(args)?,
        "verify" => verify(args)?,
        "serve" => serve(args.get(1).ok_or("missing <dir>")?)?,
        "gpu" => gpu_probe(),
        "help" | "--help" | "-h" => print_usage(),
        other => return Err(format!("unknown command `{other}`").into()),
    }
    Ok(())
}

/// `cine offload <card> <destination>`: the DIT's first job of the day.
fn offload(args: &[String]) -> CliResult {
    use cine_media::offload::Step;
    let source = PathBuf::from(args.get(1).ok_or("missing <card>")?);
    let destination = PathBuf::from(args.get(2).ok_or("missing <destination>")?);
    let kind = hash_kind(flag(args, "--hash").unwrap_or("xxh64"))?;
    if !source.is_dir() {
        return Err(format!("{} is not a folder", source.display()).into());
    }
    println!(
        "offloading {} → {} ({kind})",
        source.display(),
        destination.display()
    );
    let mut last = String::new();
    let report = cine_media::offload::offload(&source, &destination, kind, |step| match step {
        Step::Copying { path, done, total } => {
            last = path.clone();
            print!("\r  [{:>3}/{total}] copying   {path:<48}", done + 1);
            let _ = std::io::stdout().flush();
        }
        Step::Verifying { done, total, .. } => {
            print!("\r  [{:>3}/{total}] verifying {last:<48}", done + 1);
            let _ = std::io::stdout().flush();
        }
    })?;
    println!("\r{:<80}", "");
    for (path, source_hash, copy_hash) in &report.mismatched {
        println!("  MISMATCH {path}: card {source_hash}, copy {copy_hash}");
    }
    for path in &report.refused {
        println!("  refused (already at the destination): {path}");
    }
    println!(
        "  {} file(s), {:.1} MB, {:.1} MB/s including verification",
        report.copied.len(),
        report.bytes as f64 / 1e6,
        report.megabytes_per_second()
    );
    match &report.hash_list {
        Some(path) => println!("  hash list: {}", path.display()),
        None => println!("  nothing to write a hash list for"),
    }
    if !report.is_clean() {
        return Err("the offload did not complete cleanly; the card has not been verified".into());
    }
    println!("  verified from disk. The card may be wiped.");
    Ok(())
}

/// `cine verify <folder>`: check a folder against the newest hash list it has.
fn verify(args: &[String]) -> CliResult {
    let root = PathBuf::from(args.get(1).ok_or("missing <folder>")?);
    let generations = cine_media::mhl::generations(&root);
    let newest = generations
        .last()
        .ok_or_else(|| format!("{} has no ascmhl folder to check against", root.display()))?;
    println!("checking {} against {}", root.display(), newest.display());
    let list = cine_media::mhl::read_generation(newest)?;
    if !list.unimplemented.is_empty() {
        println!(
            "  note: this list uses {} — not applied by this build",
            list.unimplemented.join(", ")
        );
    }
    let total = list.entries.len();
    let report = cine_media::mhl::verify(&root, &list, |done, _| {
        print!("\r  [{done:>3}/{total}]");
        let _ = std::io::stdout().flush();
    })?;
    println!("\r{:<40}", "");
    for (path, expected, actual) in &report.failed {
        println!("  FAILED {path}: expected {expected}, found {actual}");
    }
    for path in &report.missing {
        println!("  MISSING {path}");
    }
    for path in &report.unlisted {
        println!("  not in the list: {path}");
    }
    println!(
        "  {} verified, {} failed, {} missing, {} unlisted",
        report.verified.len(),
        report.failed.len(),
        report.missing.len(),
        report.unlisted.len()
    );
    // The check is itself a generation: the folder's history is its record.
    let written =
        cine_media::mhl::write_generation(&root, &cine_media::mhl::generation_for(&list, &report))?;
    println!("  recorded as {}", written.display());
    if !report.is_clean() {
        return Err("the folder does not match its hash list".into());
    }
    Ok(())
}

fn hash_kind(name: &str) -> std::result::Result<cine_media::mhl::HashKind, String> {
    use cine_media::mhl::HashKind;
    Ok(match name {
        "xxh64" => HashKind::Xxh64,
        "xxh3" => HashKind::Xxh3,
        "md5" => HashKind::Md5,
        "sha1" => HashKind::Sha1,
        other => {
            return Err(format!(
                "unknown hash `{other}`; ASC MHL names xxh64, xxh3, md5 and sha1"
            ))
        }
    })
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
