//! Output without a window: `rustview <path> --export <dir>` and `--report`.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rv_analysis::{CallGraph, Facts};
use rv_ir::{FnId, Project};

use crate::export;

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
            &crate::program_view::svg(&project, &program, &layout),
        )?;
        if !(program.channels.is_empty() && program.shared.is_empty() && program.joins.is_empty()) {
            write(
                &format!("{qual}_communication"),
                &crate::comm_view::svg(&program),
            )?;
        }
    }
    for crate_name in rv_analysis::crates(&project) {
        if crate_name.starts_with("build_script") {
            continue;
        }
        let (g, l, sizes) =
            crate::callgraph_view::module_layout(&project, &calls, &facts, &crate_name, 2);
        write(
            &format!("{crate_name}_modules"),
            &crate::callgraph_view::module_svg(&crate_name, &g, &l, &sizes),
        )?;
    }
    if let Some(wanted) = select {
        let f = find_function(&project, wanted)
            .with_context(|| format!("no function matches `{wanted}`"))?;
        let svg = crate::diagram::function_svg(&project, f).context("function has no body")?;
        write(&project.function(f).qual_path, &svg)?;
    }
    Ok(written)
}

/// The Markdown project report (see [`rv_analysis::report`]).
pub fn report(path: &Path) -> Result<String> {
    let project = rv_syn::load(path)?;
    Ok(rv_analysis::report(&project))
}

/// Exact qualified path, or the shortest one ending in `::wanted`.
fn find_function(project: &Project, wanted: &str) -> Option<FnId> {
    let suffix = format!("::{wanted}");
    project
        .functions
        .iter()
        .enumerate()
        .filter(|(_, f)| f.qual_path == wanted || f.qual_path.ends_with(&suffix))
        .min_by_key(|(_, f)| f.qual_path.len())
        .map(|(i, _)| FnId(i as u32))
}
