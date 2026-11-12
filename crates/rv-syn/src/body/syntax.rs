//! Classifying expressions and items: what becomes a structure, what kind of
//! statement something is.

use rv_ir::StmtKind;
use syn::spanned::Spanned;

use crate::source::Parsed;
use crate::text::one_line;

/// The expression inside parentheses and invisible groups.
pub(super) fn unwrap(e: &syn::Expr) -> &syn::Expr {
    match e {
        syn::Expr::Paren(p) => unwrap(&p.expr),
        syn::Expr::Group(g) => unwrap(&g.expr),
        other => other,
    }
}

/// Expressions shown as structures (frames) rather than as text.
pub(super) fn is_structure(e: &syn::Expr) -> bool {
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
pub(super) fn can_have_value(e: &syn::Expr) -> bool {
    match unwrap(e) {
        syn::Expr::If(i) => i.else_branch.is_some(),
        syn::Expr::While(_) | syn::Expr::ForLoop(_) | syn::Expr::Loop(_) => false,
        _ => true,
    }
}

/// `+=`, `-=`, `<<=` and the like.
pub(super) fn is_assign_op(op: &syn::BinOp) -> bool {
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

/// Kind of a statement that consists of the expression `e`.
pub(super) fn classify(p: &Parsed, e: &syn::Expr) -> StmtKind {
    match unwrap(e) {
        syn::Expr::Call(_) => StmtKind::Call,
        syn::Expr::MethodCall(_) => StmtKind::MethodCall,
        syn::Expr::Macro(m) => StmtKind::Macro {
            name: p.text_of(m.mac.path.span()),
        },
        syn::Expr::Assign(_) => StmtKind::Assign,
        syn::Expr::Binary(b) if is_assign_op(&b.op) => StmtKind::Assign,
        syn::Expr::Await(a) => classify(p, &a.base),
        syn::Expr::Try(t) => classify(p, &t.expr),
        _ => StmtKind::Expr,
    }
}

/// Label of an item declared inside a body: keyword and name.
pub(super) fn item_label(p: &Parsed, item: &syn::Item) -> String {
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
        syn::Item::Impl(i) => format!("impl {}", p.text_of(i.self_ty.span())),
        other => one_line(&p.text_of(other.span())),
    }
}
