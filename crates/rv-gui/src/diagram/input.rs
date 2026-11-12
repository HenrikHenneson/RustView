//! Input on the diagram canvas: panning and zooming, and what a click
//! means. Clicks are turned into events; nothing is changed here but the
//! scroll position.

use egui::{Pos2, Rect, Response, Ui};
use rv_ir::{NodeId, Span};
use rv_layout::{Item, ItemKind};

use super::fn_view::{Control, FnView};
use super::paint::Canvas;
use super::scene::Scene;

/// What a click on the canvas means.
pub(super) enum CanvasEvent {
    /// A sub-expression of statement `stmt` was clicked.
    ShowExpr {
        stmt: NodeId,
        span: Span,
    },
    /// A control in the header of a structure was clicked.
    Control(Control, NodeId),
    Select(NodeId),
    /// Double click on an item.
    Open(NodeId),
}

/// Dragging moves the diagram, the wheel scrolls it and zooming (Ctrl+wheel,
/// pinch) keeps the point under the pointer in place.
pub(super) fn pan_and_zoom(ui: &Ui, response: &Response, rect: Rect, view: &mut FnView) {
    if response.dragged() {
        view.pan += response.drag_delta();
    }
    if response.hovered() {
        let (zoom_delta, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
        if zoom_delta != 1.0
            && let Some(p) = response.hover_pos()
        {
            view.zoom_about(p - rect.min, zoom_delta);
        } else {
            view.pan += scroll;
        }
    }
}

/// The clicks of this frame: at most one click event and, for a double
/// click on an item, an [`CanvasEvent::Open`] after it.
pub(super) fn clicks(response: &Response, canvas: &Canvas, scene: &Scene) -> Vec<CanvasEvent> {
    let mut events = Vec::new();
    let Some(world) = response.interact_pointer_pos().map(|p| canvas.to_world(p)) else {
        return events;
    };
    if response.clicked() {
        events.extend(click_at(scene, world));
    }
    if response.double_clicked()
        && let Some(item) = scene.layout.hit(world)
        && control_at(item, world).is_none()
    {
        events.push(CanvasEvent::Open(item.node));
    }
    events
}

/// What a single click at `world` (diagram coordinates) hits: an
/// expression, a control of a structure or an item.
fn click_at(scene: &Scene, world: Pos2) -> Option<CanvasEvent> {
    if let Some(e) = scene.expr_at(world) {
        return Some(CanvasEvent::ShowExpr {
            stmt: e.stmt,
            span: scene.expr_node(e).span.clone(),
        });
    }
    let item = scene.layout.hit(world)?;
    Some(match control_at(item, world) {
        Some(control) => CanvasEvent::Control(control, item.node),
        None => CanvasEvent::Select(item.node),
    })
}

/// The header control of `item` at `world`, if it is a structure.
fn control_at(item: &Item, world: Pos2) -> Option<Control> {
    let ItemKind::Structure(s) = &item.kind else {
        return None;
    };
    let hit = |r: Rect| r.expand(2.0).contains(world);
    if hit(s.toggle) {
        return Some(Control::Toggle);
    }
    let sel = s.selector.as_ref()?;
    if sel.prev.is_some_and(hit) {
        Some(Control::PrevCase)
    } else if sel.next.is_some_and(hit) {
        Some(Control::NextCase)
    } else if hit(sel.mode.rect) {
        Some(Control::CaseMode)
    } else {
        None
    }
}
