//! State of one layout run and the texts shown for a node. Measuring
//! (`measure.rs`) and placing (`place.rs`) both ask here what a node shows,
//! so sizes and positions agree.

use std::collections::{HashMap, HashSet};

use emath::{Pos2, Vec2};
use rv_ir::{Body, ExitKind, FrameRole, NodeId, NodeKind};

use crate::metrics::Metrics;
use crate::view_state::{ViewState, case_frames};
use crate::{ExprItem, Item, Notes, Orientation, PAD, View};

pub(crate) struct Ctx<'a> {
    pub(crate) body: &'a Body,
    pub(crate) state: &'a ViewState,
    defaults: &'a HashSet<NodeId>,
    pub(crate) vertical: bool,
    pub(crate) m: Metrics,
    /// Measured size of every node, by node index.
    pub(crate) sizes: Vec<Vec2>,
    stmt_counts: Vec<usize>,
    exits: HashMap<NodeId, ExitCounts>,
    pub(crate) notes: &'a Notes,
    pub(crate) items: Vec<Item>,
    pub(crate) exprs: Vec<ExprItem>,
    pub(crate) expr_wires: Vec<(Pos2, Pos2)>,
    pub(crate) index: HashMap<NodeId, usize>,
}

/// Texts in the header of a structure.
pub(crate) struct StructTexts {
    pub(crate) prefix: Option<String>,
    pub(crate) title: String,
    /// `?` count, size when collapsed and exits to this structure.
    pub(crate) info: Option<String>,
}

/// State of the case selector of a structure with alternatives.
#[derive(Clone, Copy)]
pub(crate) struct Cases {
    pub(crate) showing_all: bool,
    pub(crate) index: usize,
    pub(crate) count: usize,
}

impl<'a> Ctx<'a> {
    pub(crate) fn new(body: &'a Body, view: View<'a>, metrics: Metrics, notes: &'a Notes) -> Self {
        let mut exits: HashMap<NodeId, ExitCounts> = HashMap::new();
        for e in &body.exits {
            if let Some(t) = e.target {
                exits.entry(t).or_default().add(&e.kind);
            }
        }
        Ctx {
            body,
            state: view.state,
            defaults: view.defaults,
            vertical: view.orientation == Orientation::Vertical,
            m: metrics,
            sizes: vec![Vec2::ZERO; body.nodes.len()],
            stmt_counts: stmt_counts(body),
            exits,
            notes,
            items: Vec::new(),
            index: HashMap::new(),
            exprs: Vec::new(),
            expr_wires: Vec::new(),
        }
    }

    pub(crate) fn expanded(&self, id: NodeId) -> bool {
        self.state.is_expanded(self.defaults, id)
    }

    /// Height of a statement line or a structure header.
    pub(crate) fn row(&self) -> f32 {
        self.m.line_height + 2.0 * PAD
    }

    pub(crate) fn stmt_text(&self, id: NodeId) -> String {
        truncate(&self.body.node(id).label, self.m.max_label_chars)
    }

    pub(crate) fn stmt_badges(&self, id: NodeId) -> Option<String> {
        let node = self.body.node(id);
        let mut b = Vec::new();
        if node.tries > 0 {
            b.push(if node.tries == 1 {
                "?".to_string()
            } else {
                format!("?{}", node.tries)
            });
        }
        if node.value {
            b.push("=>".to_string());
        }
        (!b.is_empty()).then(|| b.join(" "))
    }

    pub(crate) fn struct_texts(&self, id: NodeId) -> StructTexts {
        let node = self.body.node(id);
        let NodeKind::Structure { prefix, .. } = &node.kind else {
            unreachable!()
        };
        let title = truncate(&node.label, self.m.max_label_chars);
        let mut info = Vec::new();
        if node.tries > 0 {
            info.push(format!("?{}", node.tries));
        }
        if !self.expanded(id) {
            info.push(format!("{} stmts", self.stmt_counts[id.index()]));
        }
        if let Some(e) = self.exits.get(&id).and_then(ExitCounts::summary) {
            info.push(e);
        }
        let info = (!info.is_empty()).then(|| info.join("  ·  "));
        StructTexts {
            prefix: prefix.clone(),
            title,
            info,
        }
    }

    /// The case selector, if the structure has alternatives and is expanded.
    pub(crate) fn cases(&self, id: NodeId) -> Option<Cases> {
        let count = case_frames(self.body, id).len();
        if count < 2 || !self.expanded(id) {
            return None;
        }
        Some(Cases {
            showing_all: self.state.shows_all_cases(self.body, id),
            index: self.state.case_index(self.body, id),
            count,
        })
    }

    /// Case pattern / `else` / `cond`; `None` for plain bodies.
    pub(crate) fn frame_title(&self, id: NodeId) -> Option<String> {
        let node = self.body.node(id);
        match &node.kind {
            NodeKind::Frame {
                role: FrameRole::Body,
            } => None,
            NodeKind::Frame { .. } => Some(truncate(&node.label, self.m.max_label_chars)),
            _ => None,
        }
    }
}

/// Label of the button that switches between all cases and one at a time.
pub(crate) fn mode_text(showing_all: bool) -> &'static str {
    if showing_all { "one" } else { "all" }
}

#[derive(Default)]
struct ExitCounts {
    returns: u32,
    tries: u32,
    breaks: u32,
    continues: u32,
}

impl ExitCounts {
    fn add(&mut self, kind: &ExitKind) {
        match kind {
            ExitKind::Return => self.returns += 1,
            ExitKind::Try => self.tries += 1,
            ExitKind::Break { .. } => self.breaks += 1,
            ExitKind::Continue { .. } => self.continues += 1,
        }
    }

    fn summary(&self) -> Option<String> {
        let parts: Vec<String> = [
            ("return", self.returns),
            ("?", self.tries),
            ("break", self.breaks),
            ("continue", self.continues),
        ]
        .iter()
        .filter(|(_, n)| *n > 0)
        .map(|(what, n)| format!("{what}×{n}"))
        .collect();
        (!parts.is_empty()).then(|| format!("exits: {}", parts.join(" ")))
    }
}

/// Statements in and below every node, by node index.
fn stmt_counts(body: &Body) -> Vec<usize> {
    let mut counts = vec![0; body.nodes.len()];
    // Children always have higher ids than their parents.
    for i in (0..body.nodes.len()).rev() {
        let node = &body.nodes[i];
        let own = usize::from(matches!(node.kind, NodeKind::Stmt { .. }));
        counts[i] += own;
        if let Some(p) = node.parent {
            counts[p.index()] += counts[i];
        }
    }
    counts
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}
