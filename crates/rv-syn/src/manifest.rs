//! Finding packages and targets by reading `Cargo.toml` files directly, for
//! machines where `cargo` cannot be started. Follows Cargo's conventions
//! (`src/lib.rs`, `src/main.rs`, `src/bin/*`, `build.rs`, `tests/`, ...) and
//! the `[lib]`, `[[bin]]` and `[workspace]` tables; anything more unusual is
//! what `cargo metadata` is for.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rv_ir::TargetKind;
use toml::{Table, Value};

use crate::cargo::{PackageSpec, TargetSpec};

/// The package of `manifest` and the members of its workspace.
pub(crate) fn packages(manifest: &Path, in_workspace: bool) -> Result<Vec<PackageSpec>> {
    let root = read(manifest)?;
    let dir = manifest.parent().unwrap_or(manifest);
    let mut out = Vec::new();
    out.extend(package(manifest, &root, in_workspace));
    for member in members(&root, dir) {
        if member == manifest {
            continue;
        }
        // A member that cannot be read is left out, like a missing file.
        if let Ok(table) = read(&member) {
            out.extend(package(&member, &table, in_workspace));
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn read(manifest: &Path) -> Result<Table> {
    let text = std::fs::read_to_string(manifest)
        .with_context(|| format!("reading {}", manifest.display()))?;
    text.parse::<Table>()
        .with_context(|| format!("parsing {}", manifest.display()))
}

/// `dir` extended by a path written with `/` in a manifest.
fn join(dir: &Path, relative: &str) -> PathBuf {
    relative
        .split(['/', '\\'])
        .filter(|part| !part.is_empty() && *part != ".")
        .fold(dir.to_path_buf(), |path, part| path.join(part))
}

fn strings<'t>(table: &'t Table, key: &str) -> impl Iterator<Item = &'t str> {
    table
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
}

/// Manifests of the workspace members: `members` entries (a trailing `*`
/// stands for every directory) without the `exclude` entries.
fn members(root: &Table, dir: &Path) -> Vec<PathBuf> {
    let Some(workspace) = root.get("workspace").and_then(Value::as_table) else {
        return Vec::new();
    };
    let excluded: Vec<PathBuf> = strings(workspace, "exclude")
        .map(|e| join(dir, e))
        .collect();
    let mut dirs: Vec<PathBuf> = Vec::new();
    for member in strings(workspace, "members") {
        match member.strip_suffix('*') {
            Some(prefix) => dirs.extend(subdirs(&join(dir, prefix))),
            None => dirs.push(join(dir, member)),
        }
    }
    dirs.retain(|d| !excluded.contains(d));
    let mut manifests: Vec<PathBuf> = dirs
        .into_iter()
        .map(|d| d.join("Cargo.toml"))
        .filter(|m| m.is_file())
        .collect();
    manifests.sort();
    manifests.dedup();
    manifests
}

fn subdirs(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect()
}

/// The package a manifest declares; `None` for a pure workspace manifest.
fn package(manifest: &Path, table: &Table, in_workspace: bool) -> Option<PackageSpec> {
    let package = table.get("package")?.as_table()?;
    let name = package.get("name")?.as_str()?.to_string();
    let root_dir = manifest.parent().unwrap_or(manifest).to_path_buf();
    let mut targets = Targets {
        dir: &root_dir,
        found: Vec::new(),
    };
    targets.lib(table, &name);
    targets.bins(table, package, &name);
    targets.build_script(package);
    targets.directory("tests", TargetKind::Test);
    targets.directory("benches", TargetKind::Bench);
    targets.directory("examples", TargetKind::Example);
    let mut targets = targets.found;
    targets.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    Some(PackageSpec {
        name,
        manifest_path: manifest.to_path_buf(),
        root_dir,
        in_workspace,
        dependencies: dependencies(table),
        targets,
    })
}

/// Crate names of the normal and build dependencies, including those of
/// `[target.'cfg(..)'.dependencies]` (`package = ".."` names the real
/// package of a renamed one).
fn dependencies(table: &Table) -> Vec<String> {
    let per_target = table.get("target").and_then(Value::as_table);
    let per_target = per_target
        .into_iter()
        .flat_map(|targets| targets.values())
        .filter_map(Value::as_table);
    let mut names = Vec::new();
    for owner in std::iter::once(table).chain(per_target) {
        for section in ["dependencies", "build-dependencies"] {
            let deps = owner.get(section).and_then(Value::as_table);
            for (key, value) in deps.into_iter().flatten() {
                let name = value.get("package").and_then(Value::as_str).unwrap_or(key);
                names.push(name.replace('-', "_"));
            }
        }
    }
    names
}

/// Collects the targets of one package.
struct Targets<'d> {
    dir: &'d Path,
    found: Vec<TargetSpec>,
}

impl Targets<'_> {
    /// Adds a target if its root file exists and is not a target already.
    fn add(&mut self, name: &str, kind: TargetKind, root_file: PathBuf) {
        if root_file.is_file() && !self.found.iter().any(|t| t.root_file == root_file) {
            self.found.push(TargetSpec {
                name: name.to_string(),
                kind,
                root_file,
            });
        }
    }

    fn lib(&mut self, table: &Table, package: &str) {
        let lib = table.get("lib").and_then(Value::as_table);
        let text = |key: &str| lib.and_then(|l| l.get(key)).and_then(Value::as_str);
        let name = text("name").map_or(package.replace('-', "_"), str::to_string);
        let path = join(self.dir, text("path").unwrap_or("src/lib.rs"));
        self.add(&name, TargetKind::Lib, path);
    }

    /// `[[bin]]` entries, then `src/main.rs` and `src/bin/` unless
    /// `autobins = false`.
    fn bins(&mut self, table: &Table, package: &Table, package_name: &str) {
        let declared = table.get("bin").and_then(Value::as_array);
        for bin in declared.into_iter().flatten() {
            let Some(name) = bin.get("name").and_then(Value::as_str) else {
                continue;
            };
            let path = match bin.get("path").and_then(Value::as_str) {
                Some(path) => join(self.dir, path),
                None => self.default_bin(name, package_name),
            };
            self.add(name, TargetKind::Bin, path);
        }
        if package.get("autobins").and_then(Value::as_bool) != Some(false) {
            self.add(package_name, TargetKind::Bin, join(self.dir, "src/main.rs"));
            self.directory("src/bin", TargetKind::Bin);
        }
    }

    /// Where Cargo looks for a `[[bin]]` without `path`.
    fn default_bin(&self, name: &str, package_name: &str) -> PathBuf {
        let file = join(self.dir, &format!("src/bin/{name}.rs"));
        let nested = join(self.dir, &format!("src/bin/{name}/main.rs"));
        if file.is_file() {
            file
        } else if nested.is_file() || name != package_name {
            nested
        } else {
            join(self.dir, "src/main.rs")
        }
    }

    /// `build = "path"`, or `build.rs` unless `build = false`.
    fn build_script(&mut self, package: &Table) {
        let path = match package.get("build") {
            Some(Value::String(path)) => path.as_str(),
            Some(Value::Boolean(false)) => return,
            _ => "build.rs",
        };
        self.add(
            "build-script-build",
            TargetKind::BuildScript,
            join(self.dir, path),
        );
    }

    /// Every `name.rs` and `name/main.rs` in a directory is a target.
    fn directory(&mut self, relative: &str, kind: TargetKind) {
        let Ok(entries) = std::fs::read_dir(join(self.dir, relative)) else {
            return;
        };
        let mut paths: Vec<PathBuf> = entries.flatten().map(|e| e.path()).collect();
        paths.sort();
        for path in paths {
            let Some(name) = path.file_stem().map(|s| s.to_string_lossy().into_owned()) else {
                continue;
            };
            if path.is_dir() {
                self.add(&name, kind, path.join("main.rs"));
            } else if path.extension().is_some_and(|e| e == "rs") {
                self.add(&name, kind, path);
            }
        }
    }
}
