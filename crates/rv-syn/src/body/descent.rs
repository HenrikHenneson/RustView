//! The recursive descent through a body.
//!
//! Statements contain expressions, expressions contain structures and
//! structures contain statements again. The three parts call each other and
//! therefore live in one module: statements and structures as methods of
//! `Lower`, expressions as the visitor `Embed`.

use rv_ir::{CallKind, CallSite, FrameRole, NodeId, StmtKind, StructKind, UseMode, VarDef, VarUse};
use syn::spanned::Spanned;
use syn::visit::Visit;

use super::arena::{Container, Lower, Place, Scope, frame, stmt};
use super::calls::{arg_local, function_site, known_macro_args, method_site, new_call};
use super::syntax::{can_have_value, classify, is_assign_op, is_structure, item_label, unwrap};
use crate::text::{one_line, short};
use crate::vars;

/// Statements.
impl Lower<'_> {
    /// Lower the statements of `block` as children of `frame`.
    pub(super) fn block_into(&mut self, frame: NodeId, block: &syn::Block) {
        let n = block.stmts.len();
        for (i, s) in block.stmts.iter().enumerate() {
            let last = i + 1 == n;
            match s {
                syn::Stmt::Local(local) => self.local(frame, local),
                syn::Stmt::Item(item) => {
                    let span = self.span(item.span());
                    let label = item_label(self.p, item);
                    self.add(Some(frame), stmt(StmtKind::Item), span.clone(), span, label);
                }
                syn::Stmt::Macro(m) => self.macro_stmt(frame, m, last),
                syn::Stmt::Expr(e, semi) => self.expr_stmt(frame, e, last && semi.is_none(), None),
            }
        }
    }

    /// A macro call in statement position (`println!("..");`).
    fn macro_stmt(&mut self, frame: NodeId, m: &syn::StmtMacro, last: bool) {
        let name = self.p.text_of(m.mac.path.span());
        let span = self.span(m.span());
        let id = self.add_stmt(frame, StmtKind::Macro { name }, span);
        let mut v = Embed::new(self, id, id, Container::Node(id));
        v.macro_uses(&m.mac);
        self.finish_label(id);
        self.attach_tree(id, |b| b.macro_node(&m.mac, m.span()));
        self.nodes[id.index()].value = last && m.semi_token.is_none();
    }

    /// A `let` statement, or the structure it binds (`let x = match ..`).
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
        let id = self.add_stmt(frame, StmtKind::Let, span);
        if let Some(init) = &local.init {
            self.embed(id, Container::Node(id), &init.expr);
            if let Some((else_token, else_expr)) = &init.diverge {
                self.let_else(id, else_token, else_expr);
            }
        }
        self.nodes[id.index()].defs = defs;
        self.finish_label(id);
        let init = local.init.as_ref().map(|i| &*i.expr);
        self.attach_tree(id, |b| b.bind(&local.pat, init, local.span()));
    }

    /// The `else { .. }` of a `let ... else`: a structure below the `let` statement.
    fn let_else(&mut self, let_stmt: NodeId, else_token: &syn::Token![else], body: &syn::Expr) {
        let span = self.span(else_token.span().join(body.span()).unwrap_or(body.span()));
        let head = self.span(else_token.span());
        let at = Place {
            parent: let_stmt,
            prefix: None,
            span: span.clone(),
        };
        let s = self.open(at, StructKind::LetElse, head, "else".into());
        let f = self.add_body_frame(s, span);
        self.body_expr_into(f, body);
    }

    /// A statement consisting of an expression.
    fn expr_stmt(&mut self, frame: NodeId, e: &syn::Expr, value: bool, prefix: Option<String>) {
        let e = unwrap(e);
        let id = match e {
            _ if is_structure(e) => self.structure(frame, e, prefix),
            syn::Expr::Assign(a) if is_structure(&a.right) => self.assigned_structure(frame, a),
            syn::Expr::Return(_) | syn::Expr::Break(_) | syn::Expr::Continue(_) => {
                self.exit_stmt(frame, e)
            }
            _ => self.plain_stmt(frame, e),
        };
        self.nodes[id.index()].value = value && can_have_value(e);
    }

    /// `x = match .. { .. }`: the structure itself, with `x =` as prefix.
    fn assigned_structure(&mut self, frame: NodeId, a: &syn::ExprAssign) -> NodeId {
        let prefix = format!("{} =", self.p.text_of(a.left.span()));
        let id = self.structure(frame, &a.right, Some(prefix));
        let mut v = Embed::new(self, id, id, Container::Node(id));
        v.target(&a.left, UseMode::Assign);
        id
    }

    /// `return`, `break` or `continue` as a statement.
    fn exit_stmt(&mut self, frame: NodeId, e: &syn::Expr) -> NodeId {
        let kind = self.exit_kind(e);
        let span = self.span(e.span());
        let id = self.add_stmt(frame, StmtKind::Exit(kind.clone()), span);
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
        self.attach_tree(id, |b| b.expr(e));
        id
    }

    /// Any other expression statement: its text, with structures embedded.
    fn plain_stmt(&mut self, frame: NodeId, e: &syn::Expr) -> NodeId {
        let span = self.span(e.span());
        let id = self.add_stmt(frame, classify(self.p, e), span);
        self.embed(id, Container::Node(id), e);
        self.finish_label(id);
        self.attach_tree(id, |b| b.expr(e));
        id
    }

    /// Body of a match arm, closure or `else`: a block's statements, or one expression.
    fn body_expr_into(&mut self, frame: NodeId, e: &syn::Expr) {
        match unwrap(e) {
            syn::Expr::Block(b) if b.label.is_none() => self.block_into(frame, &b.block),
            other => self.expr_stmt(frame, other, true, None),
        }
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
}

/// Structures.
impl Lower<'_> {
    /// Lower the structure `e` as a child of `parent`.
    fn structure(&mut self, parent: NodeId, e: &syn::Expr, prefix: Option<String>) -> NodeId {
        let e = unwrap(e);
        let at = Place {
            parent,
            prefix,
            span: self.span(e.span()),
        };
        match e {
            syn::Expr::If(first) => self.if_chain(at, first),
            syn::Expr::Match(m) => self.match_structure(at, m),
            syn::Expr::Loop(l) => self.loop_structure(at, l),
            syn::Expr::While(w) => self.while_structure(at, w),
            syn::Expr::ForLoop(f) => self.for_structure(at, f),
            syn::Expr::Block(b) => self.block_structure(at, b),
            syn::Expr::Unsafe(u) => {
                let head = self.span(u.unsafe_token.span());
                let id = self.open(at, StructKind::Unsafe, head, "unsafe".into());
                let _ = self.body_frame(id, &u.block);
                id
            }
            syn::Expr::Const(c) => {
                let head = self.span(c.const_token.span());
                let id = self.open(at, StructKind::Block { label: None }, head, "const".into());
                let _ = self.body_frame(id, &c.block);
                id
            }
            syn::Expr::TryBlock(t) => {
                let head = self.span(t.try_token.span());
                let id = self.open(at, StructKind::Block { label: None }, head, "try".into());
                self.fn_like_body(id, &t.block);
                id
            }
            syn::Expr::Async(a) => {
                let (head, text) = self.header(a.span(), a.block.span());
                let kind = StructKind::Async {
                    is_move: a.capture.is_some(),
                };
                let id = self.open(at, kind, head, text);
                self.fn_like_body(id, &a.block);
                id
            }
            syn::Expr::Closure(c) => self.closure_structure(at, c),
            other => {
                // Not reachable for `is_structure` expressions; keep as statement.
                let id = self.add_stmt(at.parent, StmtKind::Expr, at.span);
                self.embed(id, Container::Node(id), other);
                self.finish_label(id);
                id
            }
        }
    }

    /// `if .. else if .. else`: one structure with a frame per branch.
    fn if_chain(&mut self, at: Place, first: &syn::ExprIf) -> NodeId {
        let head = self.span_until(first.span(), first.then_branch.span());
        let id = self.open(at, StructKind::If, head, "if".into());
        let mut cur = first;
        loop {
            self.if_branch(id, cur);
            let Some((else_token, else_expr)) = &cur.else_branch else {
                break;
            };
            match unwrap(else_expr) {
                syn::Expr::If(next) => cur = next,
                other => {
                    let f_span = self.span(other.span());
                    let f_head = self.span(else_token.span());
                    let role = frame(FrameRole::Else);
                    let f = self.add(Some(id), role, f_span, f_head, "else".into());
                    self.body_expr_into(f, other);
                    break;
                }
            }
        }
        id
    }

    /// The frame of one `if` / `else if` branch: condition and then-block.
    fn if_branch(&mut self, id: NodeId, branch: &syn::ExprIf) {
        let pattern = one_line(&self.p.text_of(branch.cond.span()));
        let f_span = self.span(branch.then_branch.span());
        let f_head = self.span(branch.cond.span());
        let role = FrameRole::Case {
            pattern: pattern.clone(),
            guard: None,
        };
        let f = self.add(Some(id), frame(role), f_span, f_head, pattern);
        let defs = self.embed_with(id, f, Container::Node(f), &branch.cond);
        self.nodes[f.index()].defs = defs;
        self.block_into(f, &branch.then_branch);
    }

    fn match_structure(&mut self, at: Place, m: &syn::ExprMatch) -> NodeId {
        let head = self.span_until(m.span(), m.brace_token.span.open());
        let label = one_line(&format!("match {}", self.p.text_of(m.expr.span())));
        let id = self.open(at, StructKind::Match, head, label);
        self.embed(id, Container::CondFrame(id), &m.expr);
        for arm in &m.arms {
            self.match_arm(id, arm);
        }
        id
    }

    /// The frame of one match arm: pattern, guard and body.
    fn match_arm(&mut self, id: NodeId, arm: &syn::Arm) {
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

    fn loop_structure(&mut self, at: Place, l: &syn::ExprLoop) -> NodeId {
        let label = self.label_text(&l.label);
        let (head, text) = self.header(l.span(), l.body.span());
        let kind = StructKind::Loop {
            label: label.clone(),
        };
        let id = self.open(at, kind, head, text);
        self.loop_body(id, label, &l.body, Vec::new());
        id
    }

    fn while_structure(&mut self, at: Place, w: &syn::ExprWhile) -> NodeId {
        let label = self.label_text(&w.label);
        let (head, text) = self.header(w.span(), w.body.span());
        let kind = StructKind::While {
            label: label.clone(),
        };
        let id = self.open(at, kind, head, text);
        let defs = self.embed_with(id, id, Container::CondFrame(id), &w.cond);
        self.loop_body(id, label, &w.body, defs);
        id
    }

    fn for_structure(&mut self, at: Place, f: &syn::ExprForLoop) -> NodeId {
        let label = self.label_text(&f.label);
        let (head, text) = self.header(f.span(), f.body.span());
        let kind = StructKind::For {
            label: label.clone(),
        };
        let id = self.open(at, kind, head, text);
        self.embed(id, Container::CondFrame(id), &f.expr);
        let mut defs = Vec::new();
        vars::pattern_defs(self.p, self.file, &f.pat, &mut defs);
        self.loop_body(id, label, &f.body, defs);
        id
    }

    /// A block expression; with a label it is a target for `break`.
    fn block_structure(&mut self, at: Place, b: &syn::ExprBlock) -> NodeId {
        let label = self.label_text(&b.label);
        let head = self.span(b.block.brace_token.span.open());
        let text = label
            .as_ref()
            .map_or("{ }".to_string(), |l| format!("{l}: {{ }}"));
        let kind = StructKind::Block {
            label: label.clone(),
        };
        let id = self.open(at, kind, head, text);
        if let Some(label) = label {
            self.scopes.push(Scope::Block { node: id, label });
        }
        let _ = self.body_frame(id, &b.block);
        if b.label.is_some() {
            self.scopes.pop();
        }
        id
    }

    /// A closure with a block body; its parameters are bindings of the body.
    fn closure_structure(&mut self, at: Place, c: &syn::ExprClosure) -> NodeId {
        let (head, text) = self.header(c.span(), c.body.span());
        let kind = StructKind::Closure {
            is_move: c.capture.is_some(),
            is_async: c.asyncness.is_some(),
        };
        let id = self.open(at, kind, head, text);
        self.scopes.push(Scope::FnLike(id));
        let body_span = self.span(c.body.span());
        let f = self.add_body_frame(id, body_span);
        let mut defs = Vec::new();
        for input in &c.inputs {
            vars::pattern_defs(self.p, self.file, input, &mut defs);
        }
        self.nodes[f.index()].defs = defs;
        self.body_expr_into(f, &c.body);
        self.scopes.pop();
        id
    }

    /// Body of a loop: the target of `break` and `continue` inside it.
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

    /// Body of a `try` or `async` block: `?` and `return` inside end there.
    fn fn_like_body(&mut self, id: NodeId, block: &syn::Block) {
        self.scopes.push(Scope::FnLike(id));
        let _ = self.body_frame(id, block);
        self.scopes.pop();
    }

    /// The `Body` frame of a structure, filled with the statements of `block`.
    fn body_frame(&mut self, structure: NodeId, block: &syn::Block) -> NodeId {
        let span = self.span(block.span());
        let f = self.add_body_frame(structure, span);
        self.block_into(f, block);
        f
    }
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
        self.use_full(name, span, mode, certain, None);
    }

    fn use_full(
        &mut self,
        name: String,
        span: proc_macro2::Span,
        mode: UseMode,
        certain: bool,
        stored_in: Option<(String, String)>,
    ) {
        if self.shadow.iter().any(|s| s.contains(&name)) {
            return;
        }
        let span = self.lower.span(span);
        self.lower.nodes[self.use_owner.index()].uses.push(VarUse {
            name,
            span,
            mode,
            certain,
            stored_in,
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

    /// Visit call arguments; closures with block bodies become structures and are
    /// remembered for the call site.
    fn call_args<'e>(&mut self, args: impl Iterator<Item = &'e syn::Expr>, call: &mut CallSite) {
        for arg in args {
            call.args.push(arg_local(arg));
            call.arg_texts
                .push(short(&one_line(&self.lower.p.text_of(arg.span())), 40));
            let arg = unwrap(arg);
            if is_structure(arg) {
                let parent = self.lower.container_node(&mut self.container);
                call.structures
                    .push(self.lower.structure(parent, arg, None));
                continue;
            }
            if let syn::Expr::Closure(_) = arg {
                call.inline_closures
                    .push(one_line(&self.lower.p.text_of(arg.span())));
            }
            self.visit_expr(arg);
        }
    }

    fn push_call(&mut self, call: CallSite) {
        self.lower.nodes[self.use_owner.index()].calls.push(call);
    }

    /// A macro call: its call site and the variables its input uses.
    fn macro_uses(&mut self, mac: &syn::Macro) {
        let span = self.lower.span(mac.path.span());
        self.push_call(new_call(CallKind::Macro, vars::segments(&mac.path), span));
        if let Some(args) = known_macro_args(mac) {
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
        // Not understood: every local-looking name might be a use.
        let mut names = Vec::new();
        vars::token_names(mac.tokens.clone(), &mut names);
        for (name, span) in names {
            self.use_name(name, span, UseMode::Read, false);
        }
    }

    /// A closure without a block body: part of the statement text. Its
    /// parameters hide outer variables of the same name.
    fn inline_closure(&mut self, c: &syn::ExprClosure) {
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
    }

    /// A method call: its call site, the receiver as written or read, the arguments.
    fn method_call(&mut self, m: &syn::ExprMethodCall) {
        let mut call = method_site(m, self.lower.span(m.method.span()));
        self.target(&m.receiver, vars::receiver_mode(&m.method.to_string()));
        self.call_args(m.args.iter(), &mut call);
        self.push_call(call);
    }

    /// A function call: its call site, the callee if it is computed, the arguments.
    fn function_call(&mut self, c: &syn::ExprCall) {
        let callee = unwrap(&c.func);
        if !matches!(callee, syn::Expr::Path(_)) {
            self.visit_expr(callee);
        }
        let mut call = function_site(self.lower.p, c, self.lower.span(c.func.span()));
        if let syn::Expr::Path(p) = callee
            && let Some(name) = vars::local_name(p)
        {
            // Calling a closure stored in a local variable.
            self.use_name(name, p.span(), UseMode::Read, true);
        }
        self.call_args(c.args.iter(), &mut call);
        self.push_call(call);
    }

    /// `S { field: x, .. }`: a local variable given as a field value is
    /// remembered as stored in that field.
    fn struct_literal(&mut self, st: &syn::ExprStruct) {
        let strukt = st
            .path
            .segments
            .last()
            .map(|s| s.ident.to_string())
            .unwrap_or_default();
        for fv in &st.fields {
            let field = vars::member_name(&fv.member);
            match unwrap(&fv.expr) {
                syn::Expr::Path(p) if vars::local_name(p).is_some() => {
                    let name = vars::local_name(p).expect("checked");
                    self.use_full(
                        name,
                        p.span(),
                        UseMode::Read,
                        true,
                        Some((strukt.clone(), field)),
                    );
                }
                other => self.visit_expr(other),
            }
        }
        if let Some(rest) = &st.rest {
            self.visit_expr(rest);
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
            syn::Expr::Closure(c) => self.inline_closure(c),
            syn::Expr::Macro(m) => self.macro_uses(&m.mac),
            syn::Expr::Path(p) => {
                if let Some(name) = vars::local_name(p) {
                    self.use_name(name, p.span(), UseMode::Read, true);
                }
            }
            syn::Expr::Assign(a) => {
                self.target(&a.left, UseMode::Assign);
                self.visit_expr(&a.right);
            }
            syn::Expr::Binary(b) if is_assign_op(&b.op) => {
                self.target(&b.left, UseMode::CompoundAssign);
                self.visit_expr(&b.right);
            }
            syn::Expr::Reference(r) if r.mutability.is_some() => {
                self.target(&r.expr, UseMode::RefMut);
            }
            syn::Expr::MethodCall(m) => self.method_call(m),
            syn::Expr::Call(c) => self.function_call(c),
            syn::Expr::Struct(st) => self.struct_literal(st),
            syn::Expr::Let(l) => {
                self.visit_expr(&l.expr);
                vars::pattern_defs(self.lower.p, self.lower.file, &l.pat, &mut self.defs);
            }
            other => {
                let leaves = matches!(
                    other,
                    syn::Expr::Try(_)
                        | syn::Expr::Return(_)
                        | syn::Expr::Break(_)
                        | syn::Expr::Continue(_)
                );
                if leaves && self.in_closure == 0 {
                    let kind = self.lower.exit_kind(other);
                    self.lower.exit(kind, self.owner);
                }
                syn::visit::visit_expr(self, other);
            }
        }
    }

    // Items inside expressions are not part of the control flow.
    fn visit_item(&mut self, _: &'ast syn::Item) {}
}
