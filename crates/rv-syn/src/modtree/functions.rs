//! Adding functions to the project: name, signature, body and the functions
//! nested in the body.

use rv_ir::{FnId, FnOwner, Function, ModuleId, Param, ParamMode};
use syn::spanned::Spanned;

use super::Builder;
use super::attrs::has_test_attr;
use crate::source::{Parsed, Source};

/// A function with a body as it stands in the syntax tree.
pub(super) struct FnItem<'a> {
    pub(super) sig: &'a syn::Signature,
    pub(super) block: &'a syn::Block,
    /// The whole item, attributes included.
    pub(super) span: proc_macro2::Span,
}

impl<'a> FnItem<'a> {
    pub(super) fn of_impl(f: &'a syn::ImplItemFn) -> Self {
        FnItem {
            sig: &f.sig,
            block: &f.block,
            span: f.span(),
        }
    }

    pub(super) fn of_free(f: &'a syn::ItemFn) -> Self {
        FnItem {
            sig: &f.sig,
            block: &f.block,
            span: f.span(),
        }
    }
}

/// Owner of the functions in an `impl` block.
pub(super) fn impl_owner(p: &Parsed, imp: &syn::ItemImpl) -> FnOwner {
    FnOwner::Impl {
        self_ty: p.text_of(imp.self_ty.span()),
        trait_: imp.trait_.as_ref().map(|(path, _)| p.text_of(path.span())),
    }
}

impl Builder {
    /// Add a function to module `m`, lower its body and add the functions
    /// nested in it.
    pub(super) fn function(
        &mut self,
        src: Source,
        m: ModuleId,
        owner: FnOwner,
        item: FnItem,
    ) -> FnId {
        let FnItem { sig, block, span } = item;
        let span = src.span(span);
        let id = FnId(self.project.functions.len() as u32);
        let name = syn::ext::IdentExt::unraw(&sig.ident).to_string();
        let name_span = src.span(sig.ident.span());
        let body = crate::body::lower(src, sig, block, span.clone(), name_span.clone());
        let qual_path = self.qual_path(m, &owner, &name);
        self.project.functions.push(Function {
            name,
            qual_path,
            module: m,
            owner,
            sig: signature(src.parsed, sig),
            span,
            name_span,
            body_span: Some(src.span(block.span())),
            body: Some(body),
        });
        self.project.modules[m.index()].functions.push(id);
        self.nested_items(src, m, id, block);
        id
    }

    /// Path of a function for display and call resolution: module, owner
    /// type or trait or enclosing function, name.
    fn qual_path(&self, m: ModuleId, owner: &FnOwner, name: &str) -> String {
        match owner {
            FnOwner::Free => format!("{}::{name}", self.project.module(m).qual_name()),
            FnOwner::Impl { self_ty, .. } => format!(
                "{}::{}::{name}",
                self.project.module(m).qual_name(),
                strip_generics(self_ty)
            ),
            FnOwner::Trait { name: t } => {
                format!("{}::{t}::{name}", self.project.module(m).qual_name())
            }
            FnOwner::Nested { parent } => {
                format!("{}::{name}", self.project.function(*parent).qual_path)
            }
        }
    }

    /// Functions and impls declared at the top level of a function body.
    fn nested_items(&mut self, src: Source, m: ModuleId, parent: FnId, block: &syn::Block) {
        for stmt in &block.stmts {
            let syn::Stmt::Item(item) = stmt else {
                continue;
            };
            match item {
                syn::Item::Fn(f) if !has_test_attr(&f.attrs) => {
                    self.function(src, m, FnOwner::Nested { parent }, FnItem::of_free(f));
                }
                syn::Item::Impl(imp) => {
                    let owner = impl_owner(src.parsed, imp);
                    for ii in &imp.items {
                        if let syn::ImplItem::Fn(f) = ii {
                            self.function(src, m, owner.clone(), FnItem::of_impl(f));
                        }
                    }
                }
                _ => {}
            }
        }
    }
}

fn signature(p: &Parsed, sig: &syn::Signature) -> rv_ir::Signature {
    let params = sig
        .inputs
        .iter()
        .map(|arg| match arg {
            syn::FnArg::Receiver(r) => {
                let mode = match &r.kind {
                    syn::ReceiverKind::Reference(_, _, Some(_)) => ParamMode::RefMut,
                    syn::ReceiverKind::Reference(_, _, None) => ParamMode::Ref,
                    _ => ParamMode::Value,
                };
                Param {
                    pat: "self".into(),
                    ty: p.text_of(r.span()),
                    mode,
                }
            }
            syn::FnArg::Typed(t) => {
                let mode = match &*t.ty {
                    syn::Type::Reference(r) if r.mutability.is_some() => ParamMode::RefMut,
                    syn::Type::Reference(_) => ParamMode::Ref,
                    _ => ParamMode::Value,
                };
                Param {
                    pat: p.text_of(t.pat.span()),
                    ty: p.text_of(t.ty.span()),
                    mode,
                }
            }
        })
        .collect();
    rv_ir::Signature {
        text: p.text_of(sig.span()),
        is_async: sig.asyncness.is_some(),
        is_unsafe: matches!(sig.safety, syn::Safety::Unsafe(_)),
        is_const: sig.constness.is_some(),
        params,
        ret: match &sig.output {
            syn::ReturnType::Default => None,
            syn::ReturnType::Type(_, ty) => Some(p.text_of(ty.span())),
        },
    }
}

fn strip_generics(ty: &str) -> &str {
    ty.split('<').next().unwrap_or(ty).trim()
}
