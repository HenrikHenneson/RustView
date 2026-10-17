//! Syntactic variable facts: bindings in patterns, local-looking names,
//! names inside macro invocations.

use proc_macro2::{TokenStream, TokenTree};
use rv_ir::{FileId, VarDef};
use syn::spanned::Spanned;

use crate::source::Parsed;

/// Macros whose arguments are ordinary, comma-separated expressions
/// (possibly starting with a format string).
const EXPR_MACROS: &[&str] = &[
    "format",
    "print",
    "println",
    "eprint",
    "eprintln",
    "panic",
    "write",
    "writeln",
    "format_args",
    "assert",
    "assert_eq",
    "assert_ne",
    "debug_assert",
    "debug_assert_eq",
    "debug_assert_ne",
    "vec",
    "trace",
    "debug",
    "info",
    "warn",
    "error",
    "log",
    "t",
    "unreachable",
    "todo",
    "unimplemented",
    "dbg",
];

/// Words that look like local names in macro tokens but are not.
const KEYWORDS: &[&str] = &[
    "as", "async", "await", "break", "const", "continue", "crate", "dyn", "else", "enum", "extern",
    "false", "fn", "for", "if", "impl", "in", "let", "loop", "match", "mod", "move", "mut", "pub",
    "ref", "return", "static", "struct", "super", "trait", "true", "type", "unsafe", "use",
    "where", "while",
];

/// Methods of std types that only read their receiver (`&self`).
const READ_METHODS: &[&str] = &[
    "as_bytes",
    "as_deref",
    "as_ref",
    "as_slice",
    "as_str",
    "borrow",
    "chars",
    "clone",
    "cloned",
    "contains",
    "contains_key",
    "copied",
    "ends_with",
    "eq",
    "first",
    "get",
    "is_empty",
    "is_err",
    "is_none",
    "is_ok",
    "is_some",
    "iter",
    "keys",
    "last",
    "len",
    "lines",
    "lock",
    "read",
    "split",
    "starts_with",
    "to_owned",
    "to_string",
    "to_vec",
    "trim",
    "values",
    "load",
];

/// Methods of std types that change their receiver (`&mut self`).
const MUT_METHODS: &[&str] = &[
    "append",
    "as_mut",
    "borrow_mut",
    "clear",
    "dedup",
    "drain",
    "entry",
    "extend",
    "get_mut",
    "get_or_insert",
    "get_or_insert_with",
    "insert",
    "iter_mut",
    "pop",
    "pop_back",
    "pop_front",
    "push",
    "push_back",
    "push_front",
    "push_str",
    "remove",
    "replace",
    "reserve",
    "resize",
    "retain",
    "reverse",
    "set",
    "sort",
    "sort_by",
    "sort_by_key",
    "sort_unstable",
    "swap",
    "take",
    "truncate",
    "values_mut",
];

/// How a method call uses its receiver, guessed from the method name.
pub(crate) fn receiver_mode(method: &str) -> rv_ir::UseMode {
    if READ_METHODS.contains(&method) {
        rv_ir::UseMode::Read
    } else if MUT_METHODS.contains(&method) {
        rv_ir::UseMode::RefMut
    } else {
        rv_ir::UseMode::MethodRecv
    }
}

pub(crate) fn is_known_macro(path: &syn::Path) -> bool {
    path.segments
        .last()
        .is_some_and(|s| EXPR_MACROS.contains(&s.ident.to_string().as_str()))
}

/// `true` for names that can be local bindings (`x`, `_tmp`, `self`), not
/// for types, constants or enum variants.
pub(crate) fn is_local_name(name: &str) -> bool {
    name.chars()
        .next()
        .is_some_and(|c| c.is_lowercase() || c == '_')
        && name != "_"
        && !KEYWORDS.contains(&name)
}

/// `x` in expression position (no path, no generics).
pub(crate) fn local_name(p: &syn::ExprPath) -> Option<String> {
    if p.qself.is_some() || p.path.leading_colon.is_some() || p.path.segments.len() != 1 {
        return None;
    }
    let seg = &p.path.segments[0];
    if !seg.arguments.is_none() {
        return None;
    }
    let name = seg.ident.to_string();
    is_local_name(&name).then_some(name)
}

/// Bindings introduced by a pattern.
pub(crate) fn pattern_defs(p: &Parsed, file: FileId, pat: &syn::Pat, out: &mut Vec<VarDef>) {
    match pat {
        syn::Pat::Ident(pi) => {
            let name = syn::ext::IdentExt::unraw(&pi.ident).to_string();
            if is_local_name(&name) {
                out.push(VarDef {
                    name,
                    span: p.span(file, pi.ident.span()),
                    mutable: pi.mutability.is_some(),
                    ty: None,
                });
            }
            if let Some((_, sub)) = &pi.subpat {
                pattern_defs(p, file, sub, out);
            }
        }
        syn::Pat::Tuple(t) => t.elems.iter().for_each(|e| pattern_defs(p, file, e, out)),
        syn::Pat::TupleStruct(t) => t.elems.iter().for_each(|e| pattern_defs(p, file, e, out)),
        syn::Pat::Slice(s) => s.elems.iter().for_each(|e| pattern_defs(p, file, e, out)),
        syn::Pat::Struct(s) => s
            .fields
            .iter()
            .for_each(|f| pattern_defs(p, file, &f.pat, out)),
        // All alternatives bind the same names.
        syn::Pat::Or(o) => {
            if let Some(first) = o.cases.first() {
                pattern_defs(p, file, first, out);
            }
        }
        syn::Pat::Reference(r) => pattern_defs(p, file, &r.pat, out),
        syn::Pat::Type(t) => {
            let before = out.len();
            pattern_defs(p, file, &t.pat, out);
            let ty = p.text_of(t.ty.span());
            out[before..]
                .iter_mut()
                .for_each(|d| d.ty = Some(ty.clone()));
        }
        syn::Pat::Paren(pp) => pattern_defs(p, file, &pp.pat, out),
        syn::Pat::Guard(g) => pattern_defs(p, file, &g.pat, out),
        _ => {}
    }
}

/// Parameters of a function signature (`self` included).
pub(crate) fn param_defs(p: &Parsed, file: FileId, sig: &syn::Signature) -> Vec<VarDef> {
    let mut out = Vec::new();
    for arg in &sig.inputs {
        match arg {
            syn::FnArg::Receiver(r) => out.push(VarDef {
                name: "self".into(),
                span: p.span(file, r.self_token.span),
                mutable: r.mutability.is_some()
                    || matches!(r.kind, syn::ReceiverKind::Reference(_, _, Some(_))),
                ty: None,
            }),
            syn::FnArg::Typed(t) => {
                let before = out.len();
                pattern_defs(p, file, &t.pat, &mut out);
                // `x: &mut T` is a mutable place for our purposes.
                let mutable = matches!(&*t.ty, syn::Type::Reference(r) if r.mutability.is_some());
                let ty = p.text_of(t.ty.span());
                for d in &mut out[before..] {
                    d.mutable |= mutable;
                    d.ty = Some(ty.clone());
                }
            }
        }
    }
    out
}

/// Path segments of a path expression or path (`std::thread::spawn`).
pub(crate) fn segments(path: &syn::Path) -> Vec<String> {
    path.segments.iter().map(|s| s.ident.to_string()).collect()
}

/// First string literal in an expression (`"x"`, `"x".to_string()`, `String::from("x")`),
/// not looking into closures.
pub(crate) fn first_str_lit(e: &syn::Expr) -> Option<String> {
    match e {
        syn::Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) => Some(s.value()),
        syn::Expr::MethodCall(m) => {
            first_str_lit(&m.receiver).or_else(|| m.args.iter().find_map(first_str_lit))
        }
        syn::Expr::Call(c) => c.args.iter().find_map(first_str_lit),
        syn::Expr::Reference(r) => first_str_lit(&r.expr),
        syn::Expr::Paren(p) => first_str_lit(&p.expr),
        syn::Expr::Macro(m) if m.mac.path.is_ident("format") => {
            syn::parse2::<syn::LitStr>(m.mac.tokens.clone().into_iter().take(1).collect())
                .ok()
                .map(|l| l.value())
        }
        _ => None,
    }
}

/// Names used as `{name}` (and `{:width$}`) in a format string.
pub(crate) fn format_arg_names(fmt: &str) -> Vec<String> {
    let chars: Vec<char> = fmt.chars().collect();
    let mut names = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '{' {
            i += 1;
            continue;
        }
        if chars.get(i + 1) == Some(&'{') {
            i += 2;
            continue;
        }
        let end = (i..chars.len())
            .find(|&k| chars[k] == '}')
            .unwrap_or(chars.len());
        let inner: String = chars[i + 1..end].iter().collect();
        let (arg, spec) = inner.split_once(':').unwrap_or((&inner, ""));
        let arg = arg.trim();
        if is_local_name(arg) && arg.chars().all(|c| c.is_alphanumeric() || c == '_') {
            names.push(arg.to_string());
        }
        // `{:width$}` / `{:.prec$}`
        for part in spec.split(|c: char| !(c.is_alphanumeric() || c == '_' || c == '$')) {
            if let Some(name) = part.strip_suffix('$')
                && is_local_name(name)
            {
                names.push(name.to_string());
            }
        }
        i = end + 1;
    }
    names
}

/// Local-looking identifiers in a token stream (for macros we cannot parse).
/// Skips fields/methods (`.x`), path segments (`a::b`), calls of macros
/// (`m!`) and named arguments (`key: value` keeps `value`).
pub(crate) fn token_names(ts: TokenStream, out: &mut Vec<(String, proc_macro2::Span)>) {
    let tokens: Vec<TokenTree> = ts.into_iter().collect();
    for (i, tt) in tokens.iter().enumerate() {
        match tt {
            TokenTree::Group(g) => token_names(g.stream(), out),
            TokenTree::Ident(id) => {
                let name = id.to_string();
                if !is_local_name(&name) {
                    continue;
                }
                let at = |k: Option<usize>| k.and_then(|k| tokens.get(k));
                let punct = |k: Option<usize>, c: char| matches!(at(k), Some(TokenTree::Punct(p)) if p.as_char() == c);
                let (p1, p2, n1) = (i.checked_sub(1), i.checked_sub(2), Some(i + 1));
                let after_path_sep = punct(p1, ':') && punct(p2, ':');
                let before_colon = punct(n1, ':'); // path head `a::b` or argument name `key: value`
                if punct(p1, '.') || after_path_sep || before_colon || punct(n1, '!') {
                    continue;
                }
                out.push((name, id.span()));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_args_are_found() {
        assert_eq!(
            format_arg_names("x = {x}, {y:?} {{z}} {0} {:>w$} {name:.prec$}"),
            ["x", "y", "w", "name", "prec"]
        );
    }

    #[test]
    fn token_names_skip_fields_paths_and_macros() {
        let ts: TokenStream = "target: log_target, a.b, c::d, e!(f), g(h)"
            .parse()
            .unwrap();
        let mut out = Vec::new();
        token_names(ts, &mut out);
        let names: Vec<_> = out.into_iter().map(|(n, _)| n).collect();
        assert_eq!(names, ["log_target", "a", "f", "g", "h"]);
    }
}
