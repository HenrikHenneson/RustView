//! Geometry for RustView structure diagrams.
//!
//! Sizes are computed bottom-up (a structure is as large as its frames, a
//! frame as large as its items), then everything is placed top-down. All
//! coordinates are in diagram units at zoom 1; the GUI scales and pans.

use std::collections::{HashMap, HashSet};

mod ctx;
mod expr;
pub mod layered;
mod measure;
mod metrics;
mod place;
mod view_state;

use emath::{Pos2, Rect, Vec2};
use rv_ir::{Body, NodeId};

pub use metrics::Metrics;
pub use view_state::{MAX_SIDE_BY_SIDE, ViewState, case_frames, default_expanded};

/// Padding inside statement boxes and structure headers.
const PAD: f32 = 4.0;
/// Padding inside frames.
const FRAME_PAD: f32 = 8.0;
/// Space between items of a frame and between frames.
const GAP: f32 = 8.0;
/// Width of a structure's border around its frames.
const BORDER: f32 = 4.0;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum Orientation {
    /// Statements top to bottom, cases side by side.
    #[default]
    Vertical,
    /// Statements left to right, cases stacked (closer to LabVIEW).
    Horizontal,
}

/// How a body is shown: what the user changed, what is expanded otherwise
/// and the direction of the statements.
#[derive(Clone, Copy, Debug)]
pub struct View<'a> {
    pub state: &'a ViewState,
    /// Structures that are expanded unless `state` says otherwise.
    pub defaults: &'a HashSet<NodeId>,
    pub orientation: Orientation,
}

#[derive(Clone, Debug)]
pub struct Layout {
    /// Parents before children (paint order).
    pub items: Vec<Item>,
    pub size: Vec2,
    index: HashMap<NodeId, usize>,
    /// Expression nodes of statements in expression mode.
    pub exprs: Vec<ExprItem>,
    /// Operand -> operation, as (from, to) points.
    pub expr_wires: Vec<(Pos2, Pos2)>,
}

/// One node of a statement's expression tree.
#[derive(Clone, Debug)]
pub struct ExprItem {
    pub stmt: NodeId,
    /// Index into the statement's `ExprTree::nodes`.
    pub index: usize,
    pub rect: Rect,
    pub text: Text,
}

impl Layout {
    pub fn item(&self, node: NodeId) -> Option<&Item> {
        self.index.get(&node).map(|&i| &self.items[i])
    }

    /// Innermost item at `pos`.
    pub fn hit(&self, pos: Pos2) -> Option<&Item> {
        self.items.iter().rev().find(|i| i.rect.contains(pos))
    }
}

#[derive(Clone, Debug)]
pub struct Item {
    pub node: NodeId,
    pub rect: Rect,
    /// Nesting depth in structures (root = 0).
    pub depth: u32,
    pub kind: ItemKind,
}

#[derive(Clone, Debug)]
pub enum ItemKind {
    Structure(Box<StructItem>),
    Frame {
        /// Case pattern / `else` / `cond`; `None` for plain bodies.
        title: Option<Text>,
    },
    Stmt {
        text: Text,
        /// `?2`, `=>` (value) etc.
        badges: Option<Text>,
        /// Annotation from the caller (e.g. "thread 'worker'").
        note: Option<Text>,
    },
}

#[derive(Clone, Debug)]
pub struct StructItem {
    pub expanded: bool,
    pub header: Rect,
    pub toggle: Rect,
    pub prefix: Option<Text>,
    pub title: Text,
    /// Annotation from the caller (e.g. "thread 'worker'").
    pub note: Option<Text>,
    /// Exits to this structure, or the size when collapsed.
    pub info: Option<Text>,
    pub selector: Option<Selector>,
}

/// Controls of a case structure.
#[derive(Clone, Debug)]
pub struct Selector {
    pub showing_all: bool,
    pub index: usize,
    pub count: usize,
    /// Arrows and position; only in one-at-a-time mode.
    pub prev: Option<Rect>,
    pub position: Option<Text>,
    pub next: Option<Rect>,
    /// Switches between "all" and "one".
    pub mode: Text,
}

/// A piece of text and where it goes (top-left at `rect.min`).
#[derive(Clone, Debug)]
pub struct Text {
    pub text: String,
    pub rect: Rect,
}

/// Short annotations shown next to a node's label (lane badges etc.).
pub type Notes = HashMap<NodeId, String>;

/// Lays out `body` as shown by `view`; `notes` are placed next to the labels
/// of their nodes.
pub fn layout(body: &Body, view: View<'_>, metrics: Metrics, notes: &Notes) -> Layout {
    let mut ctx = ctx::Ctx::new(body, view, metrics, notes);
    let size = ctx.measure(body.root);
    ctx.place(body.root, Rect::from_min_size(Pos2::ZERO, size), 0);
    Layout {
        items: ctx.items,
        size,
        index: ctx.index,
        exprs: ctx.exprs,
        expr_wires: ctx.expr_wires,
    }
}

#[cfg(test)]
mod tests;
