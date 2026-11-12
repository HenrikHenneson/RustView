//! The body under construction: its nodes, its exits and the scopes that
//! `return`, `?`, `break` and `continue` leave.

use std::ops::Range;

use rv_ir::{
    Exit, ExitKind, FileId, FrameRole, Node, NodeId, NodeKind, Span, StmtKind, StructKind,
};

use crate::exprtree::TreeBuilder;
use crate::source::{Parsed, Source};
use crate::text::one_line;

pub(super) struct Lower<'a> {
    pub(super) p: &'a Parsed,
    pub(super) file: FileId,
    pub(super) nodes: Vec<Node>,
    pub(super) exits: Vec<Exit>,
    /// Targets for `return`, `?`, `break` and `continue`, innermost last.
    pub(super) scopes: Vec<Scope>,
}

pub(super) enum Scope {
    FnLike(NodeId),
    Loop { node: NodeId, label: Option<String> },
    Block { node: NodeId, label: String },
}

/// Where structures found inside an expression go.
#[derive(Clone, Copy)]
pub(super) enum Container {
    Node(NodeId),
    /// A `Cond` frame of this structure, created on first use.
    CondFrame(NodeId),
}

/// Where a new structure goes and what its statement puts in front of it.
pub(super) struct Place {
    pub(super) parent: NodeId,
    /// `let x =` or `x =` of the statement the structure is the value of.
    pub(super) prefix: Option<String>,
    pub(super) span: Span,
}

pub(super) fn frame(role: FrameRole) -> NodeKind {
    NodeKind::Frame { role }
}

pub(super) fn stmt(kind: StmtKind) -> NodeKind {
    NodeKind::Stmt { kind }
}

impl<'a> Lower<'a> {
    pub(super) fn new(src: Source<'a>) -> Self {
        Lower {
            p: src.parsed,
            file: src.file,
            nodes: Vec::new(),
            exits: Vec::new(),
            scopes: Vec::new(),
        }
    }

    pub(super) fn span(&self, s: proc_macro2::Span) -> Span {
        self.p.span(self.file, s)
    }

    /// From the start of `from` to the start of `to` (e.g. a structure header).
    pub(super) fn span_until(&self, from: proc_macro2::Span, to: proc_macro2::Span) -> Span {
        let (a, b) = (self.span(from), self.span(to));
        Span {
            file: a.file,
            bytes: a.bytes.start..b.bytes.start.max(a.bytes.start),
            start: a.start,
            end: b.start,
        }
    }

    /// Span and text of a structure header: from the start of `from` to its
    /// body `to` (`while x < 3`, `move |a|`).
    pub(super) fn header(&self, from: proc_macro2::Span, to: proc_macro2::Span) -> (Span, String) {
        let head = self.span_until(from, to);
        let text = one_line(self.raw(head.bytes.clone()));
        (head, text)
    }

    /// Text of a loop or block label (`'outer`).
    pub(super) fn label_text(&self, label: &Option<syn::Label>) -> Option<String> {
        label.as_ref().map(|lb| self.p.text_of(lb.name.span()))
    }

    fn raw(&self, bytes: Range<u32>) -> &str {
        self.p
            .text
            .get(bytes.start as usize..bytes.end as usize)
            .unwrap_or("")
    }

    pub(super) fn add(
        &mut self,
        parent: Option<NodeId>,
        kind: NodeKind,
        span: Span,
        head: Span,
        label: String,
    ) -> NodeId {
        let id = NodeId(self.nodes.len() as u32);
        self.nodes.push(Node {
            kind,
            parent,
            children: Vec::new(),
            span,
            head,
            label,
            value: false,
            tries: 0,
            defs: Vec::new(),
            uses: Vec::new(),
            calls: Vec::new(),
            expr: None,
        });
        if let Some(p) = parent {
            self.nodes[p.index()].children.push(id);
        }
        id
    }

    /// A statement node; its label is set by `finish_label` once its
    /// embedded structures are known.
    pub(super) fn add_stmt(&mut self, parent: NodeId, kind: StmtKind, span: Span) -> NodeId {
        self.add(Some(parent), stmt(kind), span.clone(), span, String::new())
    }

    /// An empty `Body` frame below `parent`.
    pub(super) fn add_body_frame(&mut self, parent: NodeId, span: Span) -> NodeId {
        self.add(
            Some(parent),
            frame(FrameRole::Body),
            span.clone(),
            span,
            String::new(),
        )
    }

    /// The node of a new structure.
    pub(super) fn open(
        &mut self,
        at: Place,
        kind: StructKind,
        head: Span,
        label: String,
    ) -> NodeId {
        let kind = NodeKind::Structure {
            kind,
            prefix: at.prefix,
        };
        self.add(Some(at.parent), kind, at.span, head, label)
    }

    /// The node a container stands for; creates the `Cond` frame if needed.
    pub(super) fn container_node(&mut self, container: &mut Container) -> NodeId {
        match *container {
            Container::Node(id) => id,
            Container::CondFrame(structure) => {
                let s = &self.nodes[structure.index()];
                let (span, head) = (s.span.clone(), s.head.clone());
                let f = self.add(
                    Some(structure),
                    frame(FrameRole::Cond),
                    span,
                    head,
                    "cond".into(),
                );
                *container = Container::Node(f);
                f
            }
        }
    }

    pub(super) fn exit_kind(&self, e: &syn::Expr) -> ExitKind {
        match e {
            syn::Expr::Break(b) => ExitKind::Break {
                label: b.label.as_ref().map(|l| self.p.text_of(l.span())),
            },
            syn::Expr::Continue(c) => ExitKind::Continue {
                label: c.label.as_ref().map(|l| self.p.text_of(l.span())),
            },
            syn::Expr::Try(_) => ExitKind::Try,
            _ => ExitKind::Return,
        }
    }

    /// Record that `from` leaves the innermost scope matching `kind`.
    pub(super) fn exit(&mut self, kind: ExitKind, from: NodeId) {
        let target = match &kind {
            ExitKind::Return | ExitKind::Try => self.scopes.iter().rev().find_map(|s| match s {
                Scope::FnLike(n) => Some(*n),
                _ => None,
            }),
            ExitKind::Break { label } => self.loop_target(label.as_deref(), true),
            ExitKind::Continue { label } => self.loop_target(label.as_deref(), false),
        };
        if kind == ExitKind::Try {
            self.nodes[from.index()].tries += 1;
        }
        self.exits.push(Exit { kind, from, target });
    }

    fn loop_target(&self, label: Option<&str>, allow_block: bool) -> Option<NodeId> {
        for scope in self.scopes.iter().rev() {
            match scope {
                Scope::FnLike(_) => return None,
                Scope::Loop { node, label: l } => {
                    if label.is_none() || label == l.as_deref() {
                        return Some(*node);
                    }
                }
                Scope::Block { node, label: l } => {
                    if allow_block && label == Some(l.as_str()) {
                        return Some(*node);
                    }
                }
            }
        }
        None
    }

    /// Statement text with embedded structures replaced by short placeholders.
    pub(super) fn finish_label(&mut self, id: NodeId) {
        let node = &self.nodes[id.index()];
        let full = node.span.bytes.clone();
        let mut parts: Vec<(Range<u32>, String)> = node
            .children
            .iter()
            .map(|&c| {
                let child = &self.nodes[c.index()];
                (child.span.bytes.clone(), placeholder(child))
            })
            .collect();
        parts.sort_by_key(|(r, _)| r.start);
        let mut text = String::new();
        let mut pos = full.start;
        for (r, ph) in parts {
            if r.start >= pos && r.end <= full.end {
                text.push_str(self.raw(pos..r.start));
                text.push_str(&ph);
                pos = r.end;
            }
        }
        text.push_str(self.raw(pos..full.end));
        self.nodes[id.index()].label = one_line(&text);
    }

    /// Expression tree of a statement, linked to its embedded structures.
    /// `root_of` builds the tree and returns its root.
    pub(super) fn attach_tree(
        &mut self,
        id: NodeId,
        root_of: impl FnOnce(&mut TreeBuilder) -> usize,
    ) {
        let structures = self.nodes[id.index()]
            .children
            .iter()
            .map(|&c| {
                let n = &self.nodes[c.index()];
                let label: String = n.label.chars().take(24).collect();
                (n.span.clone(), c, label)
            })
            .collect();
        let mut b = TreeBuilder::new(self.p, self.file, structures);
        let root = root_of(&mut b);
        self.nodes[id.index()].expr = Some(b.finish(root));
    }
}

/// What a statement label shows instead of an embedded structure.
fn placeholder(child: &Node) -> String {
    let head: String = child.label.chars().take(40).collect();
    match &child.kind {
        NodeKind::Structure {
            kind: StructKind::Block { .. },
            ..
        } => "{…}".into(),
        NodeKind::Structure { .. } => format!("{head} {{…}}"),
        _ => "…".into(),
    }
}
