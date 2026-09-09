//! `cine start` — the application supervisor.
//!
//! ## What it does
//!
//! One command brings up the whole local application: it finds the project,
//! opens it once, binds the runtime, serves the UI, and reports the address to
//! open. There is no separate backend, bridge or UI server to start.
//!
//! ## What a second terminal does
//!
//! Nothing dangerous. Two runtimes on one project would each hold an engine and
//! each append to the op log, interleaving two incompatible histories into a
//! file that can then never be replayed — silent corruption of the whole
//! project. So the second invocation reads the instance lock, finds the address
//! of the runtime already serving, prints it and exits successfully. The second
//! terminal *attaches*; it does not compete.
//!
//! `--isolated` exists for the case where a second independent runtime is
//! genuinely wanted, and it is deliberately explicit rather than the default.

use cine_pipeline::OmniProject;
use cine_runtime::{lock, ServeConfig, Server};
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

pub struct StartOptions {
    pub dir: PathBuf,
    pub port: u16,
    pub serve_ui: bool,
    /// Start a second runtime alongside an existing one, at the caller's risk.
    pub isolated: bool,
}

/// Locate the built UI relative to the project or the repository.
///
/// Returns `None` when the UI has not been built, which is a normal state: the
/// runtime and its API work without it, and the caller says so rather than
/// failing.
pub fn find_ui_dir(project_dir: &Path) -> Option<PathBuf> {
    let mut candidates = vec![project_dir.join("ui/dist"), project_dir.join("../ui/dist")];
    // The repository checkout, for running against a project stored elsewhere.
    if let Ok(exe) = std::env::current_exe() {
        // target/{debug,release}/cine → repository root
        if let Some(root) = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
        {
            candidates.push(root.join("ui/dist"));
        }
    }
    candidates
        .into_iter()
        .find(|c| c.join("index.html").exists())
        .and_then(|c| c.canonicalize().ok())
}

/// Whether `dir` looks like a CINE-OMNI project.
pub fn is_project_dir(dir: &Path) -> bool {
    dir.join("graph.json").exists()
        || dir.join("graph.msgpack").exists()
        || dir.join("oplog").is_dir()
}

pub fn run(opts: StartOptions) -> Result<(), Box<dyn std::error::Error>> {
    let dir = opts.dir.canonicalize().unwrap_or_else(|_| opts.dir.clone());

    if !is_project_dir(&dir) {
        return Err(format!(
            "{} is not a CINE-OMNI project.\n\
             Create one with:  cine new {}",
            dir.display(),
            dir.display()
        )
        .into());
    }

    // A runtime already serving this project is the common case for a second
    // terminal. Report where it is instead of starting a rival.
    if !opts.isolated {
        match lock::read_live_lock(&dir) {
            Ok(Some(existing)) => {
                println!("CINE-OMNI is already running for this project.");
                println!("  pid     {}", existing.pid);
                println!("  open    {}", existing.url());
                println!("\nThat runtime owns the project. Opening a second window against");
                println!("the same address shares its state; starting a second runtime would");
                println!("interleave two op logs and corrupt the project's history.");
                println!("\nUse `cine start --isolated` only if you genuinely want a separate");
                println!("runtime, and understand it must not touch the same project files.");
                return Ok(());
            }
            Ok(None) => {}
            Err(e) => return Err(format!("cannot read the runtime lock: {e}").into()),
        }
    }

    eprintln!("[1/5] opening project   {}", dir.display());
    let project = OmniProject::open(&dir)?;
    let stats = project.engine.stats();
    eprintln!(
        "      revision {} — {} nodes, {} edges",
        stats.revision, stats.nodes, stats.edges
    );

    eprintln!("[2/5] locating UI");
    let ui_dir = if opts.serve_ui {
        find_ui_dir(&dir)
    } else {
        None
    };
    match &ui_dir {
        Some(d) => eprintln!("      {}", d.display()),
        None if opts.serve_ui => {
            eprintln!("      not built — the API will serve, but there is no interface.");
            eprintln!("      build it with:  cd ui && npm install && npm run build");
        }
        None => eprintln!("      disabled (--no-ui)"),
    }

    eprintln!("[3/5] binding runtime");
    let runtime = cine_runtime::Runtime::new(project, dir.clone());
    let server = Server::bind(
        runtime.clone(),
        ServeConfig {
            ui_dir,
            port: opts.port,
            ..Default::default()
        },
    )?;
    let addr = server.local_addr()?;
    if addr.port() != opts.port {
        eprintln!("      port {} was busy; using {}", opts.port, addr.port());
    }

    eprintln!("[4/5] taking the project lock");
    // Held until the runtime exits. Not held when isolated, because an isolated
    // runtime by definition is not the owner of this project directory.
    let _guard = if opts.isolated {
        None
    } else {
        Some(lock::acquire(&dir, &addr.to_string())?)
    };

    eprintln!("[5/5] starting");
    let shutdown = server.shutdown_flag();
    let broadcaster = server.spawn_change_broadcaster();

    // Ctrl-C and SIGTERM must bring the runtime down in an orderly way: the
    // lock guard has to run its destructor, or the next start would find a
    // stale lock and have to reclaim it.
    let signal_shutdown = shutdown.clone();
    let signal_addr = addr;
    ctrlc::set_handler(move || {
        eprintln!("\nshutting down…");
        signal_shutdown.store(true, Ordering::Relaxed);
        // Unblock the accept loop by connecting to ourselves.
        let _ = std::net::TcpStream::connect(signal_addr);
    })?;

    println!();
    println!("  CINE-OMNI is running");
    println!("  ───────────────────────────────────────────");
    println!("  open      http://{addr}/");
    println!("  api       ws://{addr}/ws");
    println!("  health    http://{addr}/health");
    println!("  project   {}", dir.display());
    println!("  stop      Ctrl-C");
    println!();

    server.run()?;

    shutdown.store(true, Ordering::Relaxed);
    let _ = broadcaster.join();

    // A project whose runtime is stopping should not lose uncommitted work.
    eprintln!("saving project…");
    match runtime.project.save() {
        Ok(()) => eprintln!("saved."),
        Err(e) => eprintln!("WARNING: the project could not be saved: {e}"),
    }
    eprintln!("stopped.");
    Ok(())
}
