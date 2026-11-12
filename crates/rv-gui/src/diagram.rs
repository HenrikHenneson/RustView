//! Center panel: the structure diagram of the selected function.
//!
//! A frame goes through three steps (see [`DiagramView::ui`]): the model of
//! the function is brought up to date (analysis, layout and wires, each
//! cached by what it depends on), the input on the canvas is turned into
//! events and applied, and the scene is painted.

use std::collections::HashMap;

use egui::{FontId, Rect, Sense, Ui};
use rv_ir::{Body, FnId, NodeId, NodeKind, Project};
use rv_layout::{Metrics, Orientation};

use crate::wires::WireMode;

mod analysis;
mod fn_view;
mod input;
mod model;
mod paint;
mod palette;
mod scene;
mod svg;
mod toolbar;
mod tooltip;
mod wire_layer;

pub(crate) use svg::function_svg;

use fn_view::FnView;
use input::CanvasEvent;
use model::{Model, Shown};
use paint::Canvas;
use palette::{FONT_SIZE, MAX_LABEL_CHARS, Palette};
use scene::WireFilter;
use tooltip::Hover;

pub enum DiagramAction {
    None,
    Select(NodeId),
    Open(NodeId),
    /// A sub-expression was clicked: show exactly this part of the code.
    ShowSpan(rv_ir::Span),
    /// Status message (e.g. after an export).
    Message(String),
}

#[derive(Default)]
pub struct DiagramView {
    pub orientation: Orientation,
    views: HashMap<String, FnView>,
    /// Analysis, layout and wires of the shown function.
    model: Model,
    pub wire_mode: WireMode,
    /// New function views start with the expression level on.
    pub expressions_by_default: bool,
    /// "Export…" was clicked; done once the layout is there.
    export_requested: bool,
    pub selected: Option<NodeId>,
    /// Center this node on the next frame.
    scroll_to: Option<NodeId>,
    /// Canvas of the last frame, for "Fit".
    canvas: Option<Rect>,
}

impl DiagramView {
    pub fn with_expressions(on: bool) -> Self {
        DiagramView {
            expressions_by_default: on,
            ..DiagramView::default()
        }
    }

    /// Forget everything except the orientation (another project was opened).
    pub fn reset(&mut self) {
        *self = DiagramView {
            orientation: self.orientation,
            wire_mode: self.wire_mode,
            expressions_by_default: self.expressions_by_default,
            ..DiagramView::default()
        };
    }

    /// Forget the selection when another function is shown.
    pub fn function_changed(&mut self) {
        self.selected = None;
    }

    /// Select the innermost node covering `line` of `file`, expanding what
    /// hides it; returns it.
    pub fn reveal_line(
        &mut self,
        project: &Project,
        f: FnId,
        file: rv_ir::FileId,
        line: u32,
    ) -> Option<NodeId> {
        let func = project.function(f);
        let body = func.body.as_ref()?;
        let innermost = body
            .nodes
            .iter()
            .enumerate()
            .filter(|(_, n)| {
                n.span.file == file
                    && (n.span.start.line..=n.span.end.line).contains(&line)
                    && !matches!(n.kind, NodeKind::Frame { .. })
            })
            .min_by_key(|(i, n)| (n.span.end.line - n.span.start.line, std::cmp::Reverse(*i)))
            .map(|(i, _)| NodeId(i as u32));
        if let Some(node) = innermost {
            self.reveal_node(project, f, node);
        }
        innermost
    }

    /// Select `node`, expanding the structures around it and picking its case.
    pub fn reveal_node(&mut self, project: &Project, f: FnId, node: NodeId) {
        let func = project.function(f);
        let Some(body) = &func.body else { return };
        self.view_of(&func.qual_path, body).0.reveal(body, node);
        self.selected = Some(node);
        self.scroll_to = Some(node);
    }

    /// The view state of a function, created if needed. The flag says that
    /// an existing one was replaced because the body changed (its node ids
    /// are then meaningless).
    fn view_of(&mut self, qual_path: &str, body: &Body) -> (&mut FnView, bool) {
        let expressions = self.expressions_by_default;
        let view = self
            .views
            .entry(qual_path.to_string())
            .or_insert_with(|| FnView::new(body, expressions));
        let replaced = view.node_count != body.nodes.len();
        if replaced {
            *view = FnView::new(body, expressions);
        }
        (view, replaced)
    }

    pub fn ui(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        generation: u64,
        f: FnId,
    ) -> DiagramAction {
        let func = project.function(f);
        let Some(body) = &func.body else {
            ui.weak("This function has no body.");
            return DiagramAction::None;
        };
        if self.view_of(&func.qual_path, body).1 {
            self.selected = None;
        }
        self.toolbar(ui, body, &func.qual_path, &func.sig.text);
        ui.separator();

        // 1. The model of this frame: analysis, layout and wires.
        let view = self.views.get_mut(&func.qual_path).expect("inserted above");
        let shown = Shown {
            view,
            orientation: self.orientation,
            metrics: label_metrics(ui),
            wire_mode: self.wire_mode,
        };
        let scene = self.model.update(project, f, generation, body, &shown);

        // 2. Input on the canvas: scroll position first, then the clicks.
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.canvas = Some(rect);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
        input::pan_and_zoom(ui, &response, rect, view);
        if let Some(node) = self.scroll_to.take()
            && let Some(item) = scene.layout.item(node)
        {
            view.center_on(item.rect.center(), rect.size());
        }
        let style = Palette::new(ui.visuals());
        let canvas = Canvas::new(painter, rect, view.zoom, view.pan, style);
        let mut action = DiagramAction::None;
        for event in input::clicks(&response, &canvas, &scene) {
            action = apply_event(event, body, view, &mut self.selected);
        }

        // 3. Painting, bottom to top, and the tooltip.
        let pointer = response.hover_pos().map(|p| canvas.to_world(p));
        let mut hover = Hover {
            item: pointer.and_then(|p| scene.layout.hit(p)),
            expr: pointer.and_then(|p| scene.expr_at(p)),
            wire: None,
        };
        paint::items(&canvas, &scene, hover.item.map(|i| i.node), self.selected);
        paint::exit_marks(&canvas, &scene);
        paint::expressions(&canvas, &scene, hover.expr);
        let focus = self.selected.or(hover.item.map(|h| h.node));
        let filter = WireFilter::new(scene, self.wire_mode, focus);
        hover.wire = wire_layer::paint(&canvas, &scene, &filter, pointer);
        tooltip::show(response, project, &scene, hover);

        if let Some(msg) = self.export_if_requested(project, f) {
            action = DiagramAction::Message(msg);
        }
        action
    }

    /// Does the export asked for in the toolbar (the layout is there now);
    /// returns the status message unless the dialog was cancelled.
    fn export_if_requested(&mut self, project: &Project, f: FnId) -> Option<String> {
        if !std::mem::take(&mut self.export_requested) {
            return None;
        }
        let name = crate::export::file_name(&project.function(f).qual_path);
        crate::export::export(&name, || self.svg(project, f).unwrap_or_default())
    }

    /// The current function diagram (as laid out) as SVG in light colors.
    pub fn svg(&self, project: &Project, f: FnId) -> Option<String> {
        let body = project.function(f).body.as_ref()?;
        let scene = self.model.scene(body)?;
        Some(svg::diagram_svg(&scene, self.wire_mode, self.selected))
    }
}

/// Metrics of the label font at zoom 1.
fn label_metrics(ui: &Ui) -> Metrics {
    let font = FontId::monospace(FONT_SIZE);
    ui.fonts_mut(|fonts| Metrics {
        char_width: fonts.glyph_width(&font, 'x'),
        line_height: fonts.row_height(&font),
        max_label_chars: MAX_LABEL_CHARS,
    })
}

/// Applies a click to the view state and the selection; returns what the
/// app has to do about it.
fn apply_event(
    event: CanvasEvent,
    body: &Body,
    view: &mut FnView,
    selected: &mut Option<NodeId>,
) -> DiagramAction {
    match event {
        CanvasEvent::ShowExpr { stmt, span } => {
            *selected = Some(stmt);
            DiagramAction::ShowSpan(span)
        }
        CanvasEvent::Control(control, node) => {
            view.apply(control, node, body);
            DiagramAction::None
        }
        CanvasEvent::Select(node) => {
            *selected = Some(node);
            DiagramAction::Select(node)
        }
        CanvasEvent::Open(node) => DiagramAction::Open(node),
    }
}
