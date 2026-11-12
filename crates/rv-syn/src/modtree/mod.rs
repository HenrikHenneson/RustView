//! Following `mod` declarations from target roots and collecting functions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rv_ir::{
    Diagnostic, FileId, FnOwner, LineIndex, Module, ModuleId, Package, Project, Severity,
    SourceFile, Target, TargetKind, TestSummary,
};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::cargo::PackageSpec;
use crate::source::{Parsed, Source, is_skipped_dir, parse, read_lossy};

mod attrs;
mod functions;

use attrs::{count_tests, is_cfg_test, is_shown, path_attr};
use functions::{FnItem, impl_owner};

pub(crate) fn build(
    root: &Path,
    specs: &[PackageSpec],
    in_memory: Option<(PathBuf, String)>,
) -> Project {
    let mut b = Builder {
        project: Project {
            root: root.to_path_buf(),
            ..Project::default()
        },
        file_ids: HashMap::new(),
        parsed: HashMap::new(),
        in_memory,
    };
    for spec in specs {
        b.package(spec);
    }
    let package_dirs: Vec<PathBuf> = specs.iter().map(|s| s.root_dir.clone()).collect();
    for (i, spec) in specs.iter().enumerate() {
        let unlinked = b.unlinked_files(spec, &package_dirs);
        b.project.packages[i].unlinked_files = unlinked;
    }
    b.project
}

struct Builder {
    project: Project,
    file_ids: HashMap<PathBuf, FileId>,
    /// Parse results stay cached while building, a file can be part of
    /// several targets (e.g. `mod foo;` from both lib.rs and main.rs).
    parsed: HashMap<FileId, Parsed>,
    /// Source given as text instead of read from disk (tests, snippets).
    in_memory: Option<(PathBuf, String)>,
}

/// Where `mod x;` and `#[path = ...] mod x;` look for files.
#[derive(Clone)]
struct DirCtx {
    child_dir: PathBuf,
    path_base: PathBuf,
}

impl DirCtx {
    /// For crate roots, `mod.rs` files and files loaded via `#[path]`.
    fn mod_rs(file: &Path) -> Self {
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        DirCtx {
            child_dir: dir.clone(),
            path_base: dir,
        }
    }

    /// For `dir/foo.rs`: children live in `dir/foo/`.
    fn non_mod_rs(file: &Path) -> Self {
        let dir = file.parent().map(Path::to_path_buf).unwrap_or_default();
        let stem = file.file_stem().unwrap_or_default();
        DirCtx {
            child_dir: dir.join(stem),
            path_base: dir,
        }
    }

    fn inline(&self, name: &str) -> Self {
        let dir = self.child_dir.join(name);
        DirCtx {
            child_dir: dir.clone(),
            path_base: dir,
        }
    }

    /// The file `mod name;` refers to (existing or not) and the context for
    /// the modules declared in it. `path_attr` is the value of `#[path]`.
    fn child_file(&self, name: &str, path_attr: Option<String>) -> (PathBuf, DirCtx) {
        if let Some(rel) = path_attr {
            let path = self.path_base.join(rel);
            let ctx = DirCtx::mod_rs(&path);
            return (path, ctx);
        }
        let flat = self.child_dir.join(format!("{name}.rs"));
        if flat.is_file() {
            let ctx = DirCtx::non_mod_rs(&flat);
            return (flat, ctx);
        }
        let nested = self.child_dir.join(name).join("mod.rs");
        let ctx = DirCtx::mod_rs(&nested);
        (nested, ctx)
    }
}

/// Name and place of a module that is about to be created.
struct ModDecl {
    name: String,
    parent: Option<ModuleId>,
    /// The `mod` item; `None` for a crate root.
    span: Option<rv_ir::Span>,
}

/// Per-target walking state.
struct Walk<'a> {
    crate_name: String,
    /// Files on the current `mod` chain, to break cycles.
    stack: Vec<PathBuf>,
    tests: &'a mut TestSummary,
}

impl Builder {
    fn package(&mut self, spec: &PackageSpec) {
        let mut tests = TestSummary::default();
        let mut targets = Vec::new();
        for t in &spec.targets {
            match t.kind {
                TargetKind::Test | TargetKind::Bench | TargetKind::Example => {
                    match t.kind {
                        TargetKind::Test => tests.test_targets += 1,
                        TargetKind::Bench => tests.bench_targets += 1,
                        _ => tests.example_targets += 1,
                    }
                    self.count_tests_in_file(&t.root_file, &mut tests);
                }
                _ => {
                    let mut walk = Walk {
                        crate_name: t.name.replace('-', "_"),
                        stack: Vec::new(),
                        tests: &mut tests,
                    };
                    let root = ModDecl {
                        name: walk.crate_name.clone(),
                        parent: None,
                        span: None,
                    };
                    let ctx = DirCtx::mod_rs(&t.root_file);
                    let root_module = self.module_file(&t.root_file, ctx, root, &mut walk);
                    targets.push(Target {
                        name: t.name.clone(),
                        kind: t.kind,
                        root_file: t.root_file.clone(),
                        root_module,
                    });
                }
            }
        }
        self.project.packages.push(Package {
            name: spec.name.clone(),
            manifest_path: spec.manifest_path.clone(),
            root_dir: spec.root_dir.clone(),
            in_workspace: spec.in_workspace,
            dependencies: spec.dependencies.clone(),
            targets,
            tests,
            unlinked_files: Vec::new(),
        });
    }

    /// Read and parse a file once; returns its id.
    fn file(&mut self, path: &Path) -> Option<FileId> {
        if let Some(&id) = self.file_ids.get(path) {
            return Some(id);
        }
        let from_memory = self
            .in_memory
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, t)| t.clone());
        let text = match from_memory.map_or_else(|| read_lossy(path), Ok) {
            Ok(t) => t,
            Err(e) => {
                self.diag(Severity::Error, format!("cannot read: {e}"), Some(path));
                return None;
            }
        };
        let id = FileId(self.project.files.len() as u32);
        let parsed = parse(text);
        let parse_error = parsed.result.as_ref().err().map(|e| {
            let at = e.span().start();
            format!("{}:{}: {e}", at.line, at.column + 1)
        });
        if let Some(msg) = &parse_error {
            self.diag(Severity::Error, format!("parse error at {msg}"), Some(path));
        }
        self.project.files.push(SourceFile {
            path: path.to_path_buf(),
            lines: LineIndex::new(&parsed.text),
            text: Arc::from(parsed.text.as_str()),
            parse_error,
        });
        self.file_ids.insert(path.to_path_buf(), id);
        self.parsed.insert(id, parsed);
        Some(id)
    }

    /// Create a module for a whole file and walk its items.
    fn module_file(
        &mut self,
        path: &Path,
        ctx: DirCtx,
        decl: ModDecl,
        walk: &mut Walk,
    ) -> Option<ModuleId> {
        if walk.stack.iter().any(|p| p == path) {
            self.diag(Severity::Warning, "module cycle".into(), Some(path));
            return None;
        }
        let file = self.file(path)?;
        let m = self.new_module(decl, file, false);
        // Temporarily take the parse result out to satisfy the borrow checker.
        let parsed = self.parsed.remove(&file)?;
        if let Ok(ast) = &parsed.result {
            let src = Source {
                parsed: &parsed,
                file,
            };
            walk.stack.push(path.to_path_buf());
            self.items(src, &ast.items, m, &ctx, walk);
            walk.stack.pop();
        }
        self.parsed.insert(file, parsed);
        Some(m)
    }

    fn new_module(&mut self, decl: ModDecl, file: FileId, inline: bool) -> ModuleId {
        let ModDecl { name, parent, span } = decl;
        let id = ModuleId(self.project.modules.len() as u32);
        let mut path = parent.map_or_else(Vec::new, |p| self.project.module(p).path.clone());
        path.push(name.clone());
        self.project.modules.push(Module {
            name,
            path,
            file,
            span,
            inline,
            parent,
            children: Vec::new(),
            functions: Vec::new(),
        });
        if let Some(p) = parent {
            self.project.modules[p.index()].children.push(id);
        }
        id
    }

    /// Walk the items of module `m`: functions, nested modules, and the
    /// struct fields and type aliases call resolution needs.
    fn items(
        &mut self,
        src: Source,
        items: &[syn::Item],
        m: ModuleId,
        ctx: &DirCtx,
        walk: &mut Walk,
    ) {
        for item in items {
            match item {
                syn::Item::Fn(f) => {
                    if is_shown(&f.attrs, walk.tests) {
                        self.function(src, m, FnOwner::Free, FnItem::of_free(f));
                    }
                }
                syn::Item::Impl(imp) => self.impl_item(src, imp, m, walk.tests),
                syn::Item::Trait(t) => self.trait_item(src, t, m),
                syn::Item::Mod(md) => self.module_item(src, md, m, ctx, walk),
                syn::Item::Struct(st) => self.struct_item(src, st),
                syn::Item::Type(t) => {
                    self.project
                        .type_aliases
                        .insert(t.ident.to_string(), src.text_of(t.ty.span()));
                }
                _ => {}
            }
        }
    }

    /// The functions of an `impl` block; tests in it are only counted.
    fn impl_item(
        &mut self,
        src: Source,
        imp: &syn::ItemImpl,
        m: ModuleId,
        tests: &mut TestSummary,
    ) {
        if is_cfg_test(&imp.attrs) {
            tests.test_fns += count_tests(|v| v.visit_item_impl(imp));
            return;
        }
        let owner = impl_owner(src.parsed, imp);
        for ii in &imp.items {
            if let syn::ImplItem::Fn(f) = ii
                && is_shown(&f.attrs, tests)
            {
                self.function(src, m, owner.clone(), FnItem::of_impl(f));
            }
        }
    }

    /// The default methods of a trait.
    fn trait_item(&mut self, src: Source, t: &syn::ItemTrait, m: ModuleId) {
        if is_cfg_test(&t.attrs) {
            return;
        }
        let owner = FnOwner::Trait {
            name: t.ident.to_string(),
        };
        for ti in &t.items {
            if let syn::TraitItem::Fn(f) = ti
                && let Some(block) = &f.default
                && !is_cfg_test(&f.attrs)
            {
                let item = FnItem {
                    sig: &f.sig,
                    block,
                    span: f.span(),
                };
                self.function(src, m, owner.clone(), item);
            }
        }
    }

    /// Remember the field types of a struct with named fields.
    fn struct_item(&mut self, src: Source, st: &syn::ItemStruct) {
        let syn::Fields::Named(named) = &st.fields else {
            return;
        };
        let fields = named
            .named
            .iter()
            .filter_map(|f| Some((f.ident.as_ref()?.to_string(), src.text_of(f.ty.span()))))
            .collect();
        self.project
            .struct_fields
            .insert(st.ident.to_string(), fields);
    }

    /// `mod name { .. }` or `mod name;`: a child module, or counted tests if
    /// it is `#[cfg(test)]`.
    fn module_item(
        &mut self,
        src: Source,
        md: &syn::ItemMod,
        parent: ModuleId,
        ctx: &DirCtx,
        walk: &mut Walk,
    ) {
        let name = syn::ext::IdentExt::unraw(&md.ident).to_string();
        let span = src.span(md.span());
        let cfg_test = is_cfg_test(&md.attrs);
        let decl = ModDecl {
            name: name.clone(),
            parent: Some(parent),
            span: Some(span.clone()),
        };
        if let Some((_, items)) = &md.content {
            if cfg_test {
                walk.tests.cfg_test_modules += 1;
                walk.tests.test_fns += count_tests(|v| v.visit_item_mod(md));
                return;
            }
            let m = self.new_module(decl, src.file, true);
            self.items(src, items, m, &ctx.inline(&name), walk);
            return;
        }

        // `mod name;` -> find the file.
        let (path, child_ctx) = ctx.child_file(&name, path_attr(&md.attrs));
        if !path.is_file() {
            if !cfg_test {
                self.project.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("file for `mod {name};` not found ({})", path.display()),
                    file: Some(self.project.file(src.file).path.clone()),
                    span: Some(span),
                });
            }
            return;
        }
        if cfg_test {
            walk.tests.cfg_test_modules += 1;
            self.count_tests_in_file(&path, walk.tests);
            return;
        }
        self.module_file(&path, child_ctx, decl, walk);
    }

    /// Count `#[test]` functions in a file that is not shown in the tree.
    fn count_tests_in_file(&mut self, path: &Path, tests: &mut TestSummary) {
        let Some(id) = self.file(path) else { return };
        if let Some(Ok(ast)) = self.parsed.get(&id).map(|p| &p.result) {
            tests.test_fns += count_tests(|v| v.visit_file(ast));
        }
    }

    /// `.rs` files below a package that no module tree reached.
    fn unlinked_files(&mut self, spec: &PackageSpec, package_dirs: &[PathBuf]) -> Vec<FileId> {
        let skip_dirs: Vec<PathBuf> = ["tests", "benches", "examples"]
            .iter()
            .map(|d| spec.root_dir.join(d))
            .chain(
                package_dirs
                    .iter()
                    .filter(|d| **d != spec.root_dir && d.starts_with(&spec.root_dir))
                    .cloned(),
            )
            .collect();
        if spec.manifest_path.extension().is_some_and(|e| e == "rs") {
            return Vec::new(); // single-file mode
        }
        let candidates: Vec<PathBuf> = walkdir::WalkDir::new(&spec.root_dir)
            .into_iter()
            .filter_entry(|e| !is_skipped_dir(e) && !skip_dirs.iter().any(|d| e.path() == d))
            .flatten()
            .filter(|e| e.file_type().is_file() && e.path().extension().is_some_and(|x| x == "rs"))
            .map(|e| e.into_path())
            .filter(|p| !self.file_ids.contains_key(p))
            .collect();
        let mut out: Vec<FileId> = candidates.iter().filter_map(|p| self.file(p)).collect();
        out.sort_by(|a, b| self.project.file(*a).path.cmp(&self.project.file(*b).path));
        out
    }

    fn diag(&mut self, severity: Severity, message: String, file: Option<&Path>) {
        self.project.diagnostics.push(Diagnostic {
            severity,
            message,
            file: file.map(Path::to_path_buf),
            span: None,
        });
    }
}
