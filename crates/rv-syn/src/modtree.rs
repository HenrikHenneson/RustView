//! Following `mod` declarations from target roots and collecting functions.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rv_ir::{
    Diagnostic, FileId, FnId, FnOwner, Function, LineIndex, Module, ModuleId, Package, Param,
    ParamMode, Project, Severity, SourceFile, Target, TargetKind, TestSummary,
};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::cargo::PackageSpec;
use crate::source::{Parsed, is_skipped_dir, parse, read_lossy};

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
                    let root_module = self.module_file(
                        &t.root_file,
                        DirCtx::mod_rs(&t.root_file),
                        walk.crate_name.clone(),
                        None,
                        None,
                        &mut walk,
                    );
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
        name: String,
        parent: Option<ModuleId>,
        decl: Option<rv_ir::Span>,
        walk: &mut Walk,
    ) -> Option<ModuleId> {
        if walk.stack.iter().any(|p| p == path) {
            self.diag(Severity::Warning, "module cycle".into(), Some(path));
            return None;
        }
        let file = self.file(path)?;
        let m = self.new_module(name, file, parent, decl, false);
        // Temporarily take the parse result out to satisfy the borrow checker.
        let parsed = self.parsed.remove(&file)?;
        if let Ok(ast) = &parsed.result {
            walk.stack.push(path.to_path_buf());
            self.items(&parsed, file, &ast.items, m, &ctx, walk);
            walk.stack.pop();
        }
        self.parsed.insert(file, parsed);
        Some(m)
    }

    fn new_module(
        &mut self,
        name: String,
        file: FileId,
        parent: Option<ModuleId>,
        span: Option<rv_ir::Span>,
        inline: bool,
    ) -> ModuleId {
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

    fn items(
        &mut self,
        p: &Parsed,
        file: FileId,
        items: &[syn::Item],
        m: ModuleId,
        ctx: &DirCtx,
        walk: &mut Walk,
    ) {
        for item in items {
            match item {
                syn::Item::Fn(f) => {
                    if has_test_attr(&f.attrs) {
                        walk.tests.test_fns += 1;
                    } else if !is_cfg_test(&f.attrs) {
                        let span = p.span(file, f.span());
                        self.function(p, file, m, FnOwner::Free, &f.sig, Some(&f.block), span);
                    }
                }
                syn::Item::Impl(imp) => {
                    if is_cfg_test(&imp.attrs) {
                        walk.tests.test_fns += count_tests(|v| v.visit_item_impl(imp));
                        continue;
                    }
                    let owner = FnOwner::Impl {
                        self_ty: p.text_of(imp.self_ty.span()),
                        trait_: imp.trait_.as_ref().map(|(path, _)| p.text_of(path.span())),
                    };
                    for ii in &imp.items {
                        if let syn::ImplItem::Fn(f) = ii {
                            if has_test_attr(&f.attrs) {
                                walk.tests.test_fns += 1;
                            } else if !is_cfg_test(&f.attrs) {
                                let span = p.span(file, f.span());
                                self.function(
                                    p,
                                    file,
                                    m,
                                    owner.clone(),
                                    &f.sig,
                                    Some(&f.block),
                                    span,
                                );
                            }
                        }
                    }
                }
                syn::Item::Trait(t) => {
                    if is_cfg_test(&t.attrs) {
                        continue;
                    }
                    let owner = FnOwner::Trait {
                        name: t.ident.to_string(),
                    };
                    for ti in &t.items {
                        if let syn::TraitItem::Fn(f) = ti
                            && let Some(block) = &f.default
                            && !is_cfg_test(&f.attrs)
                        {
                            let span = p.span(file, f.span());
                            self.function(p, file, m, owner.clone(), &f.sig, Some(block), span);
                        }
                    }
                }
                syn::Item::Mod(md) => self.module_item(p, file, md, m, ctx, walk),
                syn::Item::Type(t) => {
                    self.project
                        .type_aliases
                        .insert(t.ident.to_string(), p.text_of(t.ty.span()));
                }
                _ => {}
            }
        }
    }

    fn module_item(
        &mut self,
        p: &Parsed,
        file: FileId,
        md: &syn::ItemMod,
        parent: ModuleId,
        ctx: &DirCtx,
        walk: &mut Walk,
    ) {
        let name = md.ident.unraw().to_string();
        let span = p.span(file, md.span());
        let cfg_test = is_cfg_test(&md.attrs);
        if let Some((_, items)) = &md.content {
            if cfg_test {
                walk.tests.cfg_test_modules += 1;
                walk.tests.test_fns += count_tests(|v| v.visit_item_mod(md));
                return;
            }
            let m = self.new_module(name.clone(), file, Some(parent), Some(span), true);
            self.items(p, file, items, m, &ctx.inline(&name), walk);
            return;
        }

        // `mod name;` -> find the file.
        let (path, child_ctx) = if let Some(rel) = path_attr(&md.attrs) {
            let path = ctx.path_base.join(rel);
            let c = DirCtx::mod_rs(&path);
            (path, c)
        } else {
            let flat = ctx.child_dir.join(format!("{name}.rs"));
            let nested = ctx.child_dir.join(&name).join("mod.rs");
            if flat.is_file() {
                let c = DirCtx::non_mod_rs(&flat);
                (flat, c)
            } else {
                let c = DirCtx::mod_rs(&nested);
                (nested, c)
            }
        };
        if !path.is_file() {
            if !cfg_test {
                self.project.diagnostics.push(Diagnostic {
                    severity: Severity::Warning,
                    message: format!("file for `mod {name};` not found ({})", path.display()),
                    file: Some(self.project.file(file).path.clone()),
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
        self.module_file(&path, child_ctx, name, Some(parent), Some(span), walk);
    }

    #[allow(clippy::too_many_arguments)]
    fn function(
        &mut self,
        p: &Parsed,
        file: FileId,
        m: ModuleId,
        owner: FnOwner,
        sig: &syn::Signature,
        block: Option<&syn::Block>,
        span: rv_ir::Span,
    ) -> FnId {
        let id = FnId(self.project.functions.len() as u32);
        let name = sig.ident.unraw().to_string();
        let name_span = p.span(file, sig.ident.span());
        let body =
            block.map(|b| crate::body::lower(p, file, sig, b, span.clone(), name_span.clone()));
        let qual_path = match &owner {
            FnOwner::Free => format!("{}::{name}", self.project.module(m).qual_name()),
            FnOwner::Impl { self_ty, .. } => format!(
                "{}::{}::{name}",
                self.project.module(m).qual_name(),
                strip_generics(self_ty)
            ),
            FnOwner::Trait { name: t } => {
                format!("{}::{t}::{name}", self.project.module(m).qual_name())
            }
            FnOwner::Nested { parent } => {
                format!("{}::{name}", self.project.function(*parent).qual_path)
            }
        };
        self.project.functions.push(Function {
            name,
            qual_path,
            module: m,
            owner,
            sig: signature(p, sig),
            span,
            name_span,
            body_span: block.map(|b| p.span(file, b.span())),
            body,
        });
        self.project.modules[m.index()].functions.push(id);
        if let Some(block) = block {
            self.nested_items(p, file, m, id, block);
        }
        id
    }

    /// Functions and impls declared at the top level of a function body.
    fn nested_items(
        &mut self,
        p: &Parsed,
        file: FileId,
        m: ModuleId,
        parent: FnId,
        block: &syn::Block,
    ) {
        for stmt in &block.stmts {
            let syn::Stmt::Item(item) = stmt else {
                continue;
            };
            match item {
                syn::Item::Fn(f) if !has_test_attr(&f.attrs) => {
                    let span = p.span(file, f.span());
                    self.function(
                        p,
                        file,
                        m,
                        FnOwner::Nested { parent },
                        &f.sig,
                        Some(&f.block),
                        span,
                    );
                }
                syn::Item::Impl(imp) => {
                    let owner = FnOwner::Impl {
                        self_ty: p.text_of(imp.self_ty.span()),
                        trait_: imp.trait_.as_ref().map(|(path, _)| p.text_of(path.span())),
                    };
                    for ii in &imp.items {
                        if let syn::ImplItem::Fn(f) = ii {
                            let span = p.span(file, f.span());
                            self.function(p, file, m, owner.clone(), &f.sig, Some(&f.block), span);
                        }
                    }
                }
                _ => {}
            }
        }
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

trait UnrawIdent {
    fn unraw(&self) -> syn::Ident;
}

impl UnrawIdent for syn::Ident {
    fn unraw(&self) -> syn::Ident {
        syn::ext::IdentExt::unraw(self)
    }
}

fn signature(p: &Parsed, sig: &syn::Signature) -> rv_ir::Signature {
    let params = sig
        .inputs
        .iter()
        .map(|arg| match arg {
            syn::FnArg::Receiver(r) => {
                let mode = match &r.kind {
                    syn::ReceiverKind::Reference(_, _, Some(_)) => ParamMode::RefMut,
                    syn::ReceiverKind::Reference(_, _, None) => ParamMode::Ref,
                    _ => ParamMode::Value,
                };
                Param {
                    pat: "self".into(),
                    ty: p.text_of(r.span()),
                    mode,
                }
            }
            syn::FnArg::Typed(t) => {
                let mode = match &*t.ty {
                    syn::Type::Reference(r) if r.mutability.is_some() => ParamMode::RefMut,
                    syn::Type::Reference(_) => ParamMode::Ref,
                    _ => ParamMode::Value,
                };
                Param {
                    pat: p.text_of(t.pat.span()),
                    ty: p.text_of(t.ty.span()),
                    mode,
                }
            }
        })
        .collect();
    rv_ir::Signature {
        text: p.text_of(sig.span()),
        is_async: sig.asyncness.is_some(),
        is_unsafe: matches!(sig.safety, syn::Safety::Unsafe(_)),
        is_const: sig.constness.is_some(),
        params,
        ret: match &sig.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, ty) => Some(p.text_of(ty.span())),
        },
    }
}

fn strip_generics(ty: &str) -> &str {
    ty.split('<').next().unwrap_or(ty).trim()
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| {
        if !a.path().is_ident("path") {
            return None;
        }
        let syn::Meta::NameValue(nv) = &a.meta else {
            return None;
        };
        match &nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Some(s.value()),
            _ => None,
        }
    })
}

/// `#[cfg(test)]`, `#[cfg(all(test, ...))]` — but not `#[cfg(not(test))]`.
fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let syn::Meta::List(list) = &a.meta else {
            return false;
        };
        cfg_mentions_test(list.tokens.clone())
    })
}

fn cfg_mentions_test(tokens: proc_macro2::TokenStream) -> bool {
    let mut prev_not = false;
    for tt in tokens {
        match tt {
            proc_macro2::TokenTree::Ident(i) if i == "test" => return true,
            proc_macro2::TokenTree::Ident(i) => prev_not = i == "not",
            proc_macro2::TokenTree::Group(g) => {
                if !prev_not && cfg_mentions_test(g.stream()) {
                    return true;
                }
                prev_not = false;
            }
            _ => {}
        }
    }
    false
}

/// `#[test]`, `#[tokio::test]`, `#[rstest]`-free heuristic: last path segment is `test`.
fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "test"))
}

fn count_tests(f: impl FnOnce(&mut TestCounter)) -> u32 {
    let mut v = TestCounter(0);
    f(&mut v);
    v.0
}

struct TestCounter(u32);

impl<'ast> Visit<'ast> for TestCounter {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if has_test_attr(&f.attrs) {
            self.0 += 1;
        }
        syn::visit::visit_item_fn(self, f);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if has_test_attr(&f.attrs) {
            self.0 += 1;
        }
        syn::visit::visit_impl_item_fn(self, f);
    }
}
