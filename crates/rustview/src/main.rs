//! RustView: LabVIEW-style structural view of Rust code (analysis and display only).

// Release builds started by double click should not open a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
#[command(version, about)]
struct Cli {
    /// Cargo project directory (package or workspace) or a single .rs file.
    /// Without it, RustView starts with the project selection screen.
    path: Option<PathBuf>,

    /// Command used to open a location in an editor. Placeholders: {file} {line} {col}.
    #[arg(long, default_value = rv_gui::DEFAULT_EDITOR)]
    editor: String,

    /// Select this function after loading, e.g. `ec_task::ethercat_task`.
    #[arg(long)]
    select: Option<String>,

    /// Start with the project-wide overview of threads and tasks:
    /// `graph` (default), `communication` or `functions`.
    #[arg(long, num_args = 0..=1, default_missing_value = "graph", value_name = "VIEW")]
    threads: Option<String>,

    /// Start with the call graph on module level, optionally showing the
    /// functions of one module (e.g. `ec_task`).
    #[arg(long, num_args = 0..=1, default_missing_value = "", value_name = "MODULE")]
    callgraph: Option<String>,

    /// Show statements as data flow of their expressions from the start.
    #[arg(long)]
    expressions: bool,

    /// Write SVG and PNG files of the overviews (and of the `--select`ed
    /// function) to this directory instead of opening a window.
    #[arg(long, value_name = "DIR")]
    export: Option<PathBuf>,

    /// Write a compact Markdown report (threads, channels, shared state,
    /// modules, hot spots, names to search for) to FILE or, without FILE,
    /// to stdout, instead of opening a window. Meant as a starting point
    /// for LLM assistants and new readers.
    #[arg(long, num_args = 0..=1, default_missing_value = "-", value_name = "FILE")]
    report: Option<PathBuf>,
}

fn main() -> anyhow::Result<()> {
    attach_parent_console();
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    if let Some(out) = &cli.report {
        let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
        let text = rv_gui::report(&path)?;
        if out.as_os_str() == "-" {
            print!("{text}");
        } else {
            std::fs::write(out, text)?;
        }
        return Ok(());
    }
    if let Some(dir) = &cli.export {
        let path = cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
        for file in rv_gui::export_all(&path, dir, cli.select.as_deref())? {
            println!("{}", file.display());
        }
        return Ok(());
    }
    let editor = rv_gui::EditorCommand::parse(&cli.editor)?;
    rv_gui::run(rv_gui::Options {
        path: cli.path,
        editor,
        select: cli.select,
        threads: cli.threads,
        callgraph: cli.callgraph,
        expressions: cli.expressions,
    })
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// With the "windows" subsystem there is no console. When started from a
/// terminal, reuse that one so `--help` and log output still show up.
#[cfg(windows)]
fn attach_parent_console() {
    use windows_sys::Win32::System::Console::{ATTACH_PARENT_PROCESS, AttachConsole};
    // Fails harmlessly when there is no parent console (double click).
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_parent_console() {}
