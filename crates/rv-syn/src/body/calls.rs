//! Call sites: what a call, method call or macro call says about its
//! callee, receiver and arguments.

use rv_ir::{CallKind, CallSite, Span};
use syn::spanned::Spanned;

use super::syntax::unwrap;
use crate::source::Parsed;
use crate::vars;

/// A call site without receiver and arguments.
pub(super) fn new_call(kind: CallKind, path: Vec<String>, span: Span) -> CallSite {
    CallSite {
        kind,
        path,
        receiver: None,
        field: None,
        turbofish: None,
        literals: Vec::new(),
        structures: Vec::new(),
        inline_closures: Vec::new(),
        args: Vec::new(),
        arg_texts: Vec::new(),
        span,
    }
}

/// Call site of a method call with its receiver chain; `span` is the method name.
pub(super) fn method_site(m: &syn::ExprMethodCall, span: Span) -> CallSite {
    let mut path = Vec::new();
    let mut literals = Vec::new();
    let receiver = chain(&m.receiver, &mut path, &mut literals);
    let method = m.method.to_string();
    path.push(method.clone());
    if let Some(lit) = m.args.iter().find_map(vars::first_str_lit) {
        literals.push((method, lit));
    }
    let mut call = new_call(CallKind::Method, path, span);
    call.receiver = receiver;
    call.field = first_field(&m.receiver);
    call.literals = literals;
    call
}

/// Call site of a function call; `span` is the called expression. The path
/// is empty if that is not a path (`(make())()`).
pub(super) fn function_site(p: &Parsed, c: &syn::ExprCall, span: Span) -> CallSite {
    let callee = match unwrap(&c.func) {
        syn::Expr::Path(path) => Some(path),
        _ => None,
    };
    let path = callee.map_or_else(Vec::new, |path| vars::segments(&path.path));
    let mut call = new_call(CallKind::Function, path, span);
    call.turbofish = callee.and_then(|path| turbofish(p, &path.path));
    if let Some(lit) = c.args.iter().find_map(vars::first_str_lit) {
        let name = call.name().to_string();
        call.literals.push((name, lit));
    }
    call
}

/// Generic arguments of the last path segment (`u8` in `parse::<u8>`).
fn turbofish(p: &Parsed, path: &syn::Path) -> Option<String> {
    let last = path.segments.last()?;
    let syn::PathArguments::AngleBracketed(g) = &last.arguments else {
        return None;
    };
    let text = p.text_of(g.span());
    Some(
        text.trim_start_matches("::")
            .trim_start_matches('<')
            .trim_end_matches('>')
            .to_string(),
    )
}

/// Receiver chain of a method call: path segments and method names from the
/// root outwards; returns the local variable the chain starts at.
fn chain(
    e: &syn::Expr,
    path: &mut Vec<String>,
    literals: &mut Vec<(String, String)>,
) -> Option<String> {
    match unwrap(e) {
        syn::Expr::MethodCall(m) => {
            let root = chain(&m.receiver, path, literals);
            let method = m.method.to_string();
            if let Some(lit) = m.args.iter().find_map(vars::first_str_lit) {
                literals.push((method.clone(), lit));
            }
            path.push(method);
            root
        }
        syn::Expr::Call(c) => {
            if let syn::Expr::Path(p) = unwrap(&c.func) {
                path.extend(vars::segments(&p.path));
                if let Some(lit) = c.args.iter().find_map(vars::first_str_lit) {
                    literals.push((path.last().cloned().unwrap_or_default(), lit));
                }
            }
            None
        }
        syn::Expr::Path(p) => match vars::local_name(p) {
            Some(name) => Some(name),
            None => {
                path.extend(vars::segments(&p.path));
                None
            }
        },
        syn::Expr::Field(f) => chain(&f.base, path, literals),
        syn::Expr::Try(t) => chain(&t.expr, path, literals),
        syn::Expr::Await(a) => chain(&a.base, path, literals),
        syn::Expr::Reference(r) => chain(&r.expr, path, literals),
        _ => None,
    }
}

/// First field accessed on the root of a receiver chain:
/// `cmd_tx` in `self.cmd_tx.send()`, `geteilt` in `self.geteilt.stand.lock()`.
fn first_field(e: &syn::Expr) -> Option<String> {
    match unwrap(e) {
        syn::Expr::Field(f) => match unwrap(&f.base) {
            syn::Expr::Path(_) => Some(vars::member_name(&f.member)),
            _ => first_field(&f.base),
        },
        syn::Expr::MethodCall(m) => first_field(&m.receiver),
        syn::Expr::Try(t) => first_field(&t.expr),
        syn::Expr::Await(a) => first_field(&a.base),
        syn::Expr::Reference(r) => first_field(&r.expr),
        _ => None,
    }
}

/// The local variable an argument consists of: `x`, `&x`, `&mut x`, `x.clone()`.
pub(super) fn arg_local(e: &syn::Expr) -> Option<String> {
    match unwrap(e) {
        syn::Expr::Path(p) => vars::local_name(p),
        syn::Expr::Reference(r) => arg_local(&r.expr),
        syn::Expr::MethodCall(m) if m.method == "clone" && m.args.is_empty() => {
            arg_local(&m.receiver)
        }
        _ => None,
    }
}

/// Arguments of a macro known to take expressions (`println!`, `vec!`, ...);
/// `None` if the macro is unknown or its input is something else.
pub(super) fn known_macro_args(mac: &syn::Macro) -> Option<Vec<syn::Expr>> {
    use syn::parse::Parser;
    if !vars::is_known_macro(&mac.path) {
        return None;
    }
    syn::punctuated::Punctuated::<syn::Expr, syn::Token![,]>::parse_terminated
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
        })
        .ok()
}
