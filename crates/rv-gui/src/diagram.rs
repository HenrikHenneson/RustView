//! Center panel: the structure diagram of the selected function.

use std::collections::{HashMap, HashSet};

use egui::{Color32, CornerRadius, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, vec2};
use rv_ir::{Body, FnId, NodeId, NodeKind, Project, StmtKind, StructKind};
use rv_layout::{ItemKind, Layout, Metrics, Orientation, ViewState};

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
    pub selected: Option<NodeId>,
    /// Center this node on the next frame.
    scroll_to: Option<NodeId>,
    /// Canvas of the last frame, for "Fit".
    canvas: Option<Rect>,
}

impl DiagramView {
    /// Forget everything except the orientation (another project was opened).
    pub fn reset(&mut self) {
        *self = DiagramView {
            orientation: self.orientation,
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
        let Some(view) = self.views.get_mut(&func.qual_path) else {
            return;
        };
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
        let Some(node) = innermost else { return };
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
            .or_insert_with(|| new_view(body));
        if view.node_count != body.nodes.len() {
            *view = new_view(body);
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
        if self.cached.as_ref().is_none_or(|(k, _)| *k != key) {
            let l = rv_layout::layout(body, &view.state, &view.defaults, self.orientation, metrics);
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
            if let Some(item) = layout.hit(world) {
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
                    draw_triangle(&painter, to_screen(s.toggle), s.expanded, style.text);
                    if let Some(p) = &s.prefix {
                        text(p.rect, &p.text, style.weak);
                    }
                    text(s.title.rect, &s.title.text, style.text);
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
                (ItemKind::Stmt { text: t, badges }, NodeKind::Stmt { kind }) => {
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

        if let Some(item) = hovered {
            response.on_hover_ui_at_pointer(|ui| tooltip(ui, project, body, item.node));
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
            ui.weak("drag: move · wheel: scroll · Ctrl+wheel: zoom");
        });
    }
}

fn new_view(body: &Body) -> FnView {
    let state = ViewState::default();
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

fn lerp(a: Color32, b: Color32, t: f32) -> Color32 {
    let mix = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t).round() as u8;
    Color32::from_rgb(mix(a.r(), b.r()), mix(a.g(), b.g()), mix(a.b(), b.b()))
}
