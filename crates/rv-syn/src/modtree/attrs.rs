//! Attributes that decide how an item is treated: `#[path]`, `#[cfg(test)]`
//! and `#[test]`; counting the tests in code that is not shown.

use rv_ir::TestSummary;
use syn::visit::Visit;

/// The file named by `#[path = "..."]`.
pub(super) fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs.iter().find_map(|a| {
        if !a.path().is_ident("path") {
            return None;
        }
        let syn::Meta::NameValue(nv) = &a.meta else {
            return None;
        };
        match &nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Some(s.value()),
            _ => None,
        }
    })
}

/// `#[cfg(test)]`, `#[cfg(all(test, ...))]` — but not `#[cfg(not(test))]`.
pub(super) fn is_cfg_test(attrs: &[syn::Attribute]) -> bool {
    attrs.iter().any(|a| {
        if !a.path().is_ident("cfg") {
            return false;
        }
        let syn::Meta::List(list) = &a.meta else {
            return false;
        };
        cfg_mentions_test(list.tokens.clone())
    })
}

fn cfg_mentions_test(tokens: proc_macro2::TokenStream) -> bool {
    let mut prev_not = false;
    for tt in tokens {
        match tt {
            proc_macro2::TokenTree::Ident(i) if i == "test" => return true,
            proc_macro2::TokenTree::Ident(i) => prev_not = i == "not",
            proc_macro2::TokenTree::Group(g) => {
                if !prev_not && cfg_mentions_test(g.stream()) {
                    return true;
                }
                prev_not = false;
            }
            _ => {}
        }
    }
    false
}

/// `#[test]`, `#[tokio::test]`, `#[rstest]`-free heuristic: last path segment is `test`.
pub(super) fn has_test_attr(attrs: &[syn::Attribute]) -> bool {
    attrs
        .iter()
        .any(|a| a.path().segments.last().is_some_and(|s| s.ident == "test"))
}

/// Whether a function with these attributes is shown in the tree. A test is
/// counted instead; other `#[cfg(test)]` code is left out.
pub(super) fn is_shown(attrs: &[syn::Attribute], tests: &mut TestSummary) -> bool {
    if has_test_attr(attrs) {
        tests.test_fns += 1;
        return false;
    }
    !is_cfg_test(attrs)
}

/// Number of `#[test]` functions in what `f` lets the counter visit.
pub(super) fn count_tests(f: impl FnOnce(&mut TestCounter)) -> u32 {
    let mut v = TestCounter(0);
    f(&mut v);
    v.0
}

pub(super) struct TestCounter(u32);

impl<'ast> Visit<'ast> for TestCounter {
    fn visit_item_fn(&mut self, f: &'ast syn::ItemFn) {
        if has_test_attr(&f.attrs) {
            self.0 += 1;
        }
        syn::visit::visit_item_fn(self, f);
    }

    fn visit_impl_item_fn(&mut self, f: &'ast syn::ImplItemFn) {
        if has_test_attr(&f.attrs) {
            self.0 += 1;
        }
        syn::visit::visit_impl_item_fn(self, f);
    }
}
