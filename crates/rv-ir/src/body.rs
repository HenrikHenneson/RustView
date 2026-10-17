//! Function bodies as nested structures.
//!
//! A body is a tree: [`NodeKind::Structure`] nodes (loops, case structures,
//! closures, ...) contain [`NodeKind::Frame`]s, frames contain statements and
//! structures in execution order, and statements can embed structures that
//! appear inside their expressions (`spawn(move || { ... })`).

use std::fmt::Write;

use crate::Span;

/// Index into [`Body::nodes`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct NodeId(pub u32);

impl NodeId {
    pub fn index(self) -> usize {
        self.0 as usize
    }
}

#[derive(Clone, Debug)]
pub struct Body {
    pub nodes: Vec<Node>,
    /// A [`StructKind::Function`] structure.
    pub root: NodeId,
    /// Early exits (`return`, `?`, `break`, `continue`) with resolved targets.
    pub exits: Vec<Exit>,
}

#[derive(Clone, Debug)]
pub struct Node {
    pub kind: NodeKind,
    pub parent: Option<NodeId>,
    /// Structure: its frames. Frame: its items in order. Stmt: embedded structures.
    pub children: Vec<NodeId>,
    pub span: Span,
    /// The part to highlight when the node is selected (e.g. `match x` of a whole match).
    pub head: Span,
    /// One-line summary: statement text with embedded bodies elided, or a structure header.
    pub label: String,
    /// The node produces the value of its frame (tail expression without `;`).
    pub value: bool,
    /// Number of `?` operators directly in this statement or structure
    /// header (not in nested closures or structures).
    pub tries: u32,
    /// Variables this node introduces (`let` patterns, match arm patterns,
    /// loop patterns, parameters). Visible after the node, or inside it for frames.
    pub defs: Vec<VarDef>,
    /// Variables read or written by this node itself (not by nested structures),
    /// in source order.
    pub uses: Vec<VarUse>,
}

/// A variable introduced by a pattern.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VarDef {
    pub name: String,
    pub span: Span,
    pub mutable: bool,
}

/// A mention of a local variable name. Whether it really refers to a local
/// variable is decided by name resolution (`rv-analysis`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VarUse {
    pub name: String,
    pub span: Span,
    pub mode: UseMode,
    /// `false` if guessed from the tokens of a macro that is not understood.
    pub certain: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UseMode {
    Read,
    /// `x = ...`, `x.field = ...`, `*x = ...`
    Assign,
    /// `x += ...`
    CompoundAssign,
    /// `&mut x`
    RefMut,
    /// Receiver of a method call: mutates if the method takes `&mut self`,
    /// which syntax alone cannot tell.
    MethodRecv,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NodeKind {
    Structure {
        kind: StructKind,
        /// Binding the structure's value goes to, e.g. `let x =`.
        prefix: Option<String>,
    },
    Frame {
        role: FrameRole,
    },
    Stmt {
        kind: StmtKind,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StructKind {
    Function,
    Loop {
        label: Option<String>,
    },
    While {
        label: Option<String>,
    },
    For {
        label: Option<String>,
    },
    /// `if` / `else if` / `else` chain.
    If,
    Match,
    Block {
        label: Option<String>,
    },
    Unsafe,
    Closure {
        is_move: bool,
        is_async: bool,
    },
    Async {
        is_move: bool,
    },
    /// The `else` block of `let ... else`.
    LetElse,
}

impl StructKind {
    pub fn is_loop(&self) -> bool {
        matches!(
            self,
            StructKind::Loop { .. } | StructKind::While { .. } | StructKind::For { .. }
        )
    }

    /// Frames are alternatives (only one runs), not parts.
    pub fn is_case(&self) -> bool {
        matches!(self, StructKind::If | StructKind::Match)
    }

    /// Has its own `return` / `?` target.
    pub fn is_fn_like(&self) -> bool {
        matches!(
            self,
            StructKind::Function | StructKind::Closure { .. } | StructKind::Async { .. }
        )
    }

    pub fn name(&self) -> &'static str {
        match self {
            StructKind::Function => "fn",
            StructKind::Loop { .. } => "loop",
            StructKind::While { .. } => "while",
            StructKind::For { .. } => "for",
            StructKind::If => "if",
            StructKind::Match => "match",
            StructKind::Block { .. } => "block",
            StructKind::Unsafe => "unsafe",
            StructKind::Closure { .. } => "closure",
            StructKind::Async { .. } => "async",
            StructKind::LetElse => "let-else",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FrameRole {
    Body,
    /// Structures inside a condition, scrutinee or iterator expression.
    Cond,
    /// A case of an `if` chain (`pattern` is the condition) or a `match` arm.
    Case {
        pattern: String,
        guard: Option<String>,
    },
    Else,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StmtKind {
    Let,
    Assign,
    Call,
    MethodCall,
    /// Not expanded; `name` is the macro path, e.g. `log::info`.
    Macro {
        name: String,
    },
    /// Item declared inside the body (`fn`, `struct`, `use`, ...).
    Item,
    Exit(ExitKind),
    Expr,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExitKind {
    Return,
    Try,
    Break { label: Option<String> },
    Continue { label: Option<String> },
}

impl ExitKind {
    pub fn symbol(&self) -> &'static str {
        match self {
            ExitKind::Return => "return",
            ExitKind::Try => "?",
            ExitKind::Break { .. } => "break",
            ExitKind::Continue { .. } => "continue",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Exit {
    pub kind: ExitKind,
    /// The statement the exit is in.
    pub from: NodeId,
    /// Structure that is left (`return`/`?`: the enclosing fn, closure or async
    /// block; `break`/`continue`: the loop or labeled block). `None` if the
    /// label could not be resolved.
    pub target: Option<NodeId>,
}

impl Body {
    pub fn node(&self, id: NodeId) -> &Node {
        &self.nodes[id.index()]
    }

    /// Number of statements below `id` (the "size" of a collapsed structure).
    pub fn stmt_count(&self, id: NodeId) -> usize {
        let node = self.node(id);
        let own = usize::from(matches!(node.kind, NodeKind::Stmt { .. }));
        own + node
            .children
            .iter()
            .map(|&c| self.stmt_count(c))
            .sum::<usize>()
    }

    /// Indented outline for tests and debugging.
    pub fn dump(&self) -> String {
        let mut out = String::new();
        self.dump_node(self.root, 0, &mut out);
        for e in &self.exits {
            let target = e
                .target
                .map_or("?".to_string(), |t| self.node(t).label.clone());
            let _ = writeln!(
                out,
                "exit {} from `{}` to `{}`",
                e.kind.symbol(),
                self.node(e.from).label,
                target
            );
        }
        out
    }

    fn dump_node(&self, id: NodeId, depth: usize, out: &mut String) {
        let n = self.node(id);
        let indent = "  ".repeat(depth);
        let value = if n.value { " =>" } else { "" };
        match &n.kind {
            NodeKind::Structure { kind, prefix } => {
                let prefix = prefix.as_ref().map(|p| format!("{p} ")).unwrap_or_default();
                let tries = tries_suffix(n.tries);
                let _ = writeln!(
                    out,
                    "{indent}[{}] {prefix}{}{tries}{value}",
                    kind.name(),
                    n.label
                );
            }
            NodeKind::Frame { role } => {
                let role = match role {
                    FrameRole::Body => "body".to_string(),
                    FrameRole::Cond => "cond".to_string(),
                    FrameRole::Else => "else".to_string(),
                    FrameRole::Case {
                        pattern,
                        guard: None,
                    } => format!("case {pattern}"),
                    FrameRole::Case {
                        pattern,
                        guard: Some(g),
                    } => format!("case {pattern} if {g}"),
                };
                let _ = writeln!(out, "{indent}<{role}>");
            }
            NodeKind::Stmt { kind } => {
                let kind = match kind {
                    StmtKind::Let => "let",
                    StmtKind::Assign => "assign",
                    StmtKind::Call => "call",
                    StmtKind::MethodCall => "method",
                    StmtKind::Macro { .. } => "macro",
                    StmtKind::Item => "item",
                    StmtKind::Exit(e) => e.symbol(),
                    StmtKind::Expr => "expr",
                };
                let tries = tries_suffix(n.tries);
                let _ = writeln!(out, "{indent}{kind}: {}{tries}{value}", n.label);
            }
        }
        for &c in &n.children {
            self.dump_node(c, depth + 1, out);
        }
    }
}

fn tries_suffix(n: u32) -> String {
    if n > 0 {
        format!(" ?x{n}")
    } else {
        String::new()
    }
}
