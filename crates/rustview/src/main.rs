//! RustView: LabVIEW-style structural view of Rust code (analysis and display only).

// Release builds started by double click should not open a console window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::path::{Path, PathBuf};

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(version, about, args_conflicts_with_subcommands = true)]
struct Cli {
    #[command(subcommand)]
    command: Option<Cmd>,

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

    /// Start with the Report or Maintainability tab.
    #[arg(long, value_enum, value_name = "TAB")]
    tab: Option<Tab>,

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

    /// Write the maintainability of the project as Markdown to FILE or, without
    /// FILE, to stdout, instead of opening a window: score, and the
    /// functions, files and module cycles over the targets.
    #[arg(long, num_args = 0..=1, default_missing_value = "-", value_name = "FILE")]
    maintainability: Option<PathBuf>,

    /// With `--maintainability`: exit with status 1 if a function or file is over a
    /// limit or modules call each other in a cycle (for CI).
    #[arg(long, requires = "maintainability")]
    gate: bool,
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum Tab {
    Report,
    Maintainability,
}

#[derive(Subcommand)]
enum Cmd {
    /// Run as MCP server over stdin/stdout for AI coding assistants (e.g.
    /// `claude mcp add rustview -- rustview mcp`). Answers questions about
    /// threads, channels, shared state, callers and the nesting of functions.
    Mcp {
        /// Cargo project directory; default: the current directory.
        path: Option<PathBuf>,
    },
}

fn main() -> anyhow::Result<()> {
    // As MCP server stdin/stdout are the pipes to the client: no console.
    let mcp = std::env::args().nth(1).as_deref() == Some("mcp");
    if !mcp {
        attach_parent_console();
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();
    let cli = Cli::parse();
    if let Some(Cmd::Mcp { path }) = cli.command {
        return rv_mcp::serve(path.unwrap_or_else(|| PathBuf::from(".")));
    }
    let project_path = || cli.path.clone().unwrap_or_else(|| PathBuf::from("."));
    if let Some(out) = &cli.report {
        let project = rv_syn::load(&project_path())?;
        return write_text(out, &rv_analysis::report(&project));
    }
    if let Some(out) = &cli.maintainability {
        return maintainability(&project_path(), out, cli.gate);
    }
    if let Some(dir) = &cli.export {
        for file in rv_gui::export_all(&project_path(), dir, cli.select.as_deref())? {
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
        tab: cli.tab.map(|t| match t {
            Tab::Report => rv_gui::StartTab::Report,
            Tab::Maintainability => rv_gui::StartTab::Maintainability,
        }),
    })
    .map_err(|e| anyhow::anyhow!("{e}"))
}

/// `--maintainability`: writes the maintainability report; with `gate`, findings over a limit
/// end the process with status 1.
fn maintainability(path: &Path, out: &Path, gate: bool) -> anyhow::Result<()> {
    let project = rv_syn::load(path)?;
    let calls = rv_analysis::CallGraph::new(&project);
    let maintainability = rv_analysis::Maintainability::new(&project, &calls);
    write_text(
        out,
        &rv_analysis::maintainability_report(&project, &maintainability),
    )?;
    let violations = maintainability.violations();
    if gate && violations > 0 {
        eprintln!("maintainability: {violations} finding(s) over a limit");
        std::process::exit(1);
    }
    Ok(())
}

/// Writes to `out`, or to stdout for `-`.
fn write_text(out: &Path, text: &str) -> anyhow::Result<()> {
    if out.as_os_str() == "-" {
        print!("{text}");
    } else {
        std::fs::write(out, text)?;
    }
    Ok(())
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
