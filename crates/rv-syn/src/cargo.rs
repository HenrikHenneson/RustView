//! Finding packages and targets via `cargo metadata`, or from the
//! `Cargo.toml` files alone where `cargo` cannot be started.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::OnceLock;

use anyhow::{Context, Result, bail};
use rv_ir::{Diagnostic, Severity, TargetKind};

use crate::source::strip_verbatim;

#[derive(Clone, Debug)]
pub struct PackageSpec {
    pub name: String,
    pub manifest_path: PathBuf,
    pub root_dir: PathBuf,
    pub in_workspace: bool,
    /// Crate names of the normal and build dependencies.
    pub dependencies: Vec<String>,
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

    let mut packages = metadata(&manifest, true)?;
    let mut diagnostics = add_stray_packages(path, &manifest, &mut packages);
    if !cargo_starts() {
        diagnostics.push(Diagnostic {
            severity: Severity::Info,
            message: "`cargo` was not found: packages and targets are read from the \
                      Cargo.toml files by Cargo's conventions"
                .to_string(),
            file: None,
            span: None,
        });
    }
    Ok((packages, diagnostics))
}

/// Adds the packages below `root` that are not workspace members
/// (stand-alone tools, excluded crates, nested workspaces). Returns a
/// warning for every manifest that could not be read.
fn add_stray_packages(
    root: &Path,
    manifest: &Path,
    packages: &mut Vec<PackageSpec>,
) -> Vec<Diagnostic> {
    let mut diagnostics = Vec::new();
    let mut known: HashSet<PathBuf> = packages.iter().map(|p| p.manifest_path.clone()).collect();
    known.insert(manifest.to_path_buf());
    for stray in find_manifests(root) {
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
    diagnostics
}

pub(crate) fn single_file(path: &Path) -> PackageSpec {
    let name = path
        .file_stem()
        .map_or("main".into(), |s| s.to_string_lossy().into_owned());
    PackageSpec {
        name: name.clone(),
        manifest_path: path.to_path_buf(),
        root_dir: path.parent().unwrap_or(path).to_path_buf(),
        in_workspace: true,
        dependencies: Vec::new(),
        targets: vec![TargetSpec {
            name,
            kind: TargetKind::Other,
            root_file: path.to_path_buf(),
        }],
    }
}

/// The `cargo` to run: the one on the `PATH` or, if there is none (a GUI
/// started without the shell's environment), the one rustup installed.
/// `None` if neither can be started.
fn cargo_program() -> Option<&'static Path> {
    static PROGRAM: OnceLock<Option<PathBuf>> = OnceLock::new();
    PROGRAM
        .get_or_init(|| {
            let starts = |program: &Path| {
                std::process::Command::new(program)
                    .arg("--version")
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok()
            };
            let home = std::env::var_os("CARGO_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    let user =
                        std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME"))?;
                    Some(PathBuf::from(user).join(".cargo"))
                });
            let installed = home.map(|h| h.join("bin").join("cargo"));
            std::iter::once(PathBuf::from("cargo"))
                .chain(installed)
                .find(|program| starts(program))
        })
        .as_deref()
}

fn cargo_starts() -> bool {
    cargo_program().is_some()
}

/// The packages of a manifest and its workspace.
fn metadata(manifest: &Path, in_workspace: bool) -> Result<Vec<PackageSpec>> {
    match cargo_program() {
        Some(cargo) => cargo_metadata(cargo, manifest, in_workspace),
        None => crate::manifest::packages(manifest, in_workspace),
    }
}

fn cargo_metadata(cargo: &Path, manifest: &Path, in_workspace: bool) -> Result<Vec<PackageSpec>> {
    let meta = cargo_metadata::MetadataCommand::new()
        .cargo_path(cargo)
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
        let dependencies = pkg
            .dependencies
            .iter()
            .filter(|d| d.kind != cargo_metadata::DependencyKind::Development)
            .map(|d| d.name.replace('-', "_"))
            .collect();
        out.push(PackageSpec {
            name: pkg.name.to_string(),
            manifest_path,
            root_dir,
            in_workspace,
            dependencies,
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

#[cfg(test)]
mod tests {
    use super::*;

    /// What both ways of finding packages have to agree on.
    fn summary(mut specs: Vec<PackageSpec>) -> Vec<String> {
        specs.sort_by(|a, b| a.name.cmp(&b.name));
        specs
            .iter()
            .map(|p| {
                let mut deps = p.dependencies.clone();
                deps.sort();
                let targets: Vec<String> = p
                    .targets
                    .iter()
                    .map(|t| format!("{:?} {} {}", t.kind, t.name, t.root_file.display()))
                    .collect();
                format!(
                    "{} {} deps {deps:?} targets {targets:#?}",
                    p.name,
                    p.manifest_path.display()
                )
            })
            .collect()
    }

    fn assert_same_without_cargo(manifest: &Path) {
        let cargo = cargo_program().expect("cargo is needed for this test");
        let with = cargo_metadata(cargo, manifest, true).unwrap();
        let without = crate::manifest::packages(manifest, true).unwrap();
        assert_eq!(summary(without), summary(with));
    }

    #[test]
    fn manifests_alone_give_the_packages_of_this_workspace() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..").join("..");
        let manifest = strip_verbatim(&root.canonicalize().unwrap()).join("Cargo.toml");
        assert_same_without_cargo(&manifest);
    }

    #[test]
    fn manifests_alone_give_the_targets_of_the_fixture() {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("modcrate")
            .join("Cargo.toml");
        assert_same_without_cargo(&manifest);
    }
}
