//! The program graph as SVG, for the export.

use egui::{Color32, Pos2, Rect, vec2};

use super::layout::{Arrow, Block, BlockKind, GraphLayout, LINE, LineStyle, PAD};
use crate::export::{Svg, TextStyle};
use crate::style::{SHARED, THREAD, light};

/// Border of the main thread's box.
const MAIN_BORDER: Color32 = Color32::from_gray(60);
const BOX_RADIUS: f32 = 6.0;
const LABEL_FONT: f32 = 11.0;
/// Estimated width of a character of an arrow label.
const LABEL_CHAR_WIDTH: f32 = 6.6;
const LABEL_HEIGHT: f32 = 15.0;

/// The program graph as SVG in light colors.
pub fn graph_svg(g: &GraphLayout) -> String {
    let mut svg = Svg::new(g.size);
    for a in &g.arrows {
        arrow(&mut svg, a);
    }
    for b in &g.boxes {
        block(&mut svg, b);
    }
    svg.finish()
}

fn arrow(svg: &mut Svg, a: &Arrow) {
    let pts = &a.points;
    if a.dashed {
        svg.polyline(pts, a.width, a.color, Some((3.0, 4.0)));
    } else if a.head {
        svg.arrow(pts, a.width, a.color, None);
    } else {
        svg.polyline(pts, a.width, a.color, None);
    }
    if let Some(label) = &a.label {
        arrow_label(svg, pts[pts.len() / 2], label, a.color);
    }
}

/// The name of a channel on a white patch in the middle of its arrow.
fn arrow_label(svg: &mut Svg, mid: Pos2, label: &str, color: Color32) {
    let w = label.chars().count() as f32 * LABEL_CHAR_WIDTH + 4.0;
    svg.rect(
        Rect::from_center_size(mid, vec2(w, LABEL_HEIGHT)),
        light::FILL,
        None,
        3.0,
    );
    svg.label_fit(
        mid - vec2(w / 2.0 - 2.0, 6.5),
        label,
        TextStyle::new(LABEL_FONT, color, true, false),
        w - 4.0,
    );
}

fn block(svg: &mut Svg, b: &Block) {
    let border = match b.kind {
        BlockKind::Thread(0) => MAIN_BORDER,
        BlockKind::Thread(_) => THREAD,
        BlockKind::Resource(_) => SHARED,
    };
    svg.rect(b.rect, light::FILL, Some((2.0, border)), BOX_RADIUS);
    for (i, (text, style)) in b.lines.iter().enumerate() {
        let (size, color, mono, bold) = match style {
            LineStyle::Title => (13.0, light::TITLE, false, true),
            LineStyle::Normal => (12.0, light::TEXT, true, false),
            LineStyle::Weak => (12.0, light::WEAK_TEXT, false, false),
        };
        svg.label(
            b.rect.min + vec2(PAD, PAD + i as f32 * LINE),
            text,
            TextStyle::new(size, color, mono, bold),
        );
    }
}
