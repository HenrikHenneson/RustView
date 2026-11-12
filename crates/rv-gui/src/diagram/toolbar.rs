//! The toolbar above the diagram: direction, wire mode, expression level,
//! expansion, zoom and export.

use egui::{Ui, Vec2};
use rv_ir::{Body, NodeId, NodeKind};
use rv_layout::Orientation;

use super::DiagramView;
use super::fn_view::{BUDGET, FnView};
use crate::wires::{self, WireMode};

impl DiagramView {
    pub(super) fn toolbar(&mut self, ui: &mut Ui, body: &Body, qual_path: &str, signature: &str) {
        let view = self.views.get_mut(qual_path).expect("inserted by caller");
        // "Fit" refers to the diagram and canvas of the last frame.
        let fit = self
            .model
            .layout_size()
            .zip(self.canvas.filter(|c| c.is_positive()).map(|c| c.size()));
        ui.horizontal_wrapped(|ui| {
            ui.strong(qual_path).on_hover_text(signature);
            ui.separator();
            orientation_buttons(ui, &mut self.orientation);
            ui.separator();
            wire_mode_buttons(ui, &mut self.wire_mode);
            ui.separator();
            expressions_button(ui, body, self.selected, view);
            ui.separator();
            expansion_buttons(ui, body, view);
            ui.separator();
            zoom_buttons(ui, view, fit);
            if ui
                .button("Export…")
                .on_hover_text("Save the whole diagram as SVG or PNG")
                .clicked()
            {
                self.export_requested = true;
            }
            ui.weak("drag: move · wheel: scroll · Ctrl+wheel: zoom");
        });
    }
}

fn orientation_buttons(ui: &mut Ui, orientation: &mut Orientation) {
    ui.selectable_value(orientation, Orientation::Vertical, "↓ top-down")
        .on_hover_text("Statements top to bottom, cases side by side");
    ui.selectable_value(orientation, Orientation::Horizontal, "→ left-right")
        .on_hover_text("Statements left to right, cases stacked");
}

fn wire_mode_buttons(ui: &mut Ui, mode: &mut WireMode) {
    ui.label("wires:");
    ui.selectable_value(mode, WireMode::Auto, "auto")
        .on_hover_text(format!(
            "All wires up to {}, otherwise only those of the selected node",
            wires::AUTO_LIMIT
        ));
    ui.selectable_value(mode, WireMode::All, "all")
        .on_hover_text("All data wires; those of the selected node stand out");
    ui.selectable_value(mode, WireMode::Selected, "selected")
        .on_hover_text("Only wires of the selected or hovered node");
    ui.selectable_value(mode, WireMode::Off, "off");
}

/// Expression level for the structure around the selection (or the whole
/// function).
fn expressions_button(ui: &mut Ui, body: &Body, selected: Option<NodeId>, view: &mut FnView) {
    let target = selected
        .and_then(|n| enclosing_structure(body, n))
        .unwrap_or(body.root);
    let on = view.state.expressions.contains(&target);
    let what = if target == body.root {
        "the whole function".to_string()
    } else {
        format!("`{}`", body.node(target).label)
    };
    if ui
        .selectable_label(on, "ƒx Expressions")
        .on_hover_text(format!(
            "Show statements as data flow of their expressions in {what} (select a structure to limit it)"
        ))
        .clicked()
    {
        view.toggle_expressions(target);
    }
}

/// `node` itself or the closest structure around it.
fn enclosing_structure(body: &Body, node: NodeId) -> Option<NodeId> {
    let mut cur = Some(node);
    while let Some(x) = cur {
        if matches!(body.node(x).kind, NodeKind::Structure { .. }) {
            return Some(x);
        }
        cur = body.node(x).parent;
    }
    None
}

fn expansion_buttons(ui: &mut Ui, body: &Body, view: &mut FnView) {
    if ui.button("Expand all").clicked() {
        view.expand_all(body);
    }
    if ui.button("Collapse all").clicked() {
        view.collapse_all(body);
    }
    if ui
        .button("Auto")
        .on_hover_text(format!(
            "Expand from the outside in until about {BUDGET} items are visible"
        ))
        .clicked()
    {
        view.auto_expand(body);
    }
}

/// Zoom buttons; `fit` is the size of the diagram and of the canvas, once
/// both are known.
fn zoom_buttons(ui: &mut Ui, view: &mut FnView, fit: Option<(Vec2, Vec2)>) {
    if ui.button("−").clicked() {
        view.zoom_out();
    }
    if ui
        .button(format!("{:.0}%", view.zoom * 100.0))
        .on_hover_text("Reset zoom")
        .clicked()
    {
        view.reset_zoom();
    }
    if ui.button("+").clicked() {
        view.zoom_in();
    }
    if ui
        .button("Fit")
        .on_hover_text("Show the whole diagram (never above 100%)")
        .clicked()
        && let Some((diagram, canvas)) = fit
    {
        view.fit(diagram, canvas);
    }
}
