//! The module graph as SVG, for the export.

use egui::{Color32, vec2};

use super::model::{LINE, LineKind, ModuleLayout, PAD, edge_width};
use crate::export::{Svg, TextStyle};
use crate::style::{THREAD, light};

/// Free space to the right of and below the graph.
const MARGIN: f32 = 80.0;
const EDGE_COLOR: Color32 = Color32::from_gray(150);
const BORDER_COLOR: Color32 = Color32::from_gray(90);
const BOX_RADIUS: f32 = 5.0;

/// The module graph (as laid out) as SVG in light colors.
pub(crate) fn module_svg(layout: &ModuleLayout) -> String {
    let mut svg = Svg::new(layout.size + vec2(MARGIN, MARGIN));
    for (i, e) in layout.graph.edges.iter().enumerate() {
        svg.arrow(layout.curve(i), edge_width(e), EDGE_COLOR, None);
    }
    for (i, n) in layout.graph.nodes.iter().enumerate() {
        let border = if n.spawns > 0 {
            (2.0, THREAD)
        } else {
            (1.0, BORDER_COLOR)
        };
        svg.rect(layout.box_of(i), light::FILL, Some(border), BOX_RADIUS);
        for (k, (text, kind)) in layout.labels(i).iter().enumerate() {
            let (size, color) = match kind {
                LineKind::Name => (12.5, light::TITLE),
                LineKind::Threads => (11.5, THREAD),
                LineKind::Size => (11.5, light::WEAK_TEXT),
            };
            svg.label(
                layout.box_of(i).min + vec2(PAD, PAD + k as f32 * LINE),
                text,
                TextStyle::new(size, color, *kind == LineKind::Name, false),
            );
        }
    }
    svg.finish()
}
