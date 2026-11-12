//! Statement expressions as data-flow trees (for the expression level).

use rv_ir::{ExprKind, ExprNode, ExprTree, FileId, NodeId, Span};
use syn::spanned::Spanned;

use crate::source::Parsed;
use crate::text::{one_line, short};
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
            text: short(&text.into(), MAX_TEXT),
            children,
            span,
        });
        self.nodes.len() - 1
    }

    fn text(&self, s: proc_macro2::Span) -> String {
        one_line(&self.p.text_of(s))
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
            syn::Expr::Macro(m) => self.macro_node(&m.mac, span),
            _ => {
                let (kind, text, operands) = self
                    .operation(e)
                    .or_else(|| construction(e))
                    .unwrap_or_else(|| self.leaf(e));
                let children = operands.into_iter().map(|x| self.expr(x)).collect();
                self.node(kind, text, children, span)
            }
        }
    }

    /// Kind, text and operands of an expression that computes something from
    /// other expressions: calls, operators, access, conversion, exits.
    fn operation<'e>(&self, e: &'e syn::Expr) -> Option<Parts<'e>> {
        let parts = match e {
            syn::Expr::Call(c) => call_parts(c),
            syn::Expr::MethodCall(m) => {
                let operands = std::iter::once(&*m.receiver).chain(&m.args).collect();
                (ExprKind::Method, format!(".{}()", m.method), operands)
            }
            syn::Expr::Binary(b) => {
                let op = self.text(b.op.span());
                let kind = if op.ends_with('=') && !matches!(op.as_str(), "==" | "!=" | "<=" | ">=")
                {
                    ExprKind::Assign
                } else {
                    ExprKind::Operator
                };
                (kind, op, vec![&*b.left, &*b.right])
            }
            syn::Expr::Assign(a) => (ExprKind::Assign, "=".into(), vec![&*a.left, &*a.right]),
            syn::Expr::Unary(u) => (ExprKind::Operator, self.text(u.op.span()), vec![&*u.expr]),
            syn::Expr::Range(r) => {
                let operands = r.start.iter().chain(&r.end).map(|x| &**x).collect();
                (ExprKind::Operator, "..".into(), operands)
            }
            syn::Expr::Field(f) => {
                let text = format!(".{}", vars::member_name(&f.member));
                (ExprKind::Field, text, vec![&*f.base])
            }
            syn::Expr::Index(i) => (ExprKind::Index, "[ ]".into(), vec![&*i.expr, &*i.index]),
            syn::Expr::Reference(r) => {
                let text = if r.mutability.is_some() { "&mut" } else { "&" };
                (ExprKind::Ref, text.into(), vec![&*r.expr])
            }
            syn::Expr::Cast(c) => {
                let text = format!("as {}", self.text(c.ty.span()));
                (ExprKind::Cast, text, vec![&*c.expr])
            }
            syn::Expr::Try(t) => (ExprKind::Try, "?".into(), vec![&*t.expr]),
            syn::Expr::Await(a) => (ExprKind::Await, ".await".into(), vec![&*a.base]),
            syn::Expr::Return(r) => {
                let operands = r.expr.as_deref().into_iter().collect();
                (ExprKind::Return, "return".into(), operands)
            }
            syn::Expr::Break(b) => {
                let operands = b.expr.as_deref().into_iter().collect();
                (ExprKind::Return, "break".into(), operands)
            }
            _ => return None,
        };
        Some(parts)
    }

    /// An expression shown without operands: a variable, a constant, or
    /// anything else as text.
    fn leaf<'e>(&self, e: &'e syn::Expr) -> Parts<'e> {
        let (kind, text) = match e {
            syn::Expr::Path(p) => match vars::local_name(p) {
                Some(name) => (ExprKind::Var, name),
                None => (ExprKind::Literal, last_two_segments(&p.path)),
            },
            syn::Expr::Lit(_) => (ExprKind::Literal, self.text(e.span())),
            syn::Expr::Closure(_) => (ExprKind::Closure, self.text(e.span())),
            _ => (ExprKind::Other, self.text(e.span())),
        };
        (kind, text, Vec::new())
    }
}

/// Kind, text and operands of an expression node.
type Parts<'e> = (ExprKind, String, Vec<&'e syn::Expr>);

/// A function call: named after its path; anything else that is called is
/// the first operand.
fn call_parts(c: &syn::ExprCall) -> Parts<'_> {
    let (text, mut operands) = match &*c.func {
        syn::Expr::Path(p) => (format!("{}()", last_two_segments(&p.path)), Vec::new()),
        other => ("call".to_string(), vec![other]),
    };
    operands.extend(&c.args);
    (ExprKind::Call, text, operands)
}

/// Kind, text and elements of an expression that builds a value from parts:
/// struct literal, tuple, array.
fn construction(e: &syn::Expr) -> Option<Parts<'_>> {
    let (text, elements) = match e {
        syn::Expr::Struct(st) => {
            let name = st
                .path
                .segments
                .last()
                .map(|s| s.ident.to_string())
                .unwrap_or_default();
            let fields = st.fields.iter().map(|f| &f.expr).collect();
            (format!("{name} {{}}"), fields)
        }
        syn::Expr::Tuple(t) => ("( )".to_string(), t.elems.iter().collect()),
        syn::Expr::Array(a) => ("[ ]".to_string(), a.elems.iter().collect()),
        syn::Expr::Repeat(r) => ("[x; n]".to_string(), vec![&*r.expr, &*r.len]),
        _ => return None,
    };
    Some((ExprKind::Construct, text, elements))
}

/// The path shortened to its last two segments (`Vec::new`).
fn last_two_segments(path: &syn::Path) -> String {
    let segs = vars::segments(path);
    segs[segs.len().saturating_sub(2)..].join("::")
}
