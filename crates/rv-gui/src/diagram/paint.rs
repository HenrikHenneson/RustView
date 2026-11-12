//! Painting the diagram on screen: boxes, labels, exit markers and
//! expression trees of the visible part, from the scene of this frame.

use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, Vec2,
    vec2,
};
use rv_ir::{NodeId, NodeKind, StmtKind, StructKind};
use rv_layout::{ExprItem, Item, ItemKind, Selector, StructItem, Text};

use super::palette::{
    EMPTY_HINT_INSET, EXPR_RADIUS, EXPR_WIRE, FONT_SIZE, FRAME_TITLE_HEIGHT, HEADER_TINT,
    ITEM_RADIUS, LANE_BAR, LANE_OUTLINE, OUTLINE, Palette, expr_color, structure_border,
};
use super::scene::Scene;

/// Below this font size (in points) labels are not drawn.
const MIN_TEXT_SIZE: f32 = 4.0;
/// Line widths that scale with the zoom do not get thinner than this factor.
const MIN_LINE_SCALE: f32 = 0.5;
/// Outline of the item under the pointer.
const HOVER_OUTLINE: f32 = 1.5;
/// Outline of the selected item.
const SELECTION_OUTLINE: f32 = 2.5;
/// Half the height of an exit marker at zoom 1.
const EXIT_MARK_SIZE: f32 = 5.0;
/// Exit markers do not get smaller than this factor.
const MIN_EXIT_MARK_SCALE: f32 = 0.6;

/// The part of the screen the diagram is painted on, with the transform
/// from diagram to screen coordinates and the colors of this frame.
pub(super) struct Canvas {
    pub(super) painter: Painter,
    /// Visible area on screen.
    pub(super) rect: Rect,
    pub(super) zoom: f32,
    /// Screen position of the diagram's origin.
    pub(super) origin: Pos2,
    pub(super) style: Palette,
    font: FontId,
    draw_text: bool,
}

impl Canvas {
    pub(super) fn new(painter: Painter, rect: Rect, zoom: f32, pan: Vec2, style: Palette) -> Self {
        let text_size = FONT_SIZE * zoom;
        Canvas {
            painter,
            rect,
            zoom,
            origin: rect.min + pan,
            style,
            font: FontId::monospace(text_size),
            draw_text: text_size >= MIN_TEXT_SIZE,
        }
    }

    pub(super) fn point_to_screen(&self, p: Pos2) -> Pos2 {
        self.origin + p.to_vec2() * self.zoom
    }

    pub(super) fn to_screen(&self, r: Rect) -> Rect {
        Rect::from_min_max(self.point_to_screen(r.min), self.point_to_screen(r.max))
    }

    pub(super) fn to_world(&self, p: Pos2) -> Pos2 {
        ((p - self.origin) / self.zoom).to_pos2()
    }

    /// Factor for line widths and marks that follow the zoom.
    fn line_scale(&self) -> f32 {
        self.zoom.max(MIN_LINE_SCALE)
    }

    fn radius(&self, radius: f32) -> CornerRadius {
        CornerRadius::same((radius * self.zoom).min(255.0) as u8)
    }

    /// Text with its top-left corner at the diagram position `pos`.
    fn text_at(&self, pos: Pos2, s: &str, color: Color32) {
        if self.draw_text {
            self.painter.text(
                self.point_to_screen(pos),
                Align2::LEFT_TOP,
                s,
                self.font.clone(),
                color,
            );
        }
    }

    fn text(&self, t: &Text, color: Color32) {
        self.text_at(t.rect.min, &t.text, color);
    }

    /// Filled triangle in `r` (screen coordinates) through `corners`, given
    /// in units of the triangle's size around the center of `r`.
    fn triangle(&self, r: Rect, corners: [Vec2; 3], color: Color32) {
        let c = r.center();
        let s = r.height().min(r.width()) * 0.3;
        let points = corners.iter().map(|&corner| c + corner * s).collect();
        self.painter
            .add(Shape::convex_polygon(points, color, Stroke::NONE));
    }

    /// Expand/collapse triangle: down when expanded, right when collapsed.
    fn toggle(&self, r: Rect, expanded: bool, color: Color32) {
        let corners = if expanded {
            [vec2(-1.0, -0.6), vec2(1.0, -0.6), vec2(0.0, 0.8)]
        } else {
            [vec2(-0.6, -1.0), vec2(-0.6, 1.0), vec2(0.8, 0.0)]
        };
        self.triangle(r, corners, color);
    }

    /// Arrow of the case selector.
    fn arrow(&self, r: Rect, right: bool, color: Color32) {
        let d = if right { 1.0 } else { -1.0 };
        let corners = [
            vec2(-0.6 * d, -1.0),
            vec2(-0.6 * d, 1.0),
            vec2(0.8 * d, 0.0),
        ];
        self.triangle(r, corners, color);
    }
}

/// Paints all visible items, parents before children, and outlines the
/// hovered and the selected one.
pub(super) fn items(
    canvas: &Canvas,
    scene: &Scene,
    hovered: Option<NodeId>,
    selected: Option<NodeId>,
) {
    let style = &canvas.style;
    for item in &scene.layout.items {
        let r = canvas.to_screen(item.rect);
        if !r.intersects(canvas.rect) {
            continue;
        }
        let node = scene.body.node(item.node);
        match (&item.kind, &node.kind) {
            (ItemKind::Structure(s), NodeKind::Structure { kind, .. }) => {
                structure(canvas, scene, item, s, kind);
            }
            (ItemKind::Frame { title }, NodeKind::Frame { .. }) => {
                frame(canvas, item, title.as_ref(), node.children.is_empty());
            }
            (ItemKind::Stmt { .. }, NodeKind::Stmt { kind }) => stmt(canvas, scene, item, kind),
            _ => {}
        }
        let radius = canvas.radius(ITEM_RADIUS);
        if hovered == Some(item.node) {
            let stroke = Stroke::new(HOVER_OUTLINE, style.hover);
            canvas
                .painter
                .rect_stroke(r, radius, stroke, StrokeKind::Outside);
        }
        if selected == Some(item.node) {
            let stroke = Stroke::new(SELECTION_OUTLINE, style.selected);
            canvas
                .painter
                .rect_stroke(r, radius, stroke, StrokeKind::Outside);
        }
    }
}

fn structure(canvas: &Canvas, scene: &Scene, item: &Item, s: &StructItem, kind: &StructKind) {
    let (painter, style) = (&canvas.painter, &canvas.style);
    let r = canvas.to_screen(item.rect);
    let radius = canvas.radius(ITEM_RADIUS);
    let color = style.structure(kind);
    painter.rect_filled(r, radius, style.frame_bg(item.depth));
    let header = canvas.to_screen(s.header);
    painter.rect_filled(header, radius, color.gamma_multiply(HEADER_TINT));
    let border = Stroke::new(structure_border(kind) * canvas.line_scale(), color);
    painter.rect_stroke(r, radius, border, StrokeKind::Inside);
    if let Some(&lane) = scene.analysis.lane_color.get(&item.node) {
        painter.rect_stroke(
            r.expand(2.0 * canvas.line_scale()),
            radius,
            Stroke::new(LANE_OUTLINE, lane),
            StrokeKind::Outside,
        );
    }
    for terminal in scene.register_terminals(item, s) {
        painter.rect_filled(canvas.to_screen(terminal), 1.0, style.note());
    }
    canvas.toggle(canvas.to_screen(s.toggle), s.expanded, style.text);
    if let Some(p) = &s.prefix {
        canvas.text(p, style.weak);
    }
    canvas.text(&s.title, style.text);
    if let Some(n) = &s.note {
        canvas.text(n, scene.note_color(item.node, style));
    }
    if let Some(i) = &s.info {
        canvas.text(i, style.info(&i.text));
    }
    if let Some(sel) = &s.selector {
        selector(canvas, sel);
    }
}

/// Controls of a case structure: arrows and position in one-at-a-time
/// mode, and the button that switches the mode.
fn selector(canvas: &Canvas, sel: &Selector) {
    let style = &canvas.style;
    if let (Some(prev), Some(pos), Some(next)) = (sel.prev, &sel.position, sel.next) {
        canvas.arrow(canvas.to_screen(prev), false, style.text);
        canvas.text(pos, style.text);
        canvas.arrow(canvas.to_screen(next), true, style.text);
    }
    let mode = canvas.to_screen(sel.mode.rect).expand(2.0 * canvas.zoom);
    canvas.painter.rect_stroke(
        mode,
        canvas.radius(ITEM_RADIUS),
        Stroke::new(OUTLINE, style.weak),
        StrokeKind::Outside,
    );
    canvas.text(&sel.mode, style.text);
}

fn frame(canvas: &Canvas, item: &Item, title: Option<&Text>, empty: bool) {
    let (painter, style) = (&canvas.painter, &canvas.style);
    let r = canvas.to_screen(item.rect);
    let radius = canvas.radius(ITEM_RADIUS);
    painter.rect_filled(r, radius, style.frame_bg(item.depth + 1));
    let line = Stroke::new(OUTLINE, style.frame_line);
    painter.rect_stroke(r, radius, line, StrokeKind::Inside);
    if let Some(t) = title {
        canvas.text(t, style.case_title);
    }
    if empty {
        let below_title = if title.is_some() {
            FRAME_TITLE_HEIGHT
        } else {
            0.0
        };
        let pos = item.rect.min + vec2(EMPTY_HINT_INSET, EMPTY_HINT_INSET + below_title);
        canvas.text_at(pos, "(empty)", style.weak);
    }
}

fn stmt(canvas: &Canvas, scene: &Scene, item: &Item, kind: &StmtKind) {
    let ItemKind::Stmt { text, badges, note } = &item.kind else {
        return;
    };
    let (painter, style) = (&canvas.painter, &canvas.style);
    let r = canvas.to_screen(item.rect);
    let radius = canvas.radius(ITEM_RADIUS);
    painter.rect_filled(r, radius, style.stmt(kind));
    let line = Stroke::new(OUTLINE, style.stmt_line(kind));
    painter.rect_stroke(r, radius, line, StrokeKind::Inside);
    canvas.text(text, style.text);
    if let Some(b) = badges {
        canvas.text(b, style.exit);
    }
    // A statement that starts a lane gets a bar and its note in the lane color.
    let lane = scene.analysis.lane_color.get(&item.node).copied();
    if let Some(lane) = lane {
        let bar = Rect::from_min_size(r.min, vec2(LANE_BAR * canvas.line_scale(), r.height()));
        painter.rect_filled(bar, 0.0, lane);
    }
    if let Some(n) = note {
        canvas.text(n, lane.unwrap_or(style.exit));
    }
}

/// Exit markers: where `return`, `break`, `continue` and `?` leave a
/// structure, on its right border at the height of the exit.
pub(super) fn exit_marks(canvas: &Canvas, scene: &Scene) {
    let s = EXIT_MARK_SIZE * canvas.zoom.max(MIN_EXIT_MARK_SCALE);
    for &tip in scene.exit_marks {
        let tip = canvas.point_to_screen(tip);
        let base = tip - vec2(2.0 * s, 0.0);
        canvas.painter.add(Shape::convex_polygon(
            vec![tip, base + vec2(0.0, -s), base + vec2(0.0, s)],
            canvas.style.exit,
            Stroke::NONE,
        ));
    }
}

/// Expression trees inside statements: the wires from operands to
/// operations, then the boxes; `hovered` gets an outline.
pub(super) fn expressions(canvas: &Canvas, scene: &Scene, hovered: Option<&ExprItem>) {
    let (painter, style) = (&canvas.painter, &canvas.style);
    for &(a, b) in &scene.layout.expr_wires {
        let (a, b) = (canvas.point_to_screen(a), canvas.point_to_screen(b));
        if canvas.rect.contains(a) || canvas.rect.contains(b) {
            expr_wire(canvas, a, b);
        }
    }
    let radius = canvas.radius(EXPR_RADIUS);
    for e in &scene.layout.exprs {
        let r = canvas.to_screen(e.rect);
        if !r.intersects(canvas.rect) {
            continue;
        }
        let node = scene.expr_node(e);
        let base = expr_color(node.kind, &node.text, style.dark, scene.body);
        painter.rect_filled(r, radius, style.expr_fill(base));
        painter.rect_stroke(r, radius, Stroke::new(OUTLINE, base), StrokeKind::Inside);
        if hovered.is_some_and(|h| h.stmt == e.stmt && h.index == e.index) {
            let stroke = Stroke::new(HOVER_OUTLINE, style.hover);
            painter.rect_stroke(r, radius, stroke, StrokeKind::Outside);
        }
        canvas.text(&e.text, style.text);
    }
}

/// Line from `a` to `b` (screen coordinates) with an arrow head at `b`.
fn expr_wire(canvas: &Canvas, a: Pos2, b: Pos2) {
    let weak = canvas.style.weak;
    canvas
        .painter
        .line_segment([a, b], Stroke::new(EXPR_WIRE, weak));
    let dir = (b - a).normalized();
    let side = vec2(-dir.y, dir.x) * 2.5 * canvas.line_scale();
    let tip = b;
    let back = tip - dir * 5.0 * canvas.line_scale();
    canvas.painter.add(Shape::convex_polygon(
        vec![tip, back + side, back - side],
        weak,
        Stroke::NONE,
    ));
}
