//! Program graph: threads as boxes in columns by spawn depth, arrows for
//! "starts", channels and shared state between them.

use egui::{
    Align2, CornerRadius, FontId, Painter, Rect, Response, Sense, Shape, Stroke, StrokeKind, Ui,
    Vec2, vec2,
};
use rv_analysis::Program;
use rv_ir::Project;

use crate::canvas::{PanZoom, Transform, hovered_line, paint_arrow_head};
use crate::style::{SHARED, THREAD};
use crate::threads::ThreadsAction;

mod layout;
mod svg;

pub use layout::{GraphLayout, layout};
pub use svg::graph_svg;

use layout::{Arrow, Block, BlockKind, LINE, LineStyle, PAD};

/// "Fit" never zooms further out or in than this.
const FIT_MIN_ZOOM: f32 = 0.2;
const FIT_MAX_ZOOM: f32 = 1.0;
/// Below this zoom the text in the boxes is left out.
const MIN_TEXT_ZOOM: f32 = 0.35;
/// Below this zoom the arrows have no labels.
const MIN_LABEL_ZOOM: f32 = 0.4;
const BOX_RADIUS: f32 = 6.0;
/// Functions listed in the tooltip of a thread.
const MAX_TOOLTIP_FNS: usize = 15;

#[derive(Default)]
pub struct ProgramView {
    view: PanZoom,
    /// The highlighted thread.
    pub selected: Option<usize>,
}

/// What is under the pointer.
#[derive(Clone, Copy, Default)]
struct Hover {
    block: Option<BlockKind>,
    /// Index of the arrow; only without a box under the pointer.
    arrow: Option<usize>,
}

/// Where and how the graph is painted this frame.
struct Scene<'a> {
    painter: &'a Painter,
    visuals: &'a egui::Visuals,
    transform: Transform,
    /// The visible part of the canvas.
    clip: Rect,
    /// The selected thread: everything not connected to it is dimmed.
    focus: Option<BlockKind>,
    hover: Hover,
}

impl ProgramView {
    pub fn fit(&mut self, g: &GraphLayout, canvas: Vec2) {
        self.view = PanZoom {
            zoom: (canvas.x / g.size.x)
                .min(canvas.y / g.size.y)
                .clamp(FIT_MIN_ZOOM, FIT_MAX_ZOOM),
            pan: Vec2::ZERO,
        };
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        program: &Program,
        g: &GraphLayout,
    ) -> ThreadsAction {
        let mut action = ThreadsAction::None;
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

        self.view.handle_input(ui, rect, &response);
        let transform = self.view.transform(rect);
        let hover = hover(g, &response, transform);

        if response.clicked() {
            self.selected = match hover.block {
                Some(BlockKind::Thread(t)) => Some(t),
                _ => None,
            };
        }
        if response.double_clicked()
            && let Some(BlockKind::Thread(t)) = hover.block
        {
            action = show_start(program, t);
        }

        let scene = Scene {
            painter: &painter,
            visuals: ui.visuals(),
            transform,
            clip: rect,
            focus: self.selected.map(BlockKind::Thread),
            hover,
        };
        // Arrows below the boxes.
        for (i, a) in g.arrows.iter().enumerate() {
            paint_arrow(&scene, a, i);
        }
        for b in &g.boxes {
            paint_block(&scene, b);
        }

        if let Some(kind) = hover.block {
            response.on_hover_ui_at_pointer(|ui| box_tooltip(ui, project, program, kind));
        } else if let Some(i) = hover.arrow {
            response.on_hover_ui_at_pointer(|ui| {
                ui.label(&g.arrows[i].tooltip);
            });
        }
        action
    }
}

fn hover(g: &GraphLayout, response: &Response, transform: Transform) -> Hover {
    let Some(pointer) = response.hover_pos().map(|p| transform.to_world(p)) else {
        return Hover::default();
    };
    let block = g
        .boxes
        .iter()
        .rev()
        .find(|b| b.rect.contains(pointer))
        .map(|b| b.kind);
    let arrow = match block {
        Some(_) => None,
        None => hovered_line(
            g.arrows.iter().map(|a| a.points.as_slice()),
            pointer,
            transform.zoom,
        ),
    };
    Hover { block, arrow }
}

/// Double click on a thread: show where it is started (the main thread: its first function).
fn show_start(program: &Program, thread: usize) -> ThreadsAction {
    let th = &program.threads[thread];
    match th.site {
        Some((f, node)) => ThreadsAction::ShowNode(f, node),
        None => th
            .runs
            .first()
            .map_or(ThreadsAction::None, |&f| ThreadsAction::ShowFn(f)),
    }
}

fn paint_arrow(scene: &Scene, a: &Arrow, index: usize) {
    let zoom = scene.transform.zoom;
    let related = scene.focus.is_none_or(|f| a.ends.0 == f || a.ends.1 == f);
    let alpha = if related { 1.0 } else { 0.15 };
    let hover_extra = if scene.hover.arrow == Some(index) {
        1.5
    } else {
        0.0
    };
    let width = (a.width + hover_extra) * zoom.max(0.5);
    let color = a.color.gamma_multiply(alpha);
    let pts = scene.transform.line_to_screen(&a.points);
    let stroke = Stroke::new(width, color);
    if a.dashed {
        scene
            .painter
            .extend(Shape::dashed_line(&pts, stroke, 3.0, 4.0));
    } else {
        scene.painter.add(Shape::line(pts.clone(), stroke));
    }
    if a.head {
        paint_arrow_head(scene.painter, &pts, stroke);
    }
    if let Some(label) = &a.label
        && zoom > MIN_LABEL_ZOOM
    {
        let mid = pts[pts.len() / 2];
        let font = FontId::monospace(11.0 * zoom);
        let galley = scene.painter.layout_no_wrap(label.clone(), font, color);
        let r = Align2::CENTER_CENTER
            .anchor_size(mid, galley.size())
            .expand(2.0);
        let background = scene.visuals.extreme_bg_color.gamma_multiply(0.9);
        scene.painter.rect_filled(r, 3.0, background);
        scene.painter.galley(r.min + vec2(2.0, 2.0), galley, color);
    }
}

fn paint_block(scene: &Scene, b: &Block) {
    let (visuals, zoom) = (scene.visuals, scene.transform.zoom);
    let r = scene.transform.rect_to_screen(b.rect);
    if !r.intersects(scene.clip) {
        return;
    }
    let border = match b.kind {
        BlockKind::Thread(0) => Stroke::new(1.5, visuals.text_color()),
        BlockKind::Thread(_) => Stroke::new(2.0, THREAD),
        BlockKind::Resource(_) => Stroke::new(1.5, SHARED),
    };
    let radius = CornerRadius::same((BOX_RADIUS * zoom).min(255.0) as u8);
    scene.painter.rect_filled(r, radius, visuals.panel_fill);
    scene
        .painter
        .rect_stroke(r, radius, border, StrokeKind::Inside);
    if scene.focus == Some(b.kind) {
        scene.painter.rect_stroke(
            r.expand(2.0),
            radius,
            Stroke::new(2.5, visuals.selection.stroke.color),
            StrokeKind::Outside,
        );
    } else if scene.hover.block == Some(b.kind) {
        scene.painter.rect_stroke(
            r.expand(1.0),
            radius,
            Stroke::new(1.5, visuals.widgets.hovered.fg_stroke.color),
            StrokeKind::Outside,
        );
    }
    if zoom < MIN_TEXT_ZOOM {
        return;
    }
    for (i, (text, style)) in b.lines.iter().enumerate() {
        let (font, color) = match style {
            LineStyle::Title => (
                FontId::proportional(13.0 * zoom),
                visuals.strong_text_color(),
            ),
            LineStyle::Normal => (FontId::monospace(12.0 * zoom), visuals.text_color()),
            LineStyle::Weak => (FontId::proportional(12.0 * zoom), visuals.weak_text_color()),
        };
        let pos = scene
            .transform
            .to_screen(b.rect.min + vec2(PAD, PAD + i as f32 * LINE));
        scene.painter.text(pos, Align2::LEFT_TOP, text, font, color);
    }
}

fn box_tooltip(ui: &mut Ui, project: &Project, program: &Program, kind: BlockKind) {
    match kind {
        BlockKind::Thread(t) => {
            let th = &program.threads[t];
            ui.strong(th.title());
            if !th.spawned_by.is_empty() {
                let by: Vec<String> = th
                    .spawned_by
                    .iter()
                    .map(|&p| program.threads[p].title())
                    .collect();
                ui.weak(format!("started by {}", by.join(", ")));
            }
            ui.weak(format!("runs {} functions, e.g.:", th.runs.len()));
            for &f in th.runs.iter().take(MAX_TOOLTIP_FNS) {
                ui.monospace(&project.function(f).qual_path);
            }
            ui.weak("click: highlight · double click: show where it starts");
        }
        BlockKind::Resource(r) => {
            let res = &program.resources[r];
            ui.strong(format!("{} ({})", res.name, res.kind));
            ui.weak("shared by:");
            for &u in &res.users {
                ui.label(program.threads[u].title());
            }
        }
    }
}
