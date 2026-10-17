//! Center panel: the structure diagram of the selected function.

use std::collections::{HashMap, HashSet};

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};
use rv_analysis::{Concurrency, DataFlow, EdgeKind, End, SpawnKind};
use rv_ir::{Body, FnId, NodeId, NodeKind, Project, StmtKind, StructKind};
use rv_layout::{ItemKind, Layout, Metrics, Orientation, ViewState};

use crate::wires::{self, WireKind, WireMode, WireSpec, WireStyle};

/// Font size of labels at zoom 1.
const FONT_SIZE: f32 = 12.0;
/// Items shown initially; larger functions start partly collapsed.
const BUDGET: usize = 150;
const MAX_LABEL_CHARS: usize = 60;
const MIN_ZOOM: f32 = 0.05;
const MAX_ZOOM: f32 = 4.0;
/// Below this font size (in points) labels are not drawn.
const MIN_TEXT_SIZE: f32 = 4.0;
const MARGIN: f32 = 16.0;

pub enum DiagramAction {
    None,
    Select(NodeId),
    Open(NodeId),
    /// A sub-expression was clicked: show exactly this part of the code.
    ShowSpan(rv_ir::Span),
    /// Status message (e.g. after an export).
    Message(String),
}

/// Per-function view state, kept while switching between functions.
struct FnView {
    state: ViewState,
    defaults: HashSet<NodeId>,
    /// To detect that a reload changed the body (node ids are then meaningless).
    node_count: usize,
    zoom: f32,
    pan: Vec2,
    version: u64,
}

#[derive(Clone, PartialEq)]
struct LayoutKey {
    func: FnId,
    generation: u64,
    orientation: Orientation,
    version: u64,
    char_width_bits: u32,
}

#[derive(Default)]
pub struct DiagramView {
    pub orientation: Orientation,
    views: HashMap<String, FnView>,
    cached: Option<(LayoutKey, Layout)>,
    /// Analyses of the shown function, per project generation.
    analysis: Option<((FnId, u64), Analysis)>,
    pub wire_mode: WireMode,
    /// New function views start with the expression level on.
    pub expressions_by_default: bool,
    /// "Export…" was clicked; done once the layout is there.
    export_requested: bool,
    /// Wire geometry for the current layout.
    wire_cache: Option<(LayoutKey, Vec<wires::Wire>)>,
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

    /// Select the innermost node covering `line` of `file`, expanding what hides it.
    pub fn reveal_line(&mut self, project: &Project, f: FnId, file: rv_ir::FileId, line: u32) {
        let func = project.function(f);
        let Some(body) = &func.body else { return };
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
    }

    /// Select `node`, expanding the structures around it and picking its case.
    pub fn reveal_node(&mut self, project: &Project, f: FnId, node: NodeId) {
        let func = project.function(f);
        let Some(body) = &func.body else { return };
        let view = self
            .views
            .entry(func.qual_path.clone())
            .or_insert_with(|| new_view(body, self.expressions_by_default));
        if view.node_count != body.nodes.len() {
            *view = new_view(body, self.expressions_by_default);
        }
        // Expand all structures on the way and pick the right case.
        let mut child = node;
        while let Some(parent) = body.node(child).parent {
            if matches!(body.node(parent).kind, NodeKind::Structure { .. }) {
                view.state.expanded.insert(parent, true);
                let cases = rv_layout::case_frames(body, parent);
                if let Some(i) = cases.iter().position(|&c| c == child) {
                    view.state.case_index.insert(parent, i);
                }
            }
            child = parent;
        }
        view.version += 1;
        self.selected = Some(node);
        self.scroll_to = Some(node);
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

        let view = self
            .views
            .entry(func.qual_path.clone())
            .or_insert_with(|| new_view(body, self.expressions_by_default));
        if view.node_count != body.nodes.len() {
            *view = new_view(body, self.expressions_by_default);
            self.selected = None;
        }

        let mut action = DiagramAction::None;
        self.toolbar(ui, body, &func.qual_path, &func.sig.text);
        ui.separator();

        let font = FontId::monospace(FONT_SIZE);
        let metrics = ui.fonts_mut(|fonts| Metrics {
            char_width: fonts.glyph_width(&font, 'x'),
            line_height: fonts.row_height(&font),
            max_label_chars: MAX_LABEL_CHARS,
        });
        let view = self.views.get_mut(&func.qual_path).expect("inserted above");
        let key = LayoutKey {
            func: f,
            generation,
            orientation: self.orientation,
            version: view.version,
            char_width_bits: metrics.char_width.to_bits(),
        };
        if self
            .analysis
            .as_ref()
            .is_none_or(|(k, _)| *k != (f, generation))
        {
            self.analysis = Some(((f, generation), Analysis::new(body, project)));
        }
        let analysis = &self.analysis.as_ref().expect("computed above").1;
        if self.cached.as_ref().is_none_or(|(k, _)| *k != key) {
            // Mark structures that show expressions.
            let mut notes = analysis.notes.clone();
            for &s in &view.state.expressions {
                notes
                    .entry(s)
                    .and_modify(|n| n.push_str(" · ƒx"))
                    .or_insert_with(|| "ƒx".into());
            }
            let l = rv_layout::layout(
                body,
                &view.state,
                &view.defaults,
                self.orientation,
                metrics,
                &notes,
            );
            self.cached = Some((key, l));
        }
        let layout = &self.cached.as_ref().expect("computed above").1;

        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        self.canvas = Some(rect);
        let painter = ui.painter_at(rect);
        painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);

        // Pan and zoom.
        if response.dragged() {
            view.pan += response.drag_delta();
        }
        if response.hovered() {
            let (zoom_delta, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
            if zoom_delta != 1.0
                && let Some(p) = response.hover_pos()
            {
                let world = (p - rect.min - view.pan) / view.zoom;
                view.zoom = (view.zoom * zoom_delta).clamp(MIN_ZOOM, MAX_ZOOM);
                view.pan = p - rect.min - world * view.zoom;
            } else {
                view.pan += scroll;
            }
        }
        if let Some(node) = self.scroll_to.take()
            && let Some(item) = layout.item(node)
        {
            let center = item.rect.center().to_vec2() * view.zoom;
            view.pan = rect.size() / 2.0 - center;
            view.pan.y = view.pan.y.min(MARGIN);
            view.pan.x = view.pan.x.min(MARGIN);
        }
        let (zoom, origin) = (view.zoom, rect.min + view.pan);
        let to_screen = |r: Rect| {
            Rect::from_min_max(
                origin + r.min.to_vec2() * zoom,
                origin + r.max.to_vec2() * zoom,
            )
        };
        let to_world = |p: Pos2| ((p - origin) / zoom).to_pos2();

        // Interaction.
        let hovered = response.hover_pos().and_then(|p| layout.hit(to_world(p)));
        if response.clicked()
            && let Some(p) = response.interact_pointer_pos()
        {
            let world = to_world(p);
            if let Some(e) = layout.exprs.iter().find(|e| e.rect.contains(world)) {
                let tree = body
                    .node(e.stmt)
                    .expr
                    .as_ref()
                    .expect("expression items come from trees");
                self.selected = Some(e.stmt);
                action = DiagramAction::ShowSpan(tree.nodes[e.index].span.clone());
            } else if let Some(item) = layout.hit(world) {
                match control_at(item, world) {
                    Some(control) => {
                        apply_control(control, item.node, body, &mut view.state, &view.defaults);
                        view.version += 1;
                    }
                    None => {
                        self.selected = Some(item.node);
                        action = DiagramAction::Select(item.node);
                    }
                }
            }
        }
        if response.double_clicked()
            && let Some(item) = response
                .interact_pointer_pos()
                .and_then(|p| layout.hit(to_world(p)))
            && control_at(
                item,
                to_world(response.interact_pointer_pos().unwrap_or_default()),
            )
            .is_none()
        {
            action = DiagramAction::Open(item.node);
        }

        // Painting.
        let style = Palette::new(ui.visuals());
        let text_size = FONT_SIZE * zoom;
        let draw_text = text_size >= MIN_TEXT_SIZE;
        let font = FontId::monospace(text_size);
        let text = |r: Rect, s: &str, color: Color32| {
            if draw_text {
                painter.text(
                    origin + r.min.to_vec2() * zoom,
                    egui::Align2::LEFT_TOP,
                    s,
                    font.clone(),
                    color,
                );
            }
        };
        for item in &layout.items {
            let r = to_screen(item.rect);
            if !r.intersects(rect) {
                continue;
            }
            let node = body.node(item.node);
            let radius = CornerRadius::same((4.0 * zoom).min(255.0) as u8);
            match (&item.kind, &node.kind) {
                (ItemKind::Structure(s), NodeKind::Structure { kind, .. }) => {
                    let color = style.structure(kind);
                    painter.rect_filled(r, radius, style.frame_bg(item.depth));
                    let header = to_screen(s.header);
                    painter.rect_filled(header, radius, color.gamma_multiply(0.35));
                    let width = if kind.is_loop() { 3.0 } else { 1.5 };
                    painter.rect_stroke(
                        r,
                        radius,
                        Stroke::new(width * zoom.max(0.5), color),
                        StrokeKind::Inside,
                    );
                    if let Some(&lane) = analysis.lane_color.get(&item.node) {
                        painter.rect_stroke(
                            r.expand(2.0 * zoom.max(0.5)),
                            radius,
                            Stroke::new(3.0, lane),
                            StrokeKind::Outside,
                        );
                    }
                    if s.expanded
                        && let Some(regs) = analysis.registers.get(&item.node)
                    {
                        // Shift register terminals on both borders, below the header.
                        let c = wires::color("", WireKind::LoopBack, style.dark);
                        for k in 0..regs.len() {
                            let y = s.header.max.y + 8.0 + k as f32 * 14.0;
                            for x in [item.rect.min.x, item.rect.max.x] {
                                let sq = to_screen(Rect::from_center_size(
                                    egui::pos2(x, y),
                                    vec2(9.0, 9.0),
                                ));
                                painter.rect_filled(sq, 1.0, c);
                            }
                        }
                    }
                    draw_triangle(&painter, to_screen(s.toggle), s.expanded, style.text);
                    if let Some(p) = &s.prefix {
                        text(p.rect, &p.text, style.weak);
                    }
                    text(s.title.rect, &s.title.text, style.text);
                    if let Some(n) = &s.note {
                        // Lane color, blue for shift registers / expressions.
                        let c = analysis
                            .lane_color
                            .get(&item.node)
                            .copied()
                            .unwrap_or_else(|| wires::color("", WireKind::LoopBack, style.dark));
                        text(n.rect, &n.text, c);
                    }
                    if let Some(i) = &s.info {
                        let c = if i.text.contains("exits") {
                            style.exit
                        } else {
                            style.weak
                        };
                        text(i.rect, &i.text, c);
                    }
                    if let Some(sel) = &s.selector {
                        if let (Some(prev), Some(pos), Some(next)) =
                            (sel.prev, &sel.position, sel.next)
                        {
                            draw_arrow(&painter, to_screen(prev), false, style.text);
                            text(pos.rect, &pos.text, style.text);
                            draw_arrow(&painter, to_screen(next), true, style.text);
                        }
                        let mode = to_screen(sel.mode.rect).expand(2.0 * zoom);
                        painter.rect_stroke(
                            mode,
                            radius,
                            Stroke::new(1.0, style.weak),
                            StrokeKind::Outside,
                        );
                        text(sel.mode.rect, &sel.mode.text, style.text);
                    }
                }
                (ItemKind::Frame { title }, NodeKind::Frame { .. }) => {
                    painter.rect_filled(r, radius, style.frame_bg(item.depth + 1));
                    painter.rect_stroke(
                        r,
                        radius,
                        Stroke::new(1.0, style.frame_line),
                        StrokeKind::Inside,
                    );
                    if let Some(t) = title {
                        text(t.rect, &t.text, style.case_title);
                    }
                    if node.children.is_empty() {
                        let pos = item.rect.min
                            + vec2(8.0, 8.0 + if title.is_some() { 22.0 } else { 0.0 });
                        text(Rect::from_min_size(pos, Vec2::ZERO), "(empty)", style.weak);
                    }
                }
                (
                    ItemKind::Stmt {
                        text: t,
                        badges,
                        note,
                    },
                    NodeKind::Stmt { kind },
                ) => {
                    painter.rect_filled(r, radius, style.stmt(kind));
                    painter.rect_stroke(
                        r,
                        radius,
                        Stroke::new(1.0, style.stmt_line(kind)),
                        StrokeKind::Inside,
                    );
                    text(t.rect, &t.text, style.text);
                    if let Some(b) = badges {
                        text(b.rect, &b.text, style.exit);
                    }
                    if let Some(&lane) = analysis.lane_color.get(&item.node) {
                        let bar = Rect::from_min_size(r.min, vec2(4.0 * zoom.max(0.5), r.height()));
                        painter.rect_filled(bar, 0.0, lane);
                        if let Some(n) = note {
                            text(n.rect, &n.text, lane);
                        }
                    } else if let Some(n) = note {
                        text(n.rect, &n.text, style.exit);
                    }
                }
                _ => {}
            }
            if hovered.is_some_and(|h| h.node == item.node) {
                painter.rect_stroke(
                    r,
                    radius,
                    Stroke::new(1.5, style.hover),
                    StrokeKind::Outside,
                );
            }
            if self.selected == Some(item.node) {
                painter.rect_stroke(
                    r,
                    radius,
                    Stroke::new(2.5, style.selected),
                    StrokeKind::Outside,
                );
            }
        }

        // Exit markers: where `return`, `break`, `continue` and `?` leave a
        // structure, on its right border at the height of the exit.
        let mut exit_marks: HashSet<(NodeId, i32)> = HashSet::new();
        for e in &body.exits {
            let (Some(target), Some(from)) = (e.target, layout.item(e.from)) else {
                continue;
            };
            let y = from.rect.min.y + (from.rect.height() / 2.0).min(11.0);
            let mut cur = body.node(e.from).parent;
            while let Some(n) = cur {
                if n == target {
                    break;
                }
                if let Some(item) = layout.item(n)
                    && matches!(item.kind, ItemKind::Structure(_))
                    && exit_marks.insert((n, y as i32))
                {
                    let tip = origin + vec2(item.rect.max.x + 7.0, y) * zoom;
                    let s = 5.0 * zoom.max(0.6);
                    let base = tip - vec2(2.0 * s, 0.0);
                    painter.add(egui::Shape::convex_polygon(
                        vec![tip, base + vec2(0.0, -s), base + vec2(0.0, s)],
                        style.exit,
                        Stroke::NONE,
                    ));
                }
                cur = body.node(n).parent;
            }
        }

        // Expression trees inside statements.
        let hovered_expr = response
            .hover_pos()
            .map(to_world)
            .and_then(|p| layout.exprs.iter().find(|e| e.rect.contains(p)));
        for &(a, b) in &layout.expr_wires {
            let (a, b) = (origin + a.to_vec2() * zoom, origin + b.to_vec2() * zoom);
            if rect.contains(a) || rect.contains(b) {
                painter.line_segment([a, b], Stroke::new(1.2, style.weak));
                let dir = (b - a).normalized();
                let side = vec2(-dir.y, dir.x) * 2.5 * zoom.max(0.5);
                let tip = b;
                let back = tip - dir * 5.0 * zoom.max(0.5);
                painter.add(egui::Shape::convex_polygon(
                    vec![tip, back + side, back - side],
                    style.weak,
                    Stroke::NONE,
                ));
            }
        }
        for e in &layout.exprs {
            let r = to_screen(e.rect);
            if !r.intersects(rect) {
                continue;
            }
            let tree = body
                .node(e.stmt)
                .expr
                .as_ref()
                .expect("expression items come from trees");
            let node = &tree.nodes[e.index];
            let base = expr_color(node.kind, &node.text, ui.visuals().dark_mode, body);
            let radius = CornerRadius::same((3.0 * zoom).min(255.0) as u8);
            painter.rect_filled(
                r,
                radius,
                lerp(style.bg, base, if style.dark { 0.35 } else { 0.22 }),
            );
            painter.rect_stroke(r, radius, Stroke::new(1.0, base), StrokeKind::Inside);
            if hovered_expr.is_some_and(|h| h.stmt == e.stmt && h.index == e.index) {
                painter.rect_stroke(
                    r,
                    radius,
                    Stroke::new(1.5, style.hover),
                    StrokeKind::Outside,
                );
            }
            text(e.text.rect, &e.text.text, style.text);
        }

        // Wires on top of the boxes.
        let mut hovered_wire = None;
        let empty = Vec::new();
        let mut wire_list = &empty;
        if self.wire_mode != WireMode::Off || !analysis.specs.is_empty() {
            let key = self.cached.as_ref().expect("computed above").0.clone();
            if self.wire_cache.as_ref().is_none_or(|(k, _)| *k != key) {
                let g = wires::geometry(body, &analysis.specs, layout, self.orientation);
                self.wire_cache = Some((key, g));
            }
            wire_list = &self.wire_cache.as_ref().expect("computed above").1;
            let specs = &analysis.specs;
            let focus = self.selected.or(hovered.map(|h| h.node));
            let data_wires = wire_list
                .iter()
                .filter(|w| !w.kind.is_concurrency())
                .count();
            let mode = match self.wire_mode {
                WireMode::Auto if data_wires <= wires::AUTO_LIMIT => WireMode::All,
                WireMode::Auto => WireMode::Selected,
                other => other,
            };
            let shown = |w: &wires::Wire| {
                w.kind.is_concurrency()
                    || match mode {
                        WireMode::All => true,
                        WireMode::Off => false,
                        _ => focus.is_some_and(|n| w.touches(n, specs)),
                    }
            };
            if mode == WireMode::Selected && focus.is_none() && data_wires > 0 {
                let hint = format!(
                    "{data_wires} data wires · select or hover a node to see its wires (wires: all shows everything)"
                );
                let galley = painter.layout_no_wrap(hint, FontId::proportional(12.0), style.weak);
                let pos = rect.left_bottom() + vec2(8.0, -8.0 - galley.size().y);
                let bg = Rect::from_min_size(pos, galley.size()).expand(4.0);
                painter.rect_filled(bg, 4.0, ui.visuals().extreme_bg_color);
                painter.galley(pos, galley, style.weak);
            }
            hovered_wire = response
                .hover_pos()
                .and_then(|p| wires::hit(wire_list, shown, to_world(p), 5.0 / zoom));
            let wstyle = WireStyle {
                dark: ui.visuals().dark_mode,
                zoom,
                origin,
            };
            for (i, w) in wire_list.iter().enumerate().filter(|(_, w)| shown(w)) {
                let related = w.kind.is_concurrency() || focus.is_none_or(|n| w.touches(n, specs));
                let alpha = if related || mode == WireMode::Selected {
                    1.0
                } else {
                    0.18
                };
                wires::paint(&painter, w, &wstyle, alpha, hovered_wire == Some(i), rect);
            }
        }

        if let Some(i) = hovered_wire {
            let w = &wire_list[i];
            response.on_hover_ui_at_pointer(|ui| wire_tooltip(ui, body, analysis, w));
        } else if let Some(e) = hovered_expr {
            let tree = body
                .node(e.stmt)
                .expr
                .as_ref()
                .expect("expression items come from trees");
            let node = &tree.nodes[e.index];
            let snippet = project.snippet(&node.span).to_string();
            let kind = format!("{:?}", node.kind);
            response.on_hover_ui_at_pointer(|ui| {
                ui.weak(kind);
                ui.label(egui::RichText::new(snippet).monospace());
                ui.weak("click: show in code");
            });
        } else if let Some(item) = hovered {
            response.on_hover_ui_at_pointer(|ui| tooltip(ui, project, body, item.node));
        }
        if std::mem::take(&mut self.export_requested) {
            let name = crate::export::file_name(&func.qual_path);
            if let Some(msg) =
                crate::export::export(&name, || self.svg(project, f).unwrap_or_default())
            {
                action = DiagramAction::Message(msg);
            }
        }
        action
    }

    fn toolbar(&mut self, ui: &mut Ui, body: &Body, qual_path: &str, signature: &str) {
        let view = self.views.get_mut(qual_path).expect("inserted by caller");
        ui.horizontal_wrapped(|ui| {
            ui.strong(qual_path).on_hover_text(signature);
            ui.separator();
            ui.selectable_value(&mut self.orientation, Orientation::Vertical, "↓ top-down")
                .on_hover_text("Statements top to bottom, cases side by side");
            ui.selectable_value(
                &mut self.orientation,
                Orientation::Horizontal,
                "→ left-right",
            )
            .on_hover_text("Statements left to right, cases stacked");
            ui.separator();
            ui.label("wires:");
            ui.selectable_value(&mut self.wire_mode, WireMode::Auto, "auto")
                .on_hover_text(format!(
                    "All wires up to {}, otherwise only those of the selected node",
                    wires::AUTO_LIMIT
                ));
            ui.selectable_value(&mut self.wire_mode, WireMode::All, "all")
                .on_hover_text("All data wires; those of the selected node stand out");
            ui.selectable_value(&mut self.wire_mode, WireMode::Selected, "selected")
                .on_hover_text("Only wires of the selected or hovered node");
            ui.selectable_value(&mut self.wire_mode, WireMode::Off, "off");
            ui.separator();
            // Expression level for the selected structure (or the whole function).
            let target = self
                .selected
                .and_then(|n| {
                    let mut cur = Some(n);
                    while let Some(x) = cur {
                        if matches!(body.node(x).kind, NodeKind::Structure { .. }) {
                            return Some(x);
                        }
                        cur = body.node(x).parent;
                    }
                    None
                })
                .unwrap_or(body.root);
            let on = view.state.expressions.contains(&target);
            let what = if target == body.root { "the whole function".to_string() } else { format!("`{}`", body.node(target).label) };
            if ui
                .selectable_label(on, "ƒx Expressions")
                .on_hover_text(format!(
                    "Show statements as data flow of their expressions in {what} (select a structure to limit it)"
                ))
                .clicked()
            {
                if on {
                    view.state.expressions.remove(&target);
                } else {
                    view.state.expressions.insert(target);
                }
                view.version += 1;
            }
            ui.separator();
            if ui.button("Expand all").clicked() {
                for (i, n) in body.nodes.iter().enumerate() {
                    if matches!(n.kind, NodeKind::Structure { .. }) {
                        view.state.expanded.insert(NodeId(i as u32), true);
                    }
                }
                view.version += 1;
            }
            if ui.button("Collapse all").clicked() {
                for (i, n) in body.nodes.iter().enumerate() {
                    if matches!(n.kind, NodeKind::Structure { .. }) && NodeId(i as u32) != body.root
                    {
                        view.state.expanded.insert(NodeId(i as u32), false);
                    }
                }
                view.version += 1;
            }
            if ui
                .button("Auto")
                .on_hover_text(format!(
                    "Expand from the outside in until about {BUDGET} items are visible"
                ))
                .clicked()
            {
                view.state.expanded.clear();
                view.defaults = rv_layout::default_expanded(body, &view.state, BUDGET);
                view.version += 1;
            }
            ui.separator();
            if ui.button("−").clicked() {
                view.zoom = (view.zoom / 1.25).max(MIN_ZOOM);
            }
            if ui
                .button(format!("{:.0}%", view.zoom * 100.0))
                .on_hover_text("Reset zoom")
                .clicked()
            {
                view.zoom = 1.0;
                view.pan = Vec2::splat(MARGIN);
            }
            if ui.button("+").clicked() {
                view.zoom = (view.zoom * 1.25).min(MAX_ZOOM);
            }
            if ui
                .button("Fit")
                .on_hover_text("Show the whole diagram (never above 100%)")
                .clicked()
                && let Some((_, layout)) = &self.cached
                && let Some(canvas) = self.canvas.filter(|c| c.is_positive())
            {
                let target = canvas.size() - Vec2::splat(2.0 * MARGIN);
                view.zoom = (target.x / layout.size.x)
                    .min(target.y / layout.size.y)
                    .clamp(MIN_ZOOM, 1.0);
                view.pan = Vec2::splat(MARGIN);
            }
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

fn new_view(body: &Body, expressions: bool) -> FnView {
    let mut state = ViewState::default();
    if expressions {
        state.expressions.insert(body.root);
    }
    FnView {
        defaults: rv_layout::default_expanded(body, &state, BUDGET),
        state,
        node_count: body.nodes.len(),
        zoom: 1.0,
        pan: Vec2::splat(MARGIN),
        version: 0,
    }
}

#[derive(Clone, Copy)]
enum Control {
    Toggle,
    PrevCase,
    NextCase,
    CaseMode,
}

fn control_at(item: &rv_layout::Item, world: Pos2) -> Option<Control> {
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

fn apply_control(
    control: Control,
    node: NodeId,
    body: &Body,
    state: &mut ViewState,
    defaults: &HashSet<NodeId>,
) {
    let count = rv_layout::case_frames(body, node).len().max(1);
    let index = state.case_index(body, node);
    match control {
        Control::Toggle => {
            let shown = state.is_expanded(defaults, node);
            state.expanded.insert(node, !shown);
        }
        Control::PrevCase => {
            state.case_index.insert(node, (index + count - 1) % count);
        }
        Control::NextCase => {
            state.case_index.insert(node, (index + 1) % count);
        }
        Control::CaseMode => {
            let all = state.shows_all_cases(body, node);
            state.all_cases.insert(node, !all);
        }
    }
}

fn draw_triangle(painter: &egui::Painter, r: Rect, expanded: bool, color: Color32) {
    let c = r.center();
    let s = r.height().min(r.width()) * 0.3;
    let points = if expanded {
        vec![
            c + vec2(-s, -s * 0.6),
            c + vec2(s, -s * 0.6),
            c + vec2(0.0, s * 0.8),
        ]
    } else {
        vec![
            c + vec2(-s * 0.6, -s),
            c + vec2(-s * 0.6, s),
            c + vec2(s * 0.8, 0.0),
        ]
    };
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

fn draw_arrow(painter: &egui::Painter, r: Rect, right: bool, color: Color32) {
    let c = r.center();
    let s = r.height().min(r.width()) * 0.3;
    let d = if right { 1.0 } else { -1.0 };
    let points = vec![
        c + vec2(-s * 0.6 * d, -s),
        c + vec2(-s * 0.6 * d, s),
        c + vec2(s * 0.8 * d, 0.0),
    ];
    painter.add(egui::Shape::convex_polygon(points, color, Stroke::NONE));
}

fn wire_tooltip(ui: &mut Ui, body: &Body, a: &Analysis, w: &wires::Wire) {
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
    ui.label(egui::RichText::new(&w.name).monospace().strong());
    ui.weak(what);
    if !certain {
        ui.weak("guessed from the tokens of an unexpanded macro");
    }
    for (dir, id) in [("from", spec.from), ("to", spec.to)] {
        let n = body.node(id);
        let label: String = n.label.chars().take(70).collect();
        ui.monospace(format!("{dir} line {}: {label}", n.span.start.line));
    }
}

/// Analyses of one function and what the diagram derives from them.
struct Analysis {
    flow: DataFlow,
    conc: Concurrency,
    specs: Vec<WireSpec>,
    notes: rv_layout::Notes,
    /// Lane structures and statements that start an inline lane.
    lane_color: HashMap<NodeId, Color32>,
    /// Shift registers per loop.
    registers: HashMap<NodeId, Vec<String>>,
}

/// Colors of parallel lanes.
const LANE_COLORS: [Color32; 6] = [
    Color32::from_rgb(0, 160, 120),
    Color32::from_rgb(210, 100, 0),
    Color32::from_rgb(40, 110, 220),
    Color32::from_rgb(200, 50, 120),
    Color32::from_rgb(120, 140, 0),
    Color32::from_rgb(110, 80, 200),
];

impl Analysis {
    fn new(body: &Body, project: &Project) -> Self {
        let flow = rv_analysis::data_flow(body);
        let conc = rv_analysis::concurrency(body, &flow, &project.type_aliases);
        let mut specs: Vec<WireSpec> = flow
            .edges
            .iter()
            .enumerate()
            .map(|(i, e)| WireSpec {
                from: e.from,
                to: e.to,
                kind: match e.kind {
                    EdgeKind::Data => WireKind::Data,
                    EdgeKind::Mutation => WireKind::Mutation,
                    EdgeKind::PossibleMutation => WireKind::PossibleMutation,
                    EdgeKind::LoopCarried => WireKind::LoopBack,
                },
                certain: e.certain,
                name: flow.binding(e).name.clone(),
                source: i,
            })
            .collect();

        // Channels: every send-side place to every receive-side place.
        for (ci, ch) in conc.channels.iter().enumerate() {
            let ends = |end: End| {
                conc.channel_uses
                    .iter()
                    .filter(move |u| u.channel == ci && u.end == end)
            };
            for s in ends(End::Send) {
                for r in ends(End::Recv) {
                    if s.node != r.node {
                        specs.push(WireSpec {
                            from: s.node,
                            to: r.node,
                            kind: WireKind::Channel,
                            certain: true,
                            name: ch.name.clone(),
                            source: ci,
                        });
                    }
                }
            }
        }
        // Shared state: from where it is created to uses in other lanes and
        // to the statements that hand it to a new lane.
        let spawn_nodes: Vec<NodeId> = conc.spawns.iter().map(|s| s.node).collect();
        for (gi, g) in conc.shared.iter().enumerate() {
            let home = conc.lane_of(body, g.def);
            for u in conc.shared_uses.iter().filter(|u| u.group == gi) {
                if conc.lane_of(body, u.node) != home || spawn_nodes.contains(&u.node) {
                    specs.push(WireSpec {
                        from: g.def,
                        to: u.node,
                        kind: WireKind::Shared,
                        certain: true,
                        name: format!("{} ({})", g.name, g.kind),
                        source: gi,
                    });
                }
            }
        }

        // Lane badges and colors.
        let mut notes: rv_layout::Notes = HashMap::new();
        let mut lane_color = HashMap::new();
        let mut next_color = 0;
        for sp in &conc.spawns {
            let mut label = match (&sp.name, sp.kind) {
                (Some(name), _) => format!("‖ {} '{name}'", sp.kind.label()),
                (None, SpawnKind::BlockOn) => "async on this thread (block_on)".to_string(),
                (None, kind) => format!("‖ {}", kind.label()),
            };
            if sp.in_loop {
                label.push_str(" ×n");
            }
            let color = if sp.kind.is_parallel() {
                let c = LANE_COLORS[next_color % LANE_COLORS.len()];
                next_color += 1;
                Some(c)
            } else {
                None
            };
            let targets: Vec<NodeId> = if sp.lanes.is_empty() {
                if let Some(first) = sp.runs.first() {
                    label.push_str(&format!(" → {first}"));
                }
                vec![sp.node]
            } else {
                sp.lanes.clone()
            };
            for t in targets {
                notes
                    .entry(t)
                    .and_modify(|n: &mut String| {
                        n.push_str(" · ");
                        n.push_str(&label);
                    })
                    .or_insert_with(|| label.clone());
                if let Some(c) = color {
                    lane_color.entry(t).or_insert(c);
                }
            }
        }
        // Shift registers: one note per loop.
        let mut registers: HashMap<NodeId, Vec<String>> = HashMap::new();
        for &(lp, b) in &flow.shift_registers {
            registers
                .entry(lp)
                .or_default()
                .push(flow.bindings[b].name.clone());
        }
        for (lp, names) in &registers {
            let label = format!("⟳ {}", names.join(", "));
            notes
                .entry(*lp)
                .and_modify(|n: &mut String| {
                    n.push_str(" · ");
                    n.push_str(&label);
                })
                .or_insert(label);
        }
        Analysis {
            registers,
            flow,
            conc,
            specs,
            notes,
            lane_color,
        }
    }
}

fn tooltip(ui: &mut Ui, project: &Project, body: &Body, id: NodeId) {
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
    const MAX_LINES: usize = 16;
    let mut shown: String = snippet
        .lines()
        .take(MAX_LINES)
        .collect::<Vec<_>>()
        .join("\n");
    if snippet.lines().count() > MAX_LINES {
        shown.push_str("\n…");
    }
    ui.label(egui::RichText::new(shown).monospace());
    ui.weak("click: show in code · double click: open in editor");
}

/// Colors for the current theme.
struct Palette {
    dark: bool,
    text: Color32,
    weak: Color32,
    exit: Color32,
    hover: Color32,
    selected: Color32,
    frame_line: Color32,
    case_title: Color32,
    bg: Color32,
}

impl Palette {
    fn new(v: &egui::Visuals) -> Self {
        Palette {
            dark: v.dark_mode,
            text: v.text_color(),
            weak: v.weak_text_color(),
            exit: if v.dark_mode {
                Color32::from_rgb(255, 120, 110)
            } else {
                Color32::from_rgb(200, 40, 30)
            },
            hover: v.widgets.hovered.fg_stroke.color,
            selected: v.selection.stroke.color,
            frame_line: v.widgets.noninteractive.bg_stroke.color,
            case_title: if v.dark_mode {
                Color32::from_rgb(240, 190, 90)
            } else {
                Color32::from_rgb(150, 95, 0)
            },
            bg: v.extreme_bg_color,
        }
    }

    fn structure(&self, kind: &StructKind) -> Color32 {
        match kind {
            StructKind::Function => Color32::from_rgb(130, 130, 130),
            StructKind::Loop { .. } | StructKind::While { .. } | StructKind::For { .. } => {
                Color32::from_rgb(70, 130, 220)
            }
            StructKind::If | StructKind::Match => Color32::from_rgb(220, 160, 40),
            StructKind::Block { .. } | StructKind::LetElse => Color32::from_rgb(120, 135, 150),
            StructKind::Unsafe => Color32::from_rgb(210, 70, 70),
            StructKind::Closure { .. } => Color32::from_rgb(150, 100, 210),
            StructKind::Async { .. } => Color32::from_rgb(40, 170, 160),
        }
    }

    /// Nested frames alternate slightly so the depth stays readable.
    fn frame_bg(&self, depth: u32) -> Color32 {
        let step = if depth.is_multiple_of(2) { 0 } else { 10 };
        let [r, g, b, _] = self.bg.to_array();
        if self.dark {
            Color32::from_rgb(
                r.saturating_add(14 + step),
                g.saturating_add(14 + step),
                b.saturating_add(16 + step),
            )
        } else {
            Color32::from_rgb(
                r.saturating_sub(6 + step),
                g.saturating_sub(6 + step),
                b.saturating_sub(4 + step),
            )
        }
    }

    fn stmt_base(kind: &StmtKind) -> Color32 {
        match kind {
            StmtKind::Let => Color32::from_rgb(90, 170, 100),
            StmtKind::Assign => Color32::from_rgb(200, 170, 60),
            StmtKind::Call | StmtKind::MethodCall => Color32::from_rgb(80, 140, 210),
            StmtKind::Macro { .. } => Color32::from_rgb(140, 140, 140),
            StmtKind::Item => Color32::from_rgb(110, 110, 110),
            StmtKind::Exit(_) => Color32::from_rgb(220, 70, 60),
            StmtKind::Expr => Color32::from_rgb(150, 150, 170),
        }
    }

    fn stmt(&self, kind: &StmtKind) -> Color32 {
        let base = Self::stmt_base(kind);
        let mix = if self.dark { 0.28 } else { 0.18 };
        lerp(self.bg, base, mix)
    }

    fn stmt_line(&self, kind: &StmtKind) -> Color32 {
        lerp(self.bg, Self::stmt_base(kind), 0.7)
    }
}

/// Color of an expression node by kind (variables: their wire color).
fn expr_color(kind: rv_ir::ExprKind, text: &str, dark: bool, body: &Body) -> Color32 {
    use rv_ir::ExprKind as K;
    match kind {
        K::Var => wires::color(text, WireKind::Data, dark),
        K::Literal | K::Other => Color32::from_rgb(140, 140, 140),
        K::Call | K::Method | K::Macro => Color32::from_rgb(80, 140, 210),
        K::Operator | K::Field | K::Index | K::Cast | K::Ref | K::Try | K::Await => {
            Color32::from_rgb(200, 150, 40)
        }
        K::Construct | K::Closure => Color32::from_rgb(150, 100, 210),
        K::Bind => Color32::from_rgb(90, 170, 100),
        K::Assign => Color32::from_rgb(225, 120, 40),
        K::Return => Color32::from_rgb(220, 70, 60),
        K::Structure(id) => match &body.node(id).kind {
            NodeKind::Structure { kind, .. } if kind.is_loop() => Color32::from_rgb(70, 130, 220),
            NodeKind::Structure { kind, .. } if kind.is_case() => Color32::from_rgb(220, 160, 40),
            _ => Color32::from_rgb(150, 100, 210),
        },
    }
}

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}

impl DiagramView {
    /// The current function diagram (as laid out) as SVG in light colors.
    pub fn svg(&self, project: &Project, f: FnId) -> Option<String> {
        let body = project.function(f).body.as_ref()?;
        let (_, layout) = self.cached.as_ref()?;
        let (_, analysis) = self.analysis.as_ref()?;
        let wires = self
            .wire_cache
            .as_ref()
            .map_or(&[][..], |(_, w)| w.as_slice());
        Some(diagram_svg(
            body,
            layout,
            analysis,
            wires,
            self.wire_mode,
            self.selected,
        ))
    }
}

fn diagram_svg(
    body: &Body,
    layout: &Layout,
    analysis: &Analysis,
    wire_list: &[wires::Wire],
    mode: WireMode,
    selected: Option<NodeId>,
) -> String {
    use crate::export::Svg;
    let style = Palette::new(&egui::Visuals::light());
    // Which data wires to include: what the view shows without hovering.
    let data_wires = wire_list
        .iter()
        .filter(|w| !w.kind.is_concurrency())
        .count();
    let mode = match mode {
        WireMode::Auto if data_wires <= wires::AUTO_LIMIT => WireMode::All,
        WireMode::Auto => WireMode::Selected,
        other => other,
    };
    let shown: Vec<&wires::Wire> = wire_list
        .iter()
        .filter(|w| {
            w.kind.is_concurrency()
                || match mode {
                    WireMode::All => true,
                    WireMode::Off => false,
                    _ => selected.is_some_and(|n| w.touches(n, &analysis.specs)),
                }
        })
        .collect();
    let mut size = layout.size;
    for w in &shown {
        for p in w.points {
            size = size.max(p.to_vec2() + vec2(12.0, 12.0));
        }
    }
    let mut svg = Svg::new(size);
    let text = |svg: &mut Svg, t: &rv_layout::Text, color: Color32| {
        svg.text_fit(
            t.rect.min,
            &t.text,
            FONT_SIZE,
            color,
            true,
            false,
            Some(t.rect.width()),
        );
    };
    for item in &layout.items {
        let r = item.rect;
        let node = body.node(item.node);
        match (&item.kind, &node.kind) {
            (ItemKind::Structure(s), NodeKind::Structure { kind, .. }) => {
                let color = style.structure(kind);
                let width = if kind.is_loop() { 3.0 } else { 1.5 };
                svg.rect(r, style.frame_bg(item.depth), Some((width, color)), 4.0);
                svg.rect(s.header, color.gamma_multiply(0.35), None, 4.0);
                if let Some(&lane) = analysis.lane_color.get(&item.node) {
                    svg.rect(r.expand(3.0), Color32::TRANSPARENT, Some((3.0, lane)), 6.0);
                }
                if let Some(p) = &s.prefix {
                    text(&mut svg, p, style.weak);
                }
                text(&mut svg, &s.title, style.text);
                if let Some(n) = &s.note {
                    let c = analysis
                        .lane_color
                        .get(&item.node)
                        .copied()
                        .unwrap_or_else(|| wires::color("", WireKind::LoopBack, false));
                    text(&mut svg, n, c);
                }
                if let Some(i) = &s.info {
                    text(
                        &mut svg,
                        i,
                        if i.text.contains("exits") {
                            style.exit
                        } else {
                            style.weak
                        },
                    );
                }
                if s.expanded
                    && let Some(regs) = analysis.registers.get(&item.node)
                {
                    let c = wires::color("", WireKind::LoopBack, false);
                    for k in 0..regs.len() {
                        let y = s.header.max.y + 8.0 + k as f32 * 14.0;
                        for x in [r.min.x, r.max.x] {
                            svg.rect(
                                Rect::from_center_size(egui::pos2(x, y), vec2(9.0, 9.0)),
                                c,
                                None,
                                1.0,
                            );
                        }
                    }
                }
            }
            (ItemKind::Frame { title }, NodeKind::Frame { .. }) => {
                svg.rect(
                    r,
                    style.frame_bg(item.depth + 1),
                    Some((1.0, style.frame_line)),
                    4.0,
                );
                if let Some(t) = title {
                    text(&mut svg, t, style.case_title);
                }
            }
            (
                ItemKind::Stmt {
                    text: t,
                    badges,
                    note,
                },
                NodeKind::Stmt { kind },
            ) => {
                svg.rect(r, style.stmt(kind), Some((1.0, style.stmt_line(kind))), 4.0);
                text(&mut svg, t, style.text);
                if let Some(b) = badges {
                    text(&mut svg, b, style.exit);
                }
                if let Some(&lane) = analysis.lane_color.get(&item.node) {
                    svg.rect(
                        Rect::from_min_size(r.min, vec2(4.0, r.height())),
                        lane,
                        None,
                        0.0,
                    );
                    if let Some(n) = note {
                        text(&mut svg, n, lane);
                    }
                }
            }
            _ => {}
        }
    }
    // Exit markers where control leaves a structure.
    let mut marks: HashSet<(NodeId, i32)> = HashSet::new();
    for e in &body.exits {
        let (Some(target), Some(from)) = (e.target, layout.item(e.from)) else {
            continue;
        };
        let y = from.rect.min.y + (from.rect.height() / 2.0).min(11.0);
        let mut cur = body.node(e.from).parent;
        while let Some(n) = cur {
            if n == target {
                break;
            }
            if let Some(item) = layout.item(n)
                && matches!(item.kind, ItemKind::Structure(_))
                && marks.insert((n, y as i32))
            {
                let tip = egui::pos2(item.rect.max.x + 7.0, y);
                svg.polygon(
                    &[tip, tip + vec2(-10.0, -5.0), tip + vec2(-10.0, 5.0)],
                    style.exit,
                );
            }
            cur = body.node(n).parent;
        }
    }
    // Expression trees.
    for &(a, b) in &layout.expr_wires {
        svg.arrow(&[a, b], 1.2, style.weak, None);
    }
    for e in &layout.exprs {
        let tree = body
            .node(e.stmt)
            .expr
            .as_ref()
            .expect("expression items come from trees");
        let n = &tree.nodes[e.index];
        let base = expr_color(n.kind, &n.text, false, body);
        svg.rect(e.rect, lerp(style.bg, base, 0.22), Some((1.0, base)), 3.0);
        text(&mut svg, &e.text, style.text);
    }
    // Wires.
    for w in shown {
        let pts = w.sample().collect::<Vec<_>>();
        let color = wires::color(&w.name, w.kind, false);
        let (width, dash) = match w.kind {
            WireKind::Channel => (3.0, None),
            WireKind::Shared => (2.0, Some((2.0, 4.0))),
            WireKind::LoopBack => (2.0, Some((9.0, 4.0))),
            WireKind::PossibleMutation => (1.4, Some((6.0, 4.0))),
            _ if !w.certain => (1.4, Some((6.0, 4.0))),
            WireKind::Mutation => (1.6, None),
            WireKind::Data => (1.4, None),
        };
        if w.kind == WireKind::Shared {
            svg.polyline(&pts, width, color, dash);
        } else {
            svg.arrow(&pts, width, color, dash);
        }
    }
    svg.finish()
}

/// Diagram of a function with default expansion, for exports without a window.
pub(crate) fn function_svg(project: &Project, f: FnId) -> Option<String> {
    let body = project.function(f).body.as_ref()?;
    let analysis = Analysis::new(body, project);
    let state = ViewState::default();
    let defaults = rv_layout::default_expanded(body, &state, BUDGET);
    // Metrics of the 12 pt monospace font used in the GUI.
    let metrics = Metrics {
        char_width: 7.2,
        line_height: 15.0,
        max_label_chars: MAX_LABEL_CHARS,
    };
    let layout = rv_layout::layout(
        body,
        &state,
        &defaults,
        Orientation::Vertical,
        metrics,
        &analysis.notes,
    );
    let wire_list = wires::geometry(body, &analysis.specs, &layout, Orientation::Vertical);
    Some(diagram_svg(
        body,
        &layout,
        &analysis,
        &wire_list,
        WireMode::Auto,
        None,
    ))
}
