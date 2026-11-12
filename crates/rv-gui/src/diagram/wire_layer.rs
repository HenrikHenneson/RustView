//! The wires on top of the boxes on screen: which ones are shown, which one
//! is under the pointer, and the hint when data wires are hidden.

use egui::{FontId, Pos2, Rect, vec2};

use super::paint::Canvas;
use super::scene::{Scene, WireFilter};
use crate::wires::{self, WireMode, WireStyle};

/// The pointer hits a wire within this distance (in points on screen).
const HIT_TOLERANCE: f32 = 5.0;
/// Opacity of wires that do not belong to the node in focus.
const UNRELATED_ALPHA: f32 = 0.18;
/// Font size of the hint about hidden wires.
const HINT_SIZE: f32 = 12.0;

/// Paints the wires `filter` shows; returns the one under `pointer`
/// (diagram coordinates), which is drawn highlighted.
pub(super) fn paint(
    canvas: &Canvas,
    scene: &Scene,
    filter: &WireFilter,
    pointer: Option<Pos2>,
) -> Option<usize> {
    if filter.mode == WireMode::Selected && filter.focus.is_none() && filter.data_wires > 0 {
        hidden_wires_hint(canvas, filter.data_wires);
    }
    let shown = |w: &wires::Wire| filter.shows(w);
    let hovered =
        pointer.and_then(|p| wires::hit(scene.wires, shown, p, HIT_TOLERANCE / canvas.zoom));
    let style = WireStyle {
        dark: canvas.style.dark,
        zoom: canvas.zoom,
        origin: canvas.origin,
        clip: canvas.rect,
    };
    for (i, w) in scene.wires.iter().enumerate().filter(|(_, w)| shown(w)) {
        // With a node in focus the other wires step back.
        let related = w.kind.is_concurrency() || filter.focus.is_none() || filter.touches_focus(w);
        let alpha = if related || filter.mode == WireMode::Selected {
            1.0
        } else {
            UNRELATED_ALPHA
        };
        wires::paint(&canvas.painter, w, &style, alpha, hovered == Some(i));
    }
    hovered
}

/// Says in the bottom-left corner how to get to see the data wires.
fn hidden_wires_hint(canvas: &Canvas, data_wires: usize) {
    let (painter, style) = (&canvas.painter, &canvas.style);
    let hint = format!(
        "{data_wires} data wires · select or hover a node to see its wires (wires: all shows everything)"
    );
    let galley = painter.layout_no_wrap(hint, FontId::proportional(HINT_SIZE), style.weak);
    let pos = canvas.rect.left_bottom() + vec2(8.0, -8.0 - galley.size().y);
    let bg = Rect::from_min_size(pos, galley.size()).expand(4.0);
    painter.rect_filled(bg, 4.0, style.bg);
    painter.galley(pos, galley, style.weak);
}
