//! Finding packages and targets via `cargo metadata`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use rv_ir::{Diagnostic, Severity, TargetKind};

use crate::source::strip_verbatim;

#[derive(Clone, Debug)]
pub struct PackageSpec {
    pub name: String,
    pub manifest_path: PathBuf,
    pub root_dir: PathBuf,
    pub in_workspace: bool,
    pub targets: Vec<TargetSpec>,
}

#[derive(Clone, Debug)]
pub struct TargetSpec {
    pub name: String,
    pub kind: TargetKind,
    pub root_file: PathBuf,
}

pub(crate) fn discover(path: &Path) -> Result<(Vec<PackageSpec>, Vec<Diagnostic>)> {
    if path.is_file() {
        if path.extension().is_some_and(|e| e == "rs") {
            return Ok((vec![single_file(path)], Vec::new()));
        }
        if path.file_name().is_some_and(|n| n == "Cargo.toml") {
            return discover(path.parent().unwrap_or(path));
        }
        bail!(
            "{} is neither a .rs file nor a Cargo project",
            path.display()
        );
    }
    let manifest = path.join("Cargo.toml");
    if !manifest.is_file() {
        bail!("no Cargo.toml in {}", path.display());
    }

    let mut diagnostics = Vec::new();
    let mut packages = metadata(&manifest, true)?;

    // Packages below the opened folder that are not workspace members
    // (stand-alone tools, excluded crates, nested workspaces).
    let mut known: HashSet<PathBuf> = packages.iter().map(|p| p.manifest_path.clone()).collect();
    known.insert(manifest.clone());
    for stray in find_manifests(path) {
        if known.contains(&stray) {
            continue;
        }
        match metadata(&stray, false) {
            Ok(found) => {
                for p in found {
                    if known.insert(p.manifest_path.clone()) {
                        packages.push(p);
                    }
                }
            }
            Err(e) => diagnostics.push(Diagnostic {
                severity: Severity::Warning,
                message: format!("skipped package: {e:#}"),
                file: Some(stray.clone()),
                span: None,
            }),
        }
        known.insert(stray);
    }
    Ok((packages, diagnostics))
}

fn single_file(path: &Path) -> PackageSpec {
    let name = path
        .file_stem()
        .map_or("main".into(), |s| s.to_string_lossy().into_owned());
    PackageSpec {
        name: name.clone(),
        manifest_path: path.to_path_buf(),
        root_dir: path.parent().unwrap_or(path).to_path_buf(),
        in_workspace: true,
        targets: vec![TargetSpec {
            name,
            kind: TargetKind::Other,
            root_file: path.to_path_buf(),
        }],
    }
}

fn metadata(manifest: &Path, in_workspace: bool) -> Result<Vec<PackageSpec>> {
    let meta = cargo_metadata::MetadataCommand::new()
        .manifest_path(manifest)
        .no_deps()
        .other_options(vec!["--offline".to_string()])
        .exec()
        .with_context(|| format!("cargo metadata for {}", manifest.display()))?;

    let mut out = Vec::new();
    for pkg in &meta.packages {
        let manifest_path = strip_verbatim(pkg.manifest_path.as_std_path());
        let root_dir = manifest_path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_default();
        let mut targets: Vec<TargetSpec> = pkg
            .targets
            .iter()
            .map(|t| TargetSpec {
                name: t.name.clone(),
                kind: target_kind(&t.kind),
                root_file: strip_verbatim(t.src_path.as_std_path()),
            })
            .collect();
        targets.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
        out.push(PackageSpec {
            name: pkg.name.to_string(),
            manifest_path,
            root_dir,
            in_workspace,
            targets,
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn target_kind(kinds: &[cargo_metadata::TargetKind]) -> TargetKind {
    use cargo_metadata::TargetKind as K;
    let Some(kind) = kinds.first() else {
        return TargetKind::Other;
    };
    match kind {
        K::Lib | K::RLib | K::DyLib | K::CDyLib | K::StaticLib | K::ProcMacro => TargetKind::Lib,
        K::Bin => TargetKind::Bin,
        K::CustomBuild => TargetKind::BuildScript,
        K::Example => TargetKind::Example,
        K::Test => TargetKind::Test,
        K::Bench => TargetKind::Bench,
        _ => TargetKind::Other,
    }
}

/// All `Cargo.toml` files below `root`, skipping build output and hidden dirs.
fn find_manifests(root: &Path) -> Vec<PathBuf> {
    walkdir::WalkDir::new(root)
        .into_iter()
        .filter_entry(|e| !crate::source::is_skipped_dir(e))
        .flatten()
        .filter(|e| e.file_type().is_file() && e.file_name() == "Cargo.toml")
        .map(|e| e.into_path())
        .collect()
}
