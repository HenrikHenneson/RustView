//! Output without a window: `rustview <path> --export <dir>`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rv_analysis::{CallGraph, Facts};

use crate::callgraph_view::{ModuleLayout, module_svg};
use crate::comm_view::Matrix;
use crate::export;
use crate::lookup::find_function;

/// Module depth of the exported module graphs (the default of the window).
const MODULE_DEPTH: usize = 2;

/// Writes SVG and PNG files of the thread graph and communication matrix of
/// every binary, the module graph of every crate and, with `select`, the
/// diagram of that function. Returns the written files.
pub fn export_all(path: &Path, dir: &Path, select: Option<&str>) -> Result<Vec<PathBuf>> {
    let project = rv_syn::load(path)?;
    std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    let facts = Facts::new(&project);
    let calls = CallGraph::new(&project);
    let mut written = Vec::new();
    let mut write = |name: &str, svg: &str| -> Result<()> {
        for ext in ["svg", "png"] {
            let file = dir.join(format!("{}.{ext}", export::file_name(name)));
            export::save(svg, &file)?;
            written.push(file);
        }
        Ok(())
    };

    for entry in rv_analysis::entry_points(&project) {
        let qual = &project.function(entry).qual_path;
        if qual.starts_with("build_script") {
            continue;
        }
        let program = rv_analysis::program(&project, &facts, &calls, entry);
        let layout = crate::program_view::layout(&project, &program);
        write(
            &format!("{qual}_threads"),
            &crate::program_view::graph_svg(&layout),
        )?;
        if !(program.channels.is_empty() && program.shared.is_empty() && program.joins.is_empty()) {
            write(
                &format!("{qual}_communication"),
                &crate::comm_view::matrix_svg(&program, &Matrix::new(&program)),
            )?;
        }
    }
    for crate_name in rv_analysis::crates(&project) {
        if crate_name.starts_with("build_script") {
            continue;
        }
        let layout = ModuleLayout::new(&project, &calls, &facts, &crate_name, MODULE_DEPTH);
        write(&format!("{crate_name}_modules"), &module_svg(&layout))?;
    }
    if let Some(wanted) = select {
        let f = find_function(&project, wanted)
            .with_context(|| format!("no function matches `{wanted}`"))?;
        let svg = crate::diagram::function_svg(&project, f).context("function has no body")?;
        write(&project.function(f).qual_path, &svg)?;
    }
    Ok(written)
}
