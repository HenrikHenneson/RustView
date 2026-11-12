//! `file:line` references relative to the project directory, shared by the
//! text outputs (report, maintainability, queries).

use std::path::{Path, PathBuf};

use rv_ir::{FnId, NodeId, Project, Span};

/// The directory paths are shown relative to.
pub(crate) fn base_dir(root: &Path) -> PathBuf {
    if root.is_file() {
        root.parent().map(Path::to_path_buf).unwrap_or_default()
    } else {
        root.to_path_buf()
    }
}

/// Last two path segments: `Zyklus::fahren`, `task::run`.
pub(crate) fn short(qual: &str) -> String {
    let parts: Vec<&str> = qual.rsplitn(3, "::").collect();
    match parts.as_slice() {
        [last, prev, ..] => format!("{prev}::{last}"),
        _ => qual.to_string(),
    }
}

/// Formats locations relative to the project directory.
pub(crate) struct Loc<'p> {
    pub project: &'p Project,
    pub base: PathBuf,
}

impl<'p> Loc<'p> {
    pub fn new(project: &'p Project) -> Self {
        Loc {
            project,
            base: base_dir(&project.root),
        }
    }

    /// `src/gui/app.rs`
    pub fn path(&self, path: &Path) -> String {
        path.strip_prefix(&self.base)
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/")
    }

    pub fn file(&self, span: &Span) -> String {
        self.path(&self.project.file(span.file).path)
    }

    /// `src/gui/app.rs:12`
    pub fn at(&self, span: &Span) -> String {
        format!("{}:{}", self.file(span), span.start.line)
    }

    /// `` `a::b::f` (src/b.rs:12) ``
    pub fn func(&self, f: FnId) -> String {
        let func = self.project.function(f);
        format!("`{}` ({})", func.qual_path, self.at(&func.name_span))
    }

    /// Location of a node, falling back to the function.
    pub fn node(&self, f: FnId, node: NodeId) -> String {
        let func = self.project.function(f);
        match &func.body {
            Some(body) if node.index() < body.nodes.len() => self.at(&body.node(node).span),
            _ => self.at(&func.name_span),
        }
    }
}
