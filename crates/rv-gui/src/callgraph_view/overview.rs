//! The module graph on screen: boxes and arrows on a canvas that can be
//! moved and zoomed, with highlighting and tooltips.

use egui::{
    Align2, Color32, CornerRadius, FontId, Painter, Rect, Response, RichText, Sense, Shape, Stroke,
    StrokeKind, Ui, Vec2, vec2,
};
use rv_analysis::ModuleGraph;
use rv_ir::Project;

use super::CallGraphView;
use super::model::{LINE, LineKind, ModuleLayout, PAD, edge_width};
use crate::canvas::{PanZoom, Transform, hovered_line, paint_arrow_head};
use crate::style::{CALLS_IN, CALLS_OUT, THREAD};

/// Free space around the graph after "Fit".
const FIT_MARGIN: f32 = 20.0;
/// "Fit" never zooms further out or in than this.
const FIT_MIN_ZOOM: f32 = 0.15;
const FIT_MAX_ZOOM: f32 = 1.0;
/// Below this zoom the text in the boxes is left out.
const MIN_TEXT_ZOOM: f32 = 0.3;
const BOX_RADIUS: f32 = 5.0;
const NAME_FONT: f32 = 12.5;
const INFO_FONT: f32 = 11.5;
/// Calls listed in the tooltip of an edge.
const MAX_TOOLTIP_CALLS: usize = 12;

/// What is under the pointer.
#[derive(Clone, Copy, Default)]
struct Hover {
    node: Option<usize>,
    /// Only without a node under the pointer.
    edge: Option<usize>,
}

/// Where and how the graph is painted this frame.
struct Scene<'a> {
    painter: &'a Painter,
    visuals: &'a egui::Visuals,
    transform: Transform,
    /// The visible part of the canvas.
    clip: Rect,
    /// Node whose edges are highlighted: the selected or else the hovered one.
    focus: Option<usize>,
    hovered_edge: Option<usize>,
}

impl CallGraphView {
    /// The module graph of the current crate.
    pub(super) fn overview_ui(&mut self, ui: &mut Ui, project: &Project) {
        let (_, layout) = self.graph.as_ref().expect("computed by caller");
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        if std::mem::take(&mut self.fit) {
            self.view = fit(layout.size, rect.size());
        }
        self.view.handle_input(ui, rect, &response);
        let transform = self.view.transform(rect);
        let hover = hover(layout, &response, transform);

        let scene = Scene {
            painter: &painter,
            visuals: ui.visuals(),
            transform,
            clip: rect,
            focus: self.selected.or(hover.node),
            hovered_edge: hover.edge,
        };
        paint_edges(&scene, layout);
        paint_nodes(&scene, layout);

        if response.clicked() {
            self.selected = hover.node;
        }
        if response.double_clicked()
            && let Some(n) = hover.node
        {
            self.focus = Some(n);
        }
        if let Some(n) = hover.node {
            response.on_hover_ui_at_pointer(|ui| node_tooltip(ui, &layout.graph, n));
        } else if let Some(e) = hover.edge {
            response.on_hover_ui_at_pointer(|ui| edge_tooltip(ui, project, &layout.graph, e));
        }
    }
}

/// Position and zoom at which the whole graph is visible on the canvas.
fn fit(graph: Vec2, canvas: Vec2) -> PanZoom {
    let margin = Vec2::splat(FIT_MARGIN);
    let avail = canvas - 2.0 * margin;
    PanZoom {
        zoom: (avail.x / graph.x.max(1.0))
            .min(avail.y / graph.y.max(1.0))
            .clamp(FIT_MIN_ZOOM, FIT_MAX_ZOOM),
        pan: margin,
    }
}

fn hover(layout: &ModuleLayout, response: &Response, transform: Transform) -> Hover {
    let Some(pointer) = response.hover_pos().map(|p| transform.to_world(p)) else {
        return Hover::default();
    };
    let node = layout.node_at(pointer);
    let edge = match node {
        Some(_) => None,
        None => hovered_line(layout.curves(), pointer, transform.zoom),
    };
    Hover { node, edge }
}

fn paint_edges(scene: &Scene, layout: &ModuleLayout) {
    let weak = scene.visuals.weak_text_color();
    let zoom = scene.transform.zoom;
    for (i, e) in layout.graph.edges.iter().enumerate() {
        let (color, alpha) = match scene.focus {
            Some(f) if e.from == f => (CALLS_OUT, 1.0),
            Some(f) if e.to == f => (CALLS_IN, 1.0),
            Some(_) => (weak, 0.15),
            None => (weak, 0.7),
        };
        let hover_extra = if scene.hovered_edge == Some(i) {
            1.5
        } else {
            0.0
        };
        let width = edge_width(e) * zoom.max(0.5) + hover_extra;
        let stroke = Stroke::new(width, color.gamma_multiply(alpha));
        let pts = scene.transform.line_to_screen(layout.curve(i));
        scene.painter.add(Shape::line(pts.clone(), stroke));
        paint_arrow_head(scene.painter, &pts, stroke);
    }
}

fn paint_nodes(scene: &Scene, layout: &ModuleLayout) {
    let (visuals, zoom) = (scene.visuals, scene.transform.zoom);
    for (i, node) in layout.graph.nodes.iter().enumerate() {
        let r = scene.transform.rect_to_screen(layout.box_of(i));
        if !r.intersects(scene.clip) {
            continue;
        }
        let radius = CornerRadius::same((BOX_RADIUS * zoom).min(255.0) as u8);
        scene.painter.rect_filled(r, radius, visuals.panel_fill);
        let border = if node.spawns > 0 {
            Stroke::new(2.0, THREAD)
        } else {
            Stroke::new(1.0, visuals.widgets.noninteractive.fg_stroke.color)
        };
        scene
            .painter
            .rect_stroke(r, radius, border, StrokeKind::Inside);
        if scene.focus == Some(i) {
            scene.painter.rect_stroke(
                r.expand(2.0),
                radius,
                Stroke::new(2.0, visuals.selection.stroke.color),
                StrokeKind::Outside,
            );
        }
        if zoom >= MIN_TEXT_ZOOM {
            paint_labels(scene, layout, i);
        }
    }
}

/// The text lines in the box of a node.
fn paint_labels(scene: &Scene, layout: &ModuleLayout, node: usize) {
    let zoom = scene.transform.zoom;
    for (k, (text, kind)) in layout.labels(node).iter().enumerate() {
        let (font, color): (FontId, Color32) = match kind {
            LineKind::Name => (
                FontId::monospace(NAME_FONT * zoom),
                scene.visuals.strong_text_color(),
            ),
            LineKind::Threads => (FontId::proportional(INFO_FONT * zoom), THREAD),
            LineKind::Size => (
                FontId::proportional(INFO_FONT * zoom),
                scene.visuals.weak_text_color(),
            ),
        };
        let pos = layout.box_of(node).min + vec2(PAD, PAD + k as f32 * LINE);
        scene.painter.text(
            scene.transform.to_screen(pos),
            Align2::LEFT_TOP,
            text,
            font,
            color,
        );
    }
}

fn node_tooltip(ui: &mut Ui, g: &ModuleGraph, n: usize) {
    let node = &g.nodes[n];
    let out: usize = g
        .edges
        .iter()
        .filter(|e| e.from == n)
        .map(|e| e.calls.len())
        .sum();
    let inc: usize = g
        .edges
        .iter()
        .filter(|e| e.to == n)
        .map(|e| e.calls.len())
        .sum();
    ui.strong(&node.key);
    ui.label(format!(
        "{} functions · {} lines · {} modules",
        node.functions.len(),
        node.lines,
        node.modules.len()
    ));
    ui.label(RichText::new(format!("calls out: {out}")).color(CALLS_OUT));
    ui.label(RichText::new(format!("called from outside: {inc}")).color(CALLS_IN));
    ui.weak(format!("calls inside: {}", node.internal_calls));
    if node.spawns > 0 {
        ui.label(
            RichText::new(format!("starts threads/tasks at {} places", node.spawns)).color(THREAD),
        );
    }
    ui.weak("click: highlight · double click: show functions");
}

fn edge_tooltip(ui: &mut Ui, project: &Project, g: &ModuleGraph, edge: usize) {
    let e = &g.edges[edge];
    ui.label(format!(
        "{} → {}: {} calls",
        g.nodes[e.from].key,
        g.nodes[e.to].key,
        e.calls.len()
    ));
    for &(a, b) in e.calls.iter().take(MAX_TOOLTIP_CALLS) {
        ui.monospace(format!(
            "{} → {}",
            project.function(a).name,
            project.function(b).name
        ));
    }
    let more = e.calls.len().saturating_sub(MAX_TOOLTIP_CALLS);
    if more > 0 {
        ui.weak(format!("… {more} more"));
    }
}
