//! Tooltips of what is under the pointer: a wire, an expression or an item.

use egui::{Response, RichText, Ui};
use rv_analysis::EdgeKind;
use rv_ir::{Body, ExprNode, NodeId, NodeKind, Project, StmtKind};
use rv_layout::{ExprItem, Item};

use super::analysis::Analysis;
use super::scene::Scene;
use crate::wires::{Wire, WireKind};

/// At most this many lines of code are shown for an item.
const MAX_LINES: usize = 16;
/// Labels of the ends of a wire are cut after this many chars.
const MAX_END_CHARS: usize = 70;

/// What is under the pointer; a wire lies over an expression, an
/// expression over its statement.
#[derive(Clone, Copy, Default)]
pub(super) struct Hover<'a> {
    pub(super) item: Option<&'a Item>,
    pub(super) expr: Option<&'a ExprItem>,
    /// Index into the wires of the scene.
    pub(super) wire: Option<usize>,
}

/// Shows the tooltip of the topmost hovered thing at the pointer.
pub(super) fn show(response: Response, project: &Project, scene: &Scene, hover: Hover) {
    if let Some(i) = hover.wire {
        let w = &scene.wires[i];
        response.on_hover_ui_at_pointer(|ui| wire(ui, scene.body, scene.analysis, w));
    } else if let Some(e) = hover.expr {
        let node = scene.expr_node(e);
        response.on_hover_ui_at_pointer(|ui| expression(ui, project, node));
    } else if let Some(item) = hover.item {
        response.on_hover_ui_at_pointer(|ui| node(ui, project, scene.body, item.node));
    }
}

fn wire(ui: &mut Ui, body: &Body, a: &Analysis, w: &Wire) {
    let spec = &a.specs[w.spec];
    let (what, certain) = match spec.kind {
        WireKind::Channel => {
            let ch = &a.conc.channels[spec.source];
            (format!("channel ({})", ch.api), true)
        }
        WireKind::Shared => {
            let g = &a.conc.shared[spec.source];
            (
                format!("shared state ({}), used from another lane", g.kind),
                true,
            )
        }
        _ => {
            let e = &a.flow.edges[spec.source];
            let what = match e.kind {
                EdgeKind::Data => "data",
                EdgeKind::Mutation => "changes it",
                EdgeKind::PossibleMutation => "may change it (method call on a `mut` variable)",
                EdgeKind::LoopCarried => "carried into the next loop iteration (shift register)",
            };
            (what.to_string(), e.certain)
        }
    };
    ui.label(RichText::new(&w.name).monospace().strong());
    ui.weak(what);
    if !certain {
        ui.weak("guessed from the tokens of an unexpanded macro");
    }
    for (dir, id) in [("from", spec.from), ("to", spec.to)] {
        let n = body.node(id);
        let label: String = n.label.chars().take(MAX_END_CHARS).collect();
        ui.monospace(format!("{dir} line {}: {label}", n.span.start.line));
    }
}

fn expression(ui: &mut Ui, project: &Project, node: &ExprNode) {
    ui.weak(format!("{:?}", node.kind));
    ui.label(RichText::new(project.snippet(&node.span).to_string()).monospace());
    ui.weak("click: show in code");
}

fn node(ui: &mut Ui, project: &Project, body: &Body, id: NodeId) {
    let node = body.node(id);
    let what = match &node.kind {
        NodeKind::Structure { kind, .. } => format!(
            "{} structure · {} statements",
            kind.name(),
            body.stmt_count(id)
        ),
        NodeKind::Frame { .. } => format!("frame · {} items", node.children.len()),
        NodeKind::Stmt {
            kind: StmtKind::Macro { name },
        } => {
            format!("macro {name}! (not expanded)")
        }
        NodeKind::Stmt { .. } => "statement".to_string(),
    };
    ui.weak(format!("{what} · line {}", node.span.start.line));
    let snippet = project.snippet(&node.span);
    let mut shown: String = snippet
        .lines()
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if snippet.lines().count() > MAX_LINES {
        shown.push_str("\n…");
    }
    ui.label(RichText::new(shown).monospace());
    ui.weak("click: show in code · double click: open in editor");
}
