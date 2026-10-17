//! Statement expressions as data-flow trees (for the expression level).

use rv_ir::{ExprKind, ExprNode, ExprTree, FileId, NodeId, Span};
use syn::spanned::Spanned;

use crate::source::Parsed;
use crate::vars;

/// Larger expressions are cut: the rest is shown as text.
const MAX_NODES: usize = 60;
const MAX_TEXT: usize = 28;

pub(crate) struct TreeBuilder<'a> {
    p: &'a Parsed,
    file: FileId,
    nodes: Vec<ExprNode>,
    /// Structures embedded in the statement, to link `match` etc. to their node.
    structures: Vec<(Span, NodeId, String)>,
}

impl<'a> TreeBuilder<'a> {
    pub(crate) fn new(
        p: &'a Parsed,
        file: FileId,
        structures: Vec<(Span, NodeId, String)>,
    ) -> Self {
        TreeBuilder {
            p,
            file,
            nodes: Vec::new(),
            structures,
        }
    }

    pub(crate) fn finish(self, root: usize) -> ExprTree {
        ExprTree {
            nodes: self.nodes,
            root,
        }
    }

    fn span(&self, s: proc_macro2::Span) -> Span {
        self.p.span(self.file, s)
    }

    pub(crate) fn node(
        &mut self,
        kind: ExprKind,
        text: impl Into<String>,
        children: Vec<usize>,
        span: proc_macro2::Span,
    ) -> usize {
        let span = self.span(span);
        self.nodes.push(ExprNode {
            kind,
            text: short(&text.into()),
            children,
            span,
        });
        self.nodes.len() - 1
    }

    fn text(&self, s: proc_macro2::Span) -> String {
        crate::body::one_line(&self.p.text_of(s))
    }

    /// `let pattern = init`
    pub(crate) fn bind(
        &mut self,
        pat: &syn::Pat,
        init: Option<&syn::Expr>,
        span: proc_macro2::Span,
    ) -> usize {
        let children = init.map(|e| vec![self.expr(e)]).unwrap_or_default();
        let text = format!("let {}", self.text(pat.span()));
        self.node(ExprKind::Bind, text, children, span)
    }

    pub(crate) fn macro_node(&mut self, mac: &syn::Macro, span: proc_macro2::Span) -> usize {
        use syn::parse::Parser;
        let name = format!(
            "{}!",
            mac.path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default()
        );
        let mut children = Vec::new();
        if vars::is_known_macro(&mac.path)
            && let Ok(args) =
                syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
                    .parse2(mac.tokens.clone())
        {
            for a in &args {
                children.push(self.expr(a));
            }
        }
        self.node(ExprKind::Macro, name, children, span)
    }

    pub(crate) fn expr(&mut self, e: &syn::Expr) -> usize {
        let span = e.span();
        if self.nodes.len() >= MAX_NODES {
            let t = self.text(span);
            return self.node(ExprKind::Other, t, Vec::new(), span);
        }
        // Structures are their own diagram nodes.
        let rv_span = self.span(span);
        if let Some((_, id, label)) = self
            .structures
            .iter()
            .find(|(s, ..)| s.bytes == rv_span.bytes)
            .cloned()
        {
            return self.node(ExprKind::Structure(id), label, Vec::new(), span);
        }
        match e {
            syn::Expr::Paren(p) => self.expr(&p.expr),
            syn::Expr::Group(g) => self.expr(&g.expr),
            syn::Expr::Path(p) => match vars::local_name(p) {
                Some(name) => self.node(ExprKind::Var, name, Vec::new(), span),
                None => {
                    let segs = vars::segments(&p.path);
                    let text = segs[segs.len().saturating_sub(2)..].join("::");
                    self.node(ExprKind::Literal, text, Vec::new(), span)
                }
            },
            syn::Expr::Lit(_) => {
                let t = self.text(span);
                self.node(ExprKind::Literal, t, Vec::new(), span)
            }
            syn::Expr::Call(c) => {
                let (text, mut children) = match &*c.func {
                    syn::Expr::Path(p) => {
                        let segs = vars::segments(&p.path);
                        (
                            format!("{}()", segs[segs.len().saturating_sub(2)..].join("::")),
                            Vec::new(),
                        )
                    }
                    other => ("call".to_string(), vec![self.expr(other)]),
                };
                for a in &c.args {
                    children.push(self.expr(a));
                }
                self.node(ExprKind::Call, text, children, span)
            }
            syn::Expr::MethodCall(m) => {
                let mut children = vec![self.expr(&m.receiver)];
                for a in &m.args {
                    children.push(self.expr(a));
                }
                self.node(ExprKind::Method, format!(".{}()", m.method), children, span)
            }
            syn::Expr::Macro(m) => self.macro_node(&m.mac, span),
            syn::Expr::Binary(b) => {
                let op = self.text(b.op.span());
                let (l, r) = (self.expr(&b.left), self.expr(&b.right));
                let kind = if op.ends_with('=') && !matches!(op.as_str(), "==" | "!=" | "<=" | ">=")
                {
                    ExprKind::Assign
                } else {
                    ExprKind::Operator
                };
                self.node(kind, op, vec![l, r], span)
            }
            syn::Expr::Assign(a) => {
                let (l, r) = (self.expr(&a.left), self.expr(&a.right));
                self.node(ExprKind::Assign, "=", vec![l, r], span)
            }
            syn::Expr::Unary(u) => {
                let op = self.text(u.op.span());
                let c = self.expr(&u.expr);
                self.node(ExprKind::Operator, op, vec![c], span)
            }
            syn::Expr::Field(f) => {
                let member = match &f.member {
                    syn::Member::Named(id) => id.to_string(),
                    syn::Member::Unnamed(i) => i.index.to_string(),
                };
                let c = self.expr(&f.base);
                self.node(ExprKind::Field, format!(".{member}"), vec![c], span)
            }
            syn::Expr::Index(i) => {
                let (b, x) = (self.expr(&i.expr), self.expr(&i.index));
                self.node(ExprKind::Index, "[ ]", vec![b, x], span)
            }
            syn::Expr::Struct(st) => {
                let name = st
                    .path
                    .segments
                    .last()
                    .map(|s| s.ident.to_string())
                    .unwrap_or_default();
                let children = st.fields.iter().map(|f| self.expr(&f.expr)).collect();
                self.node(ExprKind::Construct, format!("{name} {{}}"), children, span)
            }
            syn::Expr::Tuple(t) => {
                let children = t.elems.iter().map(|x| self.expr(x)).collect();
                self.node(ExprKind::Construct, "( )", children, span)
            }
            syn::Expr::Array(a) => {
                let children = a.elems.iter().map(|x| self.expr(x)).collect();
                self.node(ExprKind::Construct, "[ ]", children, span)
            }
            syn::Expr::Repeat(r) => {
                let (x, n) = (self.expr(&r.expr), self.expr(&r.len));
                self.node(ExprKind::Construct, "[x; n]", vec![x, n], span)
            }
            syn::Expr::Reference(r) => {
                let c = self.expr(&r.expr);
                let text = if r.mutability.is_some() { "&mut" } else { "&" };
                self.node(ExprKind::Ref, text, vec![c], span)
            }
            syn::Expr::Cast(c) => {
                let ty = self.text(c.ty.span());
                let x = self.expr(&c.expr);
                self.node(ExprKind::Cast, format!("as {ty}"), vec![x], span)
            }
            syn::Expr::Try(t) => {
                let c = self.expr(&t.expr);
                self.node(ExprKind::Try, "?", vec![c], span)
            }
            syn::Expr::Await(a) => {
                let c = self.expr(&a.base);
                self.node(ExprKind::Await, ".await", vec![c], span)
            }
            syn::Expr::Range(r) => {
                let mut children = Vec::new();
                if let Some(s) = &r.start {
                    children.push(self.expr(s));
                }
                if let Some(e) = &r.end {
                    children.push(self.expr(e));
                }
                self.node(ExprKind::Operator, "..", children, span)
            }
            syn::Expr::Closure(_) => {
                let t = self.text(span);
                self.node(ExprKind::Closure, t, Vec::new(), span)
            }
            syn::Expr::Return(r) => {
                let children = r
                    .expr
                    .as_deref()
                    .map(|x| vec![self.expr(x)])
                    .unwrap_or_default();
                self.node(ExprKind::Return, "return", children, span)
            }
            syn::Expr::Break(b) => {
                let children = b
                    .expr
                    .as_deref()
                    .map(|x| vec![self.expr(x)])
                    .unwrap_or_default();
                self.node(ExprKind::Return, "break", children, span)
            }
            _ => {
                let t = self.text(span);
                self.node(ExprKind::Other, t, Vec::new(), span)
            }
        }
    }
}

fn short(s: &str) -> String {
    if s.chars().count() <= MAX_TEXT {
        s.to_string()
    } else {
        s.chars().take(MAX_TEXT - 1).chain(['…']).collect()
    }
}
