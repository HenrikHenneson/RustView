//! Lowering function bodies to nested structures (statement level).
//!
//! Every statement becomes a node. Expressions that open their own scope or
//! control flow (`if`, `match`, loops, blocks, closures with a block body,
//! `async` blocks) become structures. A statement that *is* such an expression
//! (`let x = match ...`) is shown as the structure itself with a prefix;
//! otherwise the structures are embedded in the statement node.

use std::ops::Range;

use rv_ir::{
    Body, Exit, ExitKind, FileId, FrameRole, Node, NodeId, NodeKind, Span, StmtKind, StructKind,
    UseMode, VarDef, VarUse,
};
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::source::{Parsed, collapse_ws};
use crate::vars;

/// Longest label kept in the IR; the view may shorten further.
const MAX_LABEL: usize = 160;

pub(crate) fn lower(
    p: &Parsed,
    file: FileId,
    sig: &syn::Signature,
    block: &syn::Block,
    fn_span: Span,
    name_span: Span,
) -> Body {
    let mut l = Lower {
        p,
        file,
        nodes: Vec::new(),
        exits: Vec::new(),
        scopes: Vec::new(),
    };
    let root = l.add(
        None,
        NodeKind::Structure {
            kind: StructKind::Function,
            prefix: None,
        },
        fn_span,
        name_span,
        syn::ext::IdentExt::unraw(&sig.ident).to_string(),
    );
    l.nodes[root.index()].defs = vars::param_defs(p, file, sig);
    l.scopes.push(Scope::FnLike(root));
    let block_span = l.span(block.span());
    let frame = l.add(
        Some(root),
        frame(FrameRole::Body),
        block_span.clone(),
        block_span,
        String::new(),
    );
    l.block_into(frame, block);
    Body {
        nodes: l.nodes,
        root,
        exits: l.exits,
    }
}

struct Lower<'a> {
    p: &'a Parsed,
    file: FileId,
    nodes: Vec<Node>,
    exits: Vec<Exit>,
    /// Targets for `return`, `?`, `break` and `continue`, innermost last.
    scopes: Vec<Scope>,
}

enum Scope {
    FnLike(NodeId),
    Loop { node: NodeId, label: Option<String> },
    Block { node: NodeId, label: String },
}

/// Where structures found inside an expression go.
#[derive(Clone, Copy)]
enum Container {
    Node(NodeId),
    /// A `Cond` frame of this structure, created on first use.
    CondFrame(NodeId),
}

fn frame(role: FrameRole) -> NodeKind {
    NodeKind::Frame { role }
}

fn stmt(kind: StmtKind) -> NodeKind {
    NodeKind::Stmt { kind }
}

impl<'a> Lower<'a> {
    fn span(&self, s: proc_macro2::Span) -> Span {
        self.p.span(self.file, s)
    }

    /// From the start of `from` to the start of `to` (e.g. a structure header).
    fn span_until(&self, from: proc_macro2::Span, to: proc_macro2::Span) -> Span {
        let (a, b) = (self.span(from), self.span(to));
        Span {
            file: a.file,
            bytes: a.bytes.start..b.bytes.start.max(a.bytes.start),
            start: a.start,
            end: b.start,
        }
    }

    fn raw(&self, bytes: Range<u32>) -> &str {
        self.p
            .text
            .get(bytes.start as usize..bytes.end as usize)
            .unwrap_or("")
    }

    fn add(
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
        });
        if let Some(p) = parent {
            self.nodes[p.index()].children.push(id);
        }
        id
    }

    fn block_into(&mut self, frame: NodeId, block: &syn::Block) {
        let n = block.stmts.len();
        for (i, s) in block.stmts.iter().enumerate() {
            let last = i + 1 == n;
            match s {
                syn::Stmt::Local(local) => self.local(frame, local),
                syn::Stmt::Item(item) => {
                    let span = self.span(item.span());
                    let label = self.item_label(item);
                    self.add(Some(frame), stmt(StmtKind::Item), span.clone(), span, label);
                }
                syn::Stmt::Macro(m) => {
                    let name = self.p.text_of(m.mac.path.span());
                    let span = self.span(m.span());
                    let id = self.add(
                        Some(frame),
                        stmt(StmtKind::Macro { name }),
                        span.clone(),
                        span,
                        String::new(),
                    );
                    let mut v = Embed::new(self, id, id, Container::Node(id));
                    v.macro_uses(&m.mac);
                    self.finish_label(id);
                    self.nodes[id.index()].value = last && m.semi_token.is_none();
                }
                syn::Stmt::Expr(e, semi) => self.expr_stmt(frame, e, last && semi.is_none(), None),
            }
        }
    }

    fn local(&mut self, frame: NodeId, local: &syn::Local) {
        let prefix = format!("let {} =", self.p.text_of(local.pat.span()));
        let mut defs = Vec::new();
        vars::pattern_defs(self.p, self.file, &local.pat, &mut defs);
        if let Some(init) = &local.init
            && init.diverge.is_none()
            && is_structure(&init.expr)
        {
            let id = self.structure(frame, &init.expr, Some(prefix));
            self.nodes[id.index()].defs = defs;
            return;
        }
        let span = self.span(local.span());
        let id = self.add(
            Some(frame),
            stmt(StmtKind::Let),
            span.clone(),
            span,
            String::new(),
        );
        if let Some(init) = &local.init {
            self.embed(id, Container::Node(id), &init.expr);
            if let Some((else_token, else_expr)) = &init.diverge {
                let s_span = self.span(
                    else_token
                        .span()
                        .join(else_expr.span())
                        .unwrap_or(else_expr.span()),
                );
                let head = self.span(else_token.span());
                let s = self.add(
                    Some(id),
                    NodeKind::Structure {
                        kind: StructKind::LetElse,
                        prefix: None,
                    },
                    s_span.clone(),
                    head,
                    "else".into(),
                );
                let f = self.add(
                    Some(s),
                    frame_kind_body(),
                    s_span.clone(),
                    s_span,
                    String::new(),
                );
                self.body_expr_into(f, else_expr);
            }
        }
        self.nodes[id.index()].defs = defs;
        self.finish_label(id);
    }

    /// A statement consisting of an expression.
    fn expr_stmt(&mut self, frame: NodeId, e: &syn::Expr, value: bool, prefix: Option<String>) {
        let e = unwrap(e);
        let id = if is_structure(e) {
            self.structure(frame, e, prefix)
        } else {
            match e {
                syn::Expr::Assign(a) if is_structure(&a.right) => {
                    let prefix = format!("{} =", self.p.text_of(a.left.span()));
                    let id = self.structure(frame, &a.right, Some(prefix));
                    let mut v = Embed::new(self, id, id, Container::Node(id));
                    v.target(&a.left, UseMode::Assign);
                    id
                }
                syn::Expr::Return(_) | syn::Expr::Break(_) | syn::Expr::Continue(_) => {
                    let kind = self.exit_kind(e);
                    let span = self.span(e.span());
                    let id = self.add(
                        Some(frame),
                        stmt(StmtKind::Exit(kind.clone())),
                        span.clone(),
                        span,
                        String::new(),
                    );
                    self.exit(kind, id);
                    let inner = match e {
                        syn::Expr::Return(r) => r.expr.as_deref(),
                        syn::Expr::Break(b) => b.expr.as_deref(),
                        _ => None,
                    };
                    if let Some(inner) = inner {
                        self.embed(id, Container::Node(id), inner);
                    }
                    self.finish_label(id);
                    id
                }
                _ => {
                    let span = self.span(e.span());
                    let id = self.add(
                        Some(frame),
                        stmt(self.classify(e)),
                        span.clone(),
                        span,
                        String::new(),
                    );
                    self.embed(id, Container::Node(id), e);
                    self.finish_label(id);
                    id
                }
            }
        };
        self.nodes[id.index()].value = value && can_have_value(e);
    }

    /// Body of a match arm, closure or `else`: a block's statements, or one expression.
    fn body_expr_into(&mut self, frame: NodeId, e: &syn::Expr) {
        match unwrap(e) {
            syn::Expr::Block(b) if b.label.is_none() => self.block_into(frame, &b.block),
            other => self.expr_stmt(frame, other, true, None),
        }
    }

    fn classify(&self, e: &syn::Expr) -> StmtKind {
        match unwrap(e) {
            syn::Expr::Call(_) => StmtKind::Call,
            syn::Expr::MethodCall(_) => StmtKind::MethodCall,
            syn::Expr::Macro(m) => StmtKind::Macro {
                name: self.p.text_of(m.mac.path.span()),
            },
            syn::Expr::Assign(_) => StmtKind::Assign,
            syn::Expr::Binary(b) if is_assign_op(&b.op) => StmtKind::Assign,
            syn::Expr::Await(a) => self.classify(&a.base),
            syn::Expr::Try(t) => self.classify(&t.expr),
            _ => StmtKind::Expr,
        }
    }

    fn structure(&mut self, parent: NodeId, e: &syn::Expr, prefix: Option<String>) -> NodeId {
        let e = unwrap(e);
        let span = self.span(e.span());
        let new = |l: &mut Self, kind: StructKind, head: Span, label: String| {
            l.add(
                Some(parent),
                NodeKind::Structure {
                    kind,
                    prefix: prefix.clone(),
                },
                span.clone(),
                head,
                label,
            )
        };
        match e {
            syn::Expr::If(first) => {
                let head = self.span_until(first.span(), first.then_branch.span());
                let id = new(self, StructKind::If, head, "if".into());
                let mut cur = first;
                loop {
                    let pattern = one_line(&self.p.text_of(cur.cond.span()));
                    let f_span = self.span(cur.then_branch.span());
                    let f_head = self.span(cur.cond.span());
                    let f = self.add(
                        Some(id),
                        frame(FrameRole::Case {
                            pattern: pattern.clone(),
                            guard: None,
                        }),
                        f_span,
                        f_head,
                        pattern,
                    );
                    let defs = self.embed_with(id, f, Container::Node(f), &cur.cond);
                    self.nodes[f.index()].defs = defs;
                    self.block_into(f, &cur.then_branch);
                    let Some((else_token, else_expr)) = &cur.else_branch else {
                        break;
                    };
                    match unwrap(else_expr) {
                        syn::Expr::If(next) => cur = next,
                        other => {
                            let f_span = self.span(other.span());
                            let f_head = self.span(else_token.span());
                            let f = self.add(
                                Some(id),
                                frame(FrameRole::Else),
                                f_span,
                                f_head,
                                "else".into(),
                            );
                            self.body_expr_into(f, other);
                            break;
                        }
                    }
                }
                id
            }
            syn::Expr::Match(m) => {
                let head = self.span_until(m.span(), m.brace_token.span.open());
                let label = one_line(&format!("match {}", self.p.text_of(m.expr.span())));
                let id = new(self, StructKind::Match, head, label);
                self.embed(id, Container::CondFrame(id), &m.expr);
                for arm in &m.arms {
                    let (pat, guard) = match &arm.pat {
                        syn::Pat::Guard(g) => (&*g.pat, Some(&*g.guard)),
                        other => (other, None),
                    };
                    let pattern = one_line(&self.p.text_of(pat.span()));
                    let guard_text = guard.map(|g| one_line(&self.p.text_of(g.span())));
                    let label = match &guard_text {
                        Some(g) => format!("{pattern} if {g}"),
                        None => pattern.clone(),
                    };
                    let f_span = self.span(arm.span());
                    let f_head = self.span(arm.pat.span());
                    let role = FrameRole::Case {
                        pattern,
                        guard: guard_text,
                    };
                    let f = self.add(Some(id), frame(role), f_span, f_head, label);
                    let mut defs = Vec::new();
                    vars::pattern_defs(self.p, self.file, pat, &mut defs);
                    if let Some(g) = guard {
                        defs.extend(self.embed_with(id, f, Container::Node(f), g));
                    }
                    self.nodes[f.index()].defs = defs;
                    self.body_expr_into(f, &arm.body);
                }
                id
            }
            syn::Expr::Loop(l) => {
                let label = l.label.as_ref().map(|lb| self.p.text_of(lb.name.span()));
                let head = self.span_until(l.span(), l.body.span());
                let text = one_line(self.raw(head.bytes.clone()));
                let id = new(
                    self,
                    StructKind::Loop {
                        label: label.clone(),
                    },
                    head,
                    text,
                );
                self.loop_body(id, label, &l.body, Vec::new());
                id
            }
            syn::Expr::While(w) => {
                let label = w.label.as_ref().map(|lb| self.p.text_of(lb.name.span()));
                let head = self.span_until(w.span(), w.body.span());
                let text = one_line(self.raw(head.bytes.clone()));
                let id = new(
                    self,
                    StructKind::While {
                        label: label.clone(),
                    },
                    head,
                    text,
                );
                let defs = self.embed_with(id, id, Container::CondFrame(id), &w.cond);
                self.loop_body(id, label, &w.body, defs);
                id
            }
            syn::Expr::ForLoop(f) => {
                let label = f.label.as_ref().map(|lb| self.p.text_of(lb.name.span()));
                let head = self.span_until(f.span(), f.body.span());
                let text = one_line(self.raw(head.bytes.clone()));
                let id = new(
                    self,
                    StructKind::For {
                        label: label.clone(),
                    },
                    head,
                    text,
                );
                self.embed(id, Container::CondFrame(id), &f.expr);
                let mut defs = Vec::new();
                vars::pattern_defs(self.p, self.file, &f.pat, &mut defs);
                self.loop_body(id, label, &f.body, defs);
                id
            }
            syn::Expr::Block(b) => {
                let label = b.label.as_ref().map(|lb| self.p.text_of(lb.name.span()));
                let head = self.span(b.block.brace_token.span.open());
                let text = label
                    .as_ref()
                    .map_or("{ }".to_string(), |l| format!("{l}: {{ }}"));
                let id = new(
                    self,
                    StructKind::Block {
                        label: label.clone(),
                    },
                    head,
                    text,
                );
                if let Some(label) = label {
                    self.scopes.push(Scope::Block { node: id, label });
                }
                let _ = self.body_frame(id, &b.block);
                if b.label.is_some() {
                    self.scopes.pop();
                }
                id
            }
            syn::Expr::Unsafe(u) => {
                let head = self.span(u.unsafe_token.span());
                let id = new(self, StructKind::Unsafe, head, "unsafe".into());
                let _ = self.body_frame(id, &u.block);
                id
            }
            syn::Expr::Const(c) => {
                let head = self.span(c.const_token.span());
                let id = new(
                    self,
                    StructKind::Block { label: None },
                    head,
                    "const".into(),
                );
                let _ = self.body_frame(id, &c.block);
                id
            }
            syn::Expr::TryBlock(t) => {
                let head = self.span(t.try_token.span());
                let id = new(self, StructKind::Block { label: None }, head, "try".into());
                self.scopes.push(Scope::FnLike(id));
                let _ = self.body_frame(id, &t.block);
                self.scopes.pop();
                id
            }
            syn::Expr::Async(a) => {
                let head = self.span_until(a.span(), a.block.span());
                let text = one_line(self.raw(head.bytes.clone()));
                let kind = StructKind::Async {
                    is_move: a.capture.is_some(),
                };
                let id = new(self, kind, head, text);
                self.scopes.push(Scope::FnLike(id));
                let _ = self.body_frame(id, &a.block);
                self.scopes.pop();
                id
            }
            syn::Expr::Closure(c) => {
                let head = self.span_until(c.span(), c.body.span());
                let text = one_line(self.raw(head.bytes.clone()));
                let kind = StructKind::Closure {
                    is_move: c.capture.is_some(),
                    is_async: c.asyncness.is_some(),
                };
                let id = new(self, kind, head, text);
                self.scopes.push(Scope::FnLike(id));
                let body_span = self.span(c.body.span());
                let f = self.add(
                    Some(id),
                    frame_kind_body(),
                    body_span.clone(),
                    body_span,
                    String::new(),
                );
                let mut defs = Vec::new();
                for input in &c.inputs {
                    vars::pattern_defs(self.p, self.file, input, &mut defs);
                }
                self.nodes[f.index()].defs = defs;
                self.body_expr_into(f, &c.body);
                self.scopes.pop();
                id
            }
            other => {
                // Not reachable for `is_structure` expressions; keep as statement.
                let id = self.add(
                    Some(parent),
                    stmt(StmtKind::Expr),
                    span.clone(),
                    span,
                    String::new(),
                );
                self.embed(id, Container::Node(id), other);
                self.finish_label(id);
                id
            }
        }
    }

    fn loop_body(
        &mut self,
        id: NodeId,
        label: Option<String>,
        body: &syn::Block,
        defs: Vec<VarDef>,
    ) {
        self.scopes.push(Scope::Loop { node: id, label });
        let f = self.body_frame(id, body);
        self.nodes[f.index()].defs = defs;
        self.scopes.pop();
    }

    fn body_frame(&mut self, structure: NodeId, block: &syn::Block) -> NodeId {
        let span = self.span(block.span());
        let f = self.add(
            Some(structure),
            frame_kind_body(),
            span.clone(),
            span,
            String::new(),
        );
        self.block_into(f, block);
        f
    }

    /// Structures, exits and variable uses inside an expression.
    fn embed(&mut self, owner: NodeId, container: Container, e: &syn::Expr) {
        self.embed_with(owner, owner, container, e);
    }

    /// Exits are attributed to `owner`, variable uses to `use_owner`.
    /// Returns bindings of `let` expressions (`if let`, `while let`).
    fn embed_with(
        &mut self,
        owner: NodeId,
        use_owner: NodeId,
        container: Container,
        e: &syn::Expr,
    ) -> Vec<VarDef> {
        let mut v = Embed::new(self, owner, use_owner, container);
        v.visit_expr(e);
        v.defs
    }

    fn container_node(&mut self, container: &mut Container) -> NodeId {
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

    fn exit_kind(&self, e: &syn::Expr) -> ExitKind {
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

    fn exit(&mut self, kind: ExitKind, from: NodeId) {
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
    fn finish_label(&mut self, id: NodeId) {
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

    fn item_label(&self, item: &syn::Item) -> String {
        let named = |kw: &str, ident: &syn::Ident| format!("{kw} {ident}");
        match item {
            syn::Item::Fn(f) => named("fn", &f.sig.ident),
            syn::Item::Struct(s) => named("struct", &s.ident),
            syn::Item::Enum(e) => named("enum", &e.ident),
            syn::Item::Const(c) => named("const", &c.ident),
            syn::Item::Static(s) => named("static", &s.ident),
            syn::Item::Trait(t) => named("trait", &t.ident),
            syn::Item::Mod(m) => named("mod", &m.ident),
            syn::Item::Type(t) => named("type", &t.ident),
            syn::Item::Impl(i) => format!("impl {}", self.p.text_of(i.self_ty.span())),
            other => one_line(&self.p.text_of(other.span())),
        }
    }
}

fn frame_kind_body() -> NodeKind {
    frame(FrameRole::Body)
}

/// Finds structures, exits and variable uses inside an expression.
struct Embed<'l, 'a> {
    lower: &'l mut Lower<'a>,
    /// Gets exits and `?` counts.
    owner: NodeId,
    /// Gets variable uses.
    use_owner: NodeId,
    container: Container,
    /// Bindings of `let` expressions.
    defs: Vec<VarDef>,
    /// Parameters of enclosing inline closures (not outer variables).
    shadow: Vec<Vec<String>>,
    /// Inside an inline closure: `return`/`?` leave the closure, not us.
    in_closure: u32,
}

impl<'l, 'a> Embed<'l, 'a> {
    fn new(
        lower: &'l mut Lower<'a>,
        owner: NodeId,
        use_owner: NodeId,
        container: Container,
    ) -> Self {
        Embed {
            lower,
            owner,
            use_owner,
            container,
            defs: Vec::new(),
            shadow: Vec::new(),
            in_closure: 0,
        }
    }

    fn use_name(&mut self, name: String, span: proc_macro2::Span, mode: UseMode, certain: bool) {
        if self.shadow.iter().any(|s| s.contains(&name)) {
            return;
        }
        let span = self.lower.span(span);
        self.lower.nodes[self.use_owner.index()].uses.push(VarUse {
            name,
            span,
            mode,
            certain,
        });
    }

    /// The place written by an assignment, `&mut` or method call:
    /// `x`, `x.field`, `x[i]`, `*x`.
    fn target(&mut self, e: &syn::Expr, mode: UseMode) {
        match unwrap(e) {
            syn::Expr::Path(p) => {
                if let Some(name) = vars::local_name(p) {
                    self.use_name(name, p.span(), mode, true);
                }
            }
            syn::Expr::Field(f) => self.target(&f.base, mode),
            syn::Expr::Index(i) => {
                self.visit_expr(&i.index);
                self.target(&i.expr, mode);
            }
            syn::Expr::Unary(u) if matches!(u.op, syn::UnOp::Deref(_)) => {
                self.target(&u.expr, mode)
            }
            other => self.visit_expr(other),
        }
    }

    fn macro_uses(&mut self, mac: &syn::Macro) {
        use syn::parse::Parser;
        if vars::is_known_macro(&mac.path) {
            let parsed = syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                .parse2(mac.tokens.clone())
                .map(|args| args.into_iter().collect::<Vec<_>>())
                .or_else(|_| {
                    // `vec![x; n]`
                    (|input: syn::parse::ParseStream| {
                        let a: syn::Expr = input.parse()?;
                        let _: syn::Token![;] = input.parse()?;
                        let b: syn::Expr = input.parse()?;
                        Ok(vec![a, b])
                    })
                    .parse2(mac.tokens.clone())
                });
            if let Ok(args) = parsed {
                for arg in &args {
                    match arg {
                        syn::Expr::Lit(syn::ExprLit {
                            lit: syn::Lit::Str(s),
                            ..
                        }) => {
                            for name in vars::format_arg_names(&s.value()) {
                                self.use_name(name, s.span(), UseMode::Read, true);
                            }
                        }
                        other => self.visit_expr(other),
                    }
                }
                return;
            }
        }
        // Not understood: every local-looking name might be a use.
        let mut names = Vec::new();
        vars::token_names(mac.tokens.clone(), &mut names);
        for (name, span) in names {
            self.use_name(name, span, UseMode::Read, false);
        }
    }
}

impl<'ast> Visit<'ast> for Embed<'_, '_> {
    fn visit_expr(&mut self, e: &'ast syn::Expr) {
        if is_structure(e) {
            let parent = self.lower.container_node(&mut self.container);
            self.lower.structure(parent, e, None);
            return;
        }
        match e {
            syn::Expr::Closure(c) => {
                let mut params = Vec::new();
                for input in &c.inputs {
                    vars::pattern_defs(self.lower.p, self.lower.file, input, &mut params);
                }
                self.shadow
                    .push(params.into_iter().map(|d| d.name).collect());
                self.in_closure += 1;
                self.visit_expr(&c.body);
                self.in_closure -= 1;
                self.shadow.pop();
                return;
            }
            syn::Expr::Macro(m) => {
                self.macro_uses(&m.mac);
                return;
            }
            syn::Expr::Path(p) => {
                if let Some(name) = vars::local_name(p) {
                    self.use_name(name, p.span(), UseMode::Read, true);
                }
                return;
            }
            syn::Expr::Assign(a) => {
                self.target(&a.left, UseMode::Assign);
                self.visit_expr(&a.right);
                return;
            }
            syn::Expr::Binary(b) if is_assign_op(&b.op) => {
                self.target(&b.left, UseMode::CompoundAssign);
                self.visit_expr(&b.right);
                return;
            }
            syn::Expr::Reference(r) if r.mutability.is_some() => {
                self.target(&r.expr, UseMode::RefMut);
                return;
            }
            syn::Expr::MethodCall(m) => {
                self.target(&m.receiver, vars::receiver_mode(&m.method.to_string()));
                for arg in &m.args {
                    self.visit_expr(arg);
                }
                return;
            }
            syn::Expr::Let(l) => {
                self.visit_expr(&l.expr);
                vars::pattern_defs(self.lower.p, self.lower.file, &l.pat, &mut self.defs);
                return;
            }
            syn::Expr::Try(_)
            | syn::Expr::Return(_)
            | syn::Expr::Break(_)
            | syn::Expr::Continue(_)
                if self.in_closure == 0 =>
            {
                let kind = self.lower.exit_kind(e);
                self.lower.exit(kind, self.owner);
            }
            _ => {}
        }
        syn::visit::visit_expr(self, e);
    }

    // Items inside expressions are not part of the control flow.
    fn visit_item(&mut self, _: &'ast syn::Item) {}
}

fn unwrap(e: &syn::Expr) -> &syn::Expr {
    match e {
        syn::Expr::Paren(p) => unwrap(&p.expr),
        syn::Expr::Group(g) => unwrap(&g.expr),
        other => other,
    }
}

/// Expressions shown as structures (frames) rather than as text.
fn is_structure(e: &syn::Expr) -> bool {
    match unwrap(e) {
        syn::Expr::If(_)
        | syn::Expr::Match(_)
        | syn::Expr::Loop(_)
        | syn::Expr::While(_)
        | syn::Expr::ForLoop(_)
        | syn::Expr::Block(_)
        | syn::Expr::Unsafe(_)
        | syn::Expr::Const(_)
        | syn::Expr::TryBlock(_)
        | syn::Expr::Async(_) => true,
        syn::Expr::Closure(c) => {
            matches!(unwrap(&c.body), syn::Expr::Block(_)) || is_structure(&c.body)
        }
        _ => false,
    }
}

/// `false` for expressions that are always `()` (loops without `break value`
/// are rare enough to ignore, `if` without `else`).
fn can_have_value(e: &syn::Expr) -> bool {
    match unwrap(e) {
        syn::Expr::If(i) => i.else_branch.is_some(),
        syn::Expr::While(_) | syn::Expr::ForLoop(_) | syn::Expr::Loop(_) => false,
        _ => true,
    }
}

fn is_assign_op(op: &syn::BinOp) -> bool {
    use syn::BinOp::*;
    matches!(
        op,
        AddAssign(_)
            | SubAssign(_)
            | MulAssign(_)
            | DivAssign(_)
            | RemAssign(_)
            | BitXorAssign(_)
            | BitAndAssign(_)
            | BitOrAssign(_)
            | ShlAssign(_)
            | ShrAssign(_)
    )
}

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

/// One line without comments, whitespace collapsed, `;` dropped, length capped.
pub(crate) fn one_line(s: &str) -> String {
    let text = collapse_ws(&strip_comments(s));
    let text = text.trim_end_matches(';').trim_end();
    if text.chars().count() > MAX_LABEL {
        let mut cut: String = text.chars().take(MAX_LABEL - 1).collect();
        cut.push('…');
        cut
    } else {
        text.to_string()
    }
}

/// Remove `//` and `/* */` comments outside string and char literals.
fn strip_comments(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    i += 1;
                }
                i += 2;
                out.push(' ');
            }
            '"' => {
                let start = i;
                i += 1;
                while i < chars.len() && chars[i] != '"' {
                    if chars[i] == '\\' {
                        i += 1;
                    }
                    i += 1;
                }
                i += 1;
                out.extend(&chars[start..i.min(chars.len())]);
            }
            'r' if (next == Some('"') || next == Some('#'))
                && !out
                    .chars()
                    .last()
                    .is_some_and(|p| p.is_alphanumeric() || p == '_') =>
            {
                // Raw string r"..." / r#"..."#
                let start = i;
                let mut j = i + 1;
                let mut hashes = 0;
                while chars.get(j) == Some(&'#') {
                    hashes += 1;
                    j += 1;
                }
                if chars.get(j) != Some(&'"') {
                    out.push(c);
                    i += 1;
                    continue;
                }
                j += 1;
                loop {
                    if j >= chars.len() {
                        break;
                    }
                    if chars[j] == '"' && (0..hashes).all(|k| chars.get(j + 1 + k) == Some(&'#')) {
                        j += 1 + hashes;
                        break;
                    }
                    j += 1;
                }
                out.extend(&chars[start..j.min(chars.len())]);
                i = j;
            }
            '\'' => {
                // Char literal ('x', '\n', '\u{..}') or lifetime ('a).
                let end = if next == Some('\\') {
                    (i + 2..chars.len()).find(|&k| chars[k] == '\'')
                } else if chars.get(i + 2) == Some(&'\'') {
                    Some(i + 2)
                } else {
                    None
                };
                let end = end.map_or(i + 1, |e| e + 1);
                out.extend(&chars[i..end]);
                i = end;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn comments_are_stripped_outside_literals() {
        assert_eq!(strip_comments("a // x\nb"), "a \nb");
        assert_eq!(strip_comments("a /* x */ b"), "a   b");
        assert_eq!(
            strip_comments(r#"f("// no", '/', 'a) // yes"#),
            r#"f("// no", '/', 'a) "#
        );
        assert_eq!(
            strip_comments(r##"r#"/* raw */"# // c"##),
            r##"r#"/* raw */"# "##
        );
        assert_eq!(strip_comments(r#"'\'' // c"#), r#"'\'' "#);
    }

    #[test]
    fn one_line_caps_length() {
        let long = "x".repeat(400);
        assert_eq!(one_line(&long).chars().count(), MAX_LABEL);
        assert_eq!(one_line("foo(\n  a,\n  b,\n);"), "foo(a, b)");
    }
}
