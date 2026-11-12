//! What a diagram shows, independent of where it is drawn: the layout with
//! its wires and the marks derived from them. The screen painter and the SVG
//! export both draw from this, so they agree on what is where.

use std::collections::HashSet;

use egui::{Color32, Pos2, Rect, pos2, vec2};
use rv_ir::{Body, ExprNode, NodeId};
use rv_layout::{ExprItem, Item, ItemKind, Layout, StructItem};

use super::analysis::Analysis;
use super::palette::Palette;
use crate::wires::{self, Wire, WireMode};

/// Size of a shift register terminal.
const REGISTER_SIZE: f32 = 9.0;
/// Distance of the first shift register terminal below the header.
const REGISTER_TOP: f32 = 8.0;
/// Distance between the shift register terminals of a loop.
const REGISTER_STEP: f32 = 14.0;
/// How far the tip of an exit marker is right of its structure.
const EXIT_MARK_OFFSET: f32 = 7.0;
/// An exit marker is at most this far below the top of the exit.
const EXIT_MARK_MAX_DROP: f32 = 11.0;

/// One function as laid out, with everything needed to draw it.
#[derive(Clone, Copy)]
pub(super) struct Scene<'a> {
    pub(super) body: &'a Body,
    pub(super) layout: &'a Layout,
    pub(super) analysis: &'a Analysis,
    pub(super) wires: &'a [Wire],
    /// Tips of the exit markers, see [`exit_marks`].
    pub(super) exit_marks: &'a [Pos2],
}

impl<'a> Scene<'a> {
    /// The expression node an expression item stands for.
    pub(super) fn expr_node(&self, e: &ExprItem) -> &'a ExprNode {
        let tree = self
            .body
            .node(e.stmt)
            .expr
            .as_ref()
            .expect("expression items come from trees");
        &tree.nodes[e.index]
    }

    /// The expression item at `pos` (diagram coordinates).
    pub(super) fn expr_at(&self, pos: Pos2) -> Option<&'a ExprItem> {
        self.layout.exprs.iter().find(|e| e.rect.contains(pos))
    }

    /// Color of the note of `node`: its lane color, otherwise the palette's
    /// note color (shift registers / expressions).
    pub(super) fn note_color(&self, node: NodeId, style: &Palette) -> Color32 {
        self.analysis
            .lane_color
            .get(&node)
            .copied()
            .unwrap_or_else(|| style.note())
    }

    /// Shift register terminals of an expanded loop: one pair per register
    /// on both borders, below the header.
    pub(super) fn register_terminals(&self, item: &Item, s: &StructItem) -> Vec<Rect> {
        let count = match self.analysis.registers.get(&item.node) {
            Some(regs) if s.expanded => regs.len(),
            _ => 0,
        };
        let mut terminals = Vec::with_capacity(2 * count);
        for k in 0..count {
            let y = s.header.max.y + REGISTER_TOP + k as f32 * REGISTER_STEP;
            for x in [item.rect.min.x, item.rect.max.x] {
                terminals.push(Rect::from_center_size(
                    pos2(x, y),
                    vec2(REGISTER_SIZE, REGISTER_SIZE),
                ));
            }
        }
        terminals
    }
}

/// Tips of the exit markers: where `return`, `break`, `continue` and `?`
/// leave a structure, right of its border at the height of the exit.
pub(super) fn exit_marks(body: &Body, layout: &Layout) -> Vec<Pos2> {
    let mut seen: HashSet<(NodeId, i32)> = HashSet::new();
    let mut tips = Vec::new();
    for e in &body.exits {
        let (Some(target), Some(from)) = (e.target, layout.item(e.from)) else {
            continue;
        };
        let y = from.rect.min.y + (from.rect.height() / 2.0).min(EXIT_MARK_MAX_DROP);
        // Every structure the exit leaves on its way to the target.
        let mut cur = body.node(e.from).parent;
        while let Some(n) = cur.filter(|&n| n != target) {
            if let Some(item) = layout.item(n)
                && matches!(item.kind, ItemKind::Structure(_))
                && seen.insert((n, y as i32))
            {
                tips.push(pos2(item.rect.max.x + EXIT_MARK_OFFSET, y));
            }
            cur = body.node(n).parent;
        }
    }
    tips
}

/// Which wires are shown: concurrency wires always, data wires depending on
/// the mode and on the node in focus.
pub(super) struct WireFilter<'a> {
    /// The mode in effect (`Auto` resolved by the number of data wires).
    pub(super) mode: WireMode,
    /// Selected or hovered node.
    pub(super) focus: Option<NodeId>,
    pub(super) data_wires: usize,
    scene: Scene<'a>,
}

impl<'a> WireFilter<'a> {
    pub(super) fn new(scene: Scene<'a>, mode: WireMode, focus: Option<NodeId>) -> Self {
        let data_wires = scene
            .wires
            .iter()
            .filter(|w| !w.kind.is_concurrency())
            .count();
        let mode = match mode {
            WireMode::Auto if data_wires <= wires::AUTO_LIMIT => WireMode::All,
            WireMode::Auto => WireMode::Selected,
            other => other,
        };
        WireFilter {
            mode,
            focus,
            data_wires,
            scene,
        }
    }

    /// Whether `w` starts or ends at the node in focus.
    pub(super) fn touches_focus(&self, w: &Wire) -> bool {
        self.focus
            .is_some_and(|n| w.touches(n, &self.scene.analysis.specs))
    }

    pub(super) fn shows(&self, w: &Wire) -> bool {
        w.kind.is_concurrency()
            || match self.mode {
                WireMode::All => true,
                WireMode::Off => false,
                _ => self.touches_focus(w),
            }
    }
}
