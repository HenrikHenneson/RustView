//! Lowering function bodies to nested structures (statement level).
//!
//! Every statement becomes a node. Expressions that open their own scope or
//! control flow (`if`, `match`, loops, blocks, closures with a block body,
//! `async` blocks) become structures. A statement that *is* such an expression
//! (`let x = match ...`) is shown as the structure itself with a prefix;
//! otherwise the structures are embedded in the statement node.

use rv_ir::{Body, NodeKind, Span, StructKind};
use syn::spanned::Spanned;

use crate::source::Source;
use crate::vars;

mod arena;
mod calls;
mod descent;
mod syntax;

use arena::{Lower, Scope};

/// Lower the function `sig` with the body `block`; `fn_span` is the whole
/// function, `name_span` its name.
pub(crate) fn lower(
    src: Source,
    sig: &syn::Signature,
    block: &syn::Block,
    fn_span: Span,
    name_span: Span,
) -> Body {
    let mut l = Lower::new(src);
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
    l.nodes[root.index()].defs = vars::param_defs(src.parsed, src.file, sig);
    l.scopes.push(Scope::FnLike(root));
    let block_span = l.span(block.span());
    let frame = l.add_body_frame(root, block_span);
    l.block_into(frame, block);
    Body {
        nodes: l.nodes,
        root,
        exits: l.exits,
    }
}
