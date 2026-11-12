//! Expression trees inside statements: operands in a column left of the
//! operation they feed.

use emath::{Pos2, Rect, Vec2, pos2, vec2};
use rv_ir::{Body, ExprTree, NodeId};

use crate::ctx::Ctx;
use crate::{ExprItem, PAD, Text};

/// Space between operand column and operation in expression trees.
const EXPR_GAP_X: f32 = 16.0;
const EXPR_GAP_Y: f32 = 4.0;

impl<'a> Ctx<'a> {
    /// The statement's expression tree, if its structure shows expressions.
    pub(crate) fn expr_tree(&self, id: NodeId) -> Option<&'a ExprTree> {
        let body: &'a Body = self.body;
        let tree = body.node(id).expr.as_ref()?;
        // Trivial trees (a single call without operands) add nothing.
        if tree.nodes.len() < 2 {
            return None;
        }
        let mut cur = body.node(id).parent;
        while let Some(n) = cur {
            if self.state.expressions.contains(&n) {
                return Some(tree);
            }
            cur = body.node(n).parent;
        }
        None
    }

    fn expr_box(&self, text: &str) -> Vec2 {
        vec2(self.m.text(text) + 2.0 * PAD, self.m.line_height + PAD)
    }

    /// Size of a subtree: operands in a column left of their operation.
    pub(crate) fn tree_size(&self, t: &ExprTree, i: usize) -> Vec2 {
        let node = &t.nodes[i];
        let own = self.expr_box(&node.text);
        if node.children.is_empty() {
            return own;
        }
        let kids: Vec<Vec2> = node
            .children
            .iter()
            .map(|&c| self.tree_size(t, c))
            .collect();
        let kids_w = kids.iter().map(|k| k.x).fold(0.0, f32::max);
        let kids_h = kids.iter().map(|k| k.y).sum::<f32>() + EXPR_GAP_Y * (kids.len() - 1) as f32;
        vec2(kids_w + EXPR_GAP_X + own.x, own.y.max(kids_h))
    }

    /// Places a subtree with its top-left corner at `min`; returns the
    /// operation's box.
    pub(crate) fn place_tree(&mut self, stmt: NodeId, t: &ExprTree, i: usize, min: Pos2) -> Rect {
        let node = &t.nodes[i];
        let size = self.tree_size(t, i);
        let own_size = self.expr_box(&node.text);
        let own = Rect::from_min_size(
            pos2(
                min.x + size.x - own_size.x,
                min.y + (size.y - own_size.y) / 2.0,
            ),
            own_size,
        );
        let mut y = min.y;
        if !node.children.is_empty() {
            let kids_h: f32 = node
                .children
                .iter()
                .map(|&c| self.tree_size(t, c).y)
                .sum::<f32>()
                + EXPR_GAP_Y * (node.children.len() - 1) as f32;
            y += (size.y - kids_h) / 2.0;
        }
        for &c in &node.children {
            let ks = self.tree_size(t, c);
            let child = self.place_tree(stmt, t, c, pos2(min.x, y));
            self.expr_wires
                .push((child.right_center(), own.left_center()));
            y += ks.y + EXPR_GAP_Y;
        }
        self.exprs.push(ExprItem {
            stmt,
            index: i,
            rect: own,
            text: Text {
                text: node.text.clone(),
                rect: Rect::from_min_size(
                    own.min + vec2(PAD, PAD / 2.0),
                    vec2(self.m.text(&node.text), self.m.line_height),
                ),
            },
        });
        own
    }
}
