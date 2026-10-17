//! Backend-independent intermediate representation of a Rust project.
//!
//! Nothing in here knows about `syn`. A frontend (currently `rv-syn`, later
//! possibly a rust-analyzer based one) produces a [`Project`]; analysis,
//! layout and GUI only consume it.
//!
//! All collections are flat arenas indexed by typed ids, so that views can
//! hold cheap, copyable references into the model.

use std::path::PathBuf;
use std::sync::Arc;

mod body;
mod span;

pub use body::{
    Body, Exit, ExitKind, FrameRole, Node, NodeId, NodeKind, StmtKind, StructKind, UseMode, VarDef,
    VarUse,
};
pub use span::{LineCol, LineIndex, Span};

macro_rules! id_type {
    ($($(#[$m:meta])* $name:ident),* $(,)?) => {$(
        $(#[$m])*
        #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u32);

        impl $name {
            pub fn index(self) -> usize {
                self.0 as usize
            }
        }
    )*};
}

id_type!(
    /// Index into [`Project::files`].
    FileId,
    /// Index into [`Project::modules`].
    ModuleId,
    /// Index into [`Project::functions`].
    FnId,
);

/// A loaded Cargo project (workspace, single package or single file).
#[derive(Debug, Default)]
pub struct Project {
    /// The path the user asked to open.
    pub root: PathBuf,
    pub packages: Vec<Package>,
    pub files: Vec<SourceFile>,
    pub modules: Vec<Module>,
    pub functions: Vec<Function>,
    /// Problems found while loading (parse errors, missing module files, ...).
    pub diagnostics: Vec<Diagnostic>,
}

impl Project {
    pub fn file(&self, id: FileId) -> &SourceFile {
        &self.files[id.index()]
    }

    pub fn module(&self, id: ModuleId) -> &Module {
        &self.modules[id.index()]
    }

    pub fn function(&self, id: FnId) -> &Function {
        &self.functions[id.index()]
    }

    pub fn file_by_path(&self, path: &std::path::Path) -> Option<FileId> {
        self.files
            .iter()
            .position(|f| f.path == path)
            .map(|i| FileId(i as u32))
    }

    /// Source text covered by `span`.
    pub fn snippet(&self, span: &Span) -> &str {
        let text = &self.file(span.file).text;
        text.get(span.bytes.start as usize..span.bytes.end as usize)
            .unwrap_or("")
    }
}

/// One Cargo package.
#[derive(Debug)]
pub struct Package {
    pub name: String,
    pub manifest_path: PathBuf,
    pub root_dir: PathBuf,
    /// `false` for packages found below the opened folder that are not
    /// members of its workspace (e.g. stand-alone tools).
    pub in_workspace: bool,
    /// Targets that are shown as module trees (lib, bins, build script).
    pub targets: Vec<Target>,
    /// Tests, benches and examples are only counted, not shown in detail.
    pub tests: TestSummary,
    /// `.rs` files below the package that no target's module tree includes.
    pub unlinked_files: Vec<FileId>,
}

#[derive(Debug)]
pub struct Target {
    pub name: String,
    pub kind: TargetKind,
    pub root_file: PathBuf,
    /// `None` if the root file could not be read or parsed.
    pub root_module: Option<ModuleId>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum TargetKind {
    Lib,
    Bin,
    BuildScript,
    Example,
    Test,
    Bench,
    Other,
}

impl TargetKind {
    pub fn label(self) -> &'static str {
        match self {
            TargetKind::Lib => "lib",
            TargetKind::Bin => "bin",
            TargetKind::BuildScript => "build.rs",
            TargetKind::Example => "example",
            TargetKind::Test => "test",
            TargetKind::Bench => "bench",
            TargetKind::Other => "other",
        }
    }
}

/// Counts of test-only code, which is deliberately not shown in the tree.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TestSummary {
    /// `#[test]` functions, wherever they are.
    pub test_fns: u32,
    /// `#[cfg(test)]` modules inside the shown targets.
    pub cfg_test_modules: u32,
    /// Integration test targets (`tests/*.rs`).
    pub test_targets: u32,
    pub bench_targets: u32,
    pub example_targets: u32,
}

impl TestSummary {
    pub fn is_empty(&self) -> bool {
        *self == TestSummary::default()
    }
}

#[derive(Debug)]
pub struct SourceFile {
    pub path: PathBuf,
    pub text: Arc<str>,
    pub lines: LineIndex,
    /// Set if the file could not be parsed; the text is still available.
    pub parse_error: Option<String>,
}

#[derive(Debug)]
pub struct Module {
    pub name: String,
    /// Full path including the crate name, e.g. `["mtd", "ec", "task"]`.
    pub path: Vec<String>,
    pub file: FileId,
    /// Span of the `mod` item; `None` for a target's root module.
    pub span: Option<Span>,
    /// `mod foo { ... }` as opposed to `mod foo;`.
    pub inline: bool,
    pub parent: Option<ModuleId>,
    pub children: Vec<ModuleId>,
    /// In source order.
    pub functions: Vec<FnId>,
}

impl Module {
    pub fn qual_name(&self) -> String {
        self.path.join("::")
    }
}

#[derive(Debug)]
pub struct Function {
    pub name: String,
    /// e.g. `mtd::ec::task::Zyklus::fahren`
    pub qual_path: String,
    pub module: ModuleId,
    pub owner: FnOwner,
    pub sig: Signature,
    /// Whole item including attributes and doc comments.
    pub span: Span,
    /// Just the identifier; used as jump target.
    pub name_span: Span,
    /// `None` for trait methods without default body.
    pub body_span: Option<Span>,
    /// Structure of the body; `None` without body.
    pub body: Option<Body>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FnOwner {
    Free,
    /// `impl [Trait for] SelfTy { fn ... }`
    Impl {
        self_ty: String,
        trait_: Option<String>,
    },
    /// Provided method in `trait Name { fn ... { } }`
    Trait {
        name: String,
    },
    /// Item declared inside another function's body.
    Nested {
        parent: FnId,
    },
}

impl FnOwner {
    /// Grouping key shown in the tree, e.g. `impl Display for Foo`.
    pub fn group_label(&self) -> Option<String> {
        match self {
            FnOwner::Free | FnOwner::Nested { .. } => None,
            FnOwner::Impl {
                self_ty,
                trait_: None,
            } => Some(format!("impl {self_ty}")),
            FnOwner::Impl {
                self_ty,
                trait_: Some(t),
            } => Some(format!("impl {t} for {self_ty}")),
            FnOwner::Trait { name } => Some(format!("trait {name}")),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Signature {
    /// Signature as written, whitespace collapsed to single spaces.
    pub text: String,
    pub is_async: bool,
    pub is_unsafe: bool,
    pub is_const: bool,
    pub params: Vec<Param>,
    /// Return type as written; `None` for `()`.
    pub ret: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Param {
    /// Pattern as written (`x`, `(a, b)`, `self`).
    pub pat: String,
    /// Type as written; for `self` receivers the receiver form (`&mut self`).
    pub ty: String,
    pub mode: ParamMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ParamMode {
    Value,
    Ref,
    RefMut,
}

#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub severity: Severity,
    pub message: String,
    pub file: Option<PathBuf>,
    pub span: Option<Span>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Error,
}
