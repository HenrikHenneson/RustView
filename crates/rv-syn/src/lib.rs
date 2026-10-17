//! `syn`-based frontend for RustView.
//!
//! Purely syntactic: macros are not expanded, `cfg`s are not evaluated (except
//! that `#[cfg(test)]` code is counted instead of shown) and no names or types
//! are resolved.

use std::path::{Path, PathBuf};

use anyhow::Result;
use rv_ir::Project;

mod cargo;
mod modtree;
mod source;

pub use cargo::{PackageSpec, TargetSpec};

/// Load the project at `path`: a directory containing `Cargo.toml` (package or
/// workspace) or a single `.rs` file.
pub fn load(path: &Path) -> Result<Project> {
    let path = normalize_path(path);
    let (specs, diagnostics) = cargo::discover(&path)?;
    let mut project = modtree::build(&path, &specs);
    project.diagnostics.splice(0..0, diagnostics);
    // proc-macro2 keeps a thread-local map of every parsed file for span
    // lookups. All spans have been converted to rv-ir spans by now.
    proc_macro2::extra::invalidate_current_thread_spans();
    Ok(project)
}

/// Absolute path without the `\\?\` prefix `canonicalize` adds on Windows.
pub fn normalize_path(path: &Path) -> PathBuf {
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    source::strip_verbatim(&path)
}

/// Load from already discovered package specs (no `cargo` invocation).
pub fn load_specs(root: &Path, specs: &[PackageSpec]) -> Project {
    let project = modtree::build(root, specs);
    proc_macro2::extra::invalidate_current_thread_spans();
    project
}
