//! Program graph: threads as boxes in columns by spawn depth, arrows for
//! "starts", channels and shared state between them.

use std::collections::HashMap;

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2,
    pos2, vec2,
};
use rv_analysis::{LinkKind, Program};
use rv_ir::{FnId, NodeId, Project};

use crate::threads::ThreadsAction;

const BOX_W: f32 = 280.0;
const COL_GAP: f32 = 140.0;
const ROW_GAP: f32 = 28.0;
const LINE: f32 = 17.0;
const PAD: f32 = 8.0;
const MARGIN: f32 = 20.0;

const SPAWN_COLOR: Color32 = Color32::from_rgb(130, 130, 130);
const CHANNEL_COLOR: Color32 = Color32::from_rgb(0, 150, 165);
const SHARED_COLOR: Color32 = Color32::from_rgb(150, 90, 210);
const THREAD_COLOR: Color32 = Color32::from_rgb(0, 160, 120);

/// Geometry of one program graph.
pub struct GraphLayout {
    boxes: Vec<Block>,
    size: Vec2,
}

struct Block {
    kind: BlockKind,
    rect: Rect,
    lines: Vec<(String, LineStyle)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BlockKind {
    Thread(usize),
    Resource(usize),
}

#[derive(Clone, Copy)]
enum LineStyle {
    Title,
    Normal,
    Weak,
}

pub struct ProgramView {
    pub zoom: f32,
    pub pan: Vec2,
    pub selected: Option<usize>,
}

impl Default for ProgramView {
    fn default() -> Self {
        ProgramView {
            zoom: 1.0,
            pan: Vec2::ZERO,
            selected: None,
        }
    }
}

pub fn layout(project: &Project, program: &Program) -> GraphLayout {
    // Column = spawn depth (shortest chain of starts from the main thread).
    let n = program.threads.len();
    let mut depth = vec![usize::MAX; n];
    if n > 0 {
        depth[0] = 0;
    }
    for _ in 0..n {
        for (i, t) in program.threads.iter().enumerate() {
            for &p in &t.spawned_by {
                if depth[p] != usize::MAX {
                    depth[i] = depth[i].min(depth[p] + 1);
                }
            }
        }
    }
    let fn_name = |f: FnId| project.function(f).name.clone();
    // `Abfrage::starten` or `lkg::starten`: enough to tell same-named places apart.
    let short_path = |f: FnId| {
        let q = &project.function(f).qual_path;
        let parts: Vec<&str> = q.split("::").collect();
        parts[parts.len().saturating_sub(3)..].join("::")
    };
    let method_name = |f: FnId| {
        let func = project.function(f);
        match func.owner.group_label() {
            Some(_) => {
                let parts: Vec<&str> = func.qual_path.split("::").collect();
                parts[parts.len().saturating_sub(2)..].join("::")
            }
            None => func.name.clone(),
        }
    };
    let mut boxes = Vec::new();
    let mut column_y: HashMap<usize, f32> = HashMap::new();
    let mut max_depth = 0;
    for (i, t) in program.threads.iter().enumerate() {
        let d = if depth[i] == usize::MAX { 1 } else { depth[i] };
        max_depth = max_depth.max(d);
        let mut lines = vec![(t.title(), LineStyle::Title)];
        if let Some((f, node)) = t.site {
            let line = project
                .function(f)
                .body
                .as_ref()
                .map_or(0, |b| b.node(node).span.start.line);
            lines.push((
                format!("started in {}:{line}", short_path(f)),
                LineStyle::Weak,
            ));
        }
        let entries: Vec<String> = t.entries.iter().take(3).map(|&f| fn_name(f)).collect();
        if !entries.is_empty() {
            let more = if t.entries.len() > 3 { ", …" } else { "" };
            lines.push((format!("→ {}{more}", entries.join(", ")), LineStyle::Normal));
        }
        if !t.callbacks.is_empty() {
            let cb: Vec<String> = t
                .callbacks
                .iter()
                .take(3)
                .map(|&f| method_name(f))
                .collect();
            lines.push((format!("callbacks: {}", cb.join(", ")), LineStyle::Weak));
        }
        lines.push((format!("{} functions", t.runs.len()), LineStyle::Weak));
        let h = lines.len() as f32 * LINE + 2.0 * PAD;
        let y = column_y.entry(d).or_insert(MARGIN);
        let x = MARGIN + d as f32 * (BOX_W + COL_GAP);
        boxes.push(Block {
            kind: BlockKind::Thread(i),
            rect: Rect::from_min_size(pos2(x, *y), vec2(BOX_W, h)),
            lines,
        });
        *y += h + ROW_GAP;
    }
    // Shared state in its own column on the right, each object at the height
    // of the threads using it (keeps the dotted lines short).
    let x = MARGIN + (max_depth + 1) as f32 * (BOX_W + COL_GAP);
    let thread_center = |t: usize| {
        boxes
            .iter()
            .find(|b: &&Block| b.kind == BlockKind::Thread(t))
            .map_or(MARGIN, |b| b.rect.center().y)
    };
    let mut order: Vec<(usize, f32)> = program
        .resources
        .iter()
        .enumerate()
        .map(|(i, r)| {
            // The main thread uses nearly everything; place by the other users.
            let others: Vec<usize> = r.users.iter().copied().filter(|&u| u != 0).collect();
            let users = if others.is_empty() { &r.users } else { &others };
            let ys: Vec<f32> = users.iter().map(|&u| thread_center(u)).collect();
            (i, ys.iter().sum::<f32>() / ys.len().max(1) as f32)
        })
        .collect();
    order.sort_by(|a, b| a.1.total_cmp(&b.1));
    let mut y = MARGIN;
    for (i, center) in order {
        let r = &program.resources[i];
        let lines = vec![
            (format!("{} ({})", r.name, r.kind), LineStyle::Title),
            (
                format!("created in {}", short_path(r.site.0)),
                LineStyle::Weak,
            ),
            (format!("{} threads", r.users.len()), LineStyle::Weak),
        ];
        let h = lines.len() as f32 * LINE + 2.0 * PAD;
        y = y.max(center - h / 2.0);
        boxes.push(Block {
            kind: BlockKind::Resource(i),
            rect: Rect::from_min_size(pos2(x, y), vec2(BOX_W * 0.8, h)),
            lines,
        });
        y += h + ROW_GAP / 2.0;
    }
    let max = boxes.iter().fold(Pos2::ZERO, |m, b| m.max(b.rect.max));
    GraphLayout {
        boxes,
        size: max.to_vec2() + Vec2::splat(MARGIN),
    }
}

/// An arrow between two boxes, for painting and hit testing.
struct Arrow {
    points: [Pos2; 4],
    color: Color32,
    width: f32,
    dashed: bool,
    head: bool,
    label: Option<String>,
    tooltip: String,
    ends: (BlockKind, BlockKind),
}

fn arrows(project: &Project, program: &Program, g: &GraphLayout) -> Vec<Arrow> {
    let rect_of = |k: BlockKind| g.boxes.iter().find(|b| b.kind == k).map(|b| b.rect);
    let site_text = |(f, node): (FnId, NodeId)| {
        let func = project.function(f);
        let line = func
            .body
            .as_ref()
            .map_or(0, |b| b.node(node).span.start.line);
        format!("{}:{line}", func.qual_path)
    };
    let mut out = Vec::new();
    // Several channels between the same pair are fanned out.
    let mut pair_count: HashMap<(usize, usize), usize> = HashMap::new();
    for l in &program.links {
        let (Some(a), Some(b)) = (
            rect_of(BlockKind::Thread(l.from)),
            rect_of(BlockKind::Thread(l.to)),
        ) else {
            continue;
        };
        let k = pair_count
            .entry((l.from.min(l.to), l.from.max(l.to)))
            .or_insert(0);
        let offset = *k as f32 * 14.0;
        *k += 1;
        let points = connect(a, b, offset);
        let (color, width, label, tooltip) = match &l.kind {
            LinkKind::Spawn => (
                SPAWN_COLOR,
                1.5,
                None,
                format!(
                    "starts {}\nat {}",
                    program.threads[l.to].title(),
                    site_text(l.site)
                ),
            ),
            LinkKind::Channel { name, api } => (
                CHANNEL_COLOR,
                3.0,
                Some(name.clone()),
                format!("channel {name} ({api})\ncreated at {}", site_text(l.site)),
            ),
        };
        out.push(Arrow {
            points,
            color,
            width,
            dashed: false,
            head: true,
            label,
            tooltip,
            ends: (BlockKind::Thread(l.from), BlockKind::Thread(l.to)),
        });
    }
    for (ri, r) in program.resources.iter().enumerate() {
        let Some(rb) = rect_of(BlockKind::Resource(ri)) else {
            continue;
        };
        for &u in &r.users {
            let Some(ub) = rect_of(BlockKind::Thread(u)) else {
                continue;
            };
            out.push(Arrow {
                points: connect(ub, rb, 0.0),
                color: SHARED_COLOR,
                width: 1.5,
                dashed: true,
                head: false,
                label: None,
                tooltip: format!(
                    "{} uses shared {} ({})\ncreated at {}",
                    program.threads[u].title(),
                    r.name,
                    r.kind,
                    site_text(r.site)
                ),
                ends: (BlockKind::Thread(u), BlockKind::Resource(ri)),
            });
        }
    }
    out
}

/// Bézier from box `a` to box `b`, leaving and entering on the facing sides.
fn connect(a: Rect, b: Rect, offset: f32) -> [Pos2; 4] {
    let ay = a.min.y + (LINE + PAD).min(a.height() / 2.0) + offset;
    let by = b.min.y + (LINE + PAD).min(b.height() / 2.0) + offset;
    if b.min.x > a.max.x {
        let (p0, p3) = (pos2(a.max.x, ay), pos2(b.min.x, by));
        let dx = (p3.x - p0.x) * 0.5;
        [p0, p0 + vec2(dx, 0.0), p3 - vec2(dx, 0.0), p3]
    } else if b.max.x < a.min.x {
        let (p0, p3) = (pos2(a.min.x, ay), pos2(b.max.x, by));
        let dx = (p0.x - p3.x) * 0.5;
        [p0, p0 - vec2(dx, 0.0), p3 + vec2(dx, 0.0), p3]
    } else {
        // Same column: arc out to the right.
        let (p0, p3) = (pos2(a.max.x, ay), pos2(b.max.x, by));
        let out = 40.0 + 0.15 * (p3.y - p0.y).abs() + offset;
        let x = p0.x.max(p3.x) + out;
        [p0, pos2(x, p0.y), pos2(x, p3.y), p3]
    }
}

fn sample(p: &[Pos2; 4], steps: usize) -> Vec<Pos2> {
    (0..=steps)
        .map(|i| {
            let t = i as f32 / steps as f32;
            let u = 1.0 - t;
            (p[0].to_vec2() * (u * u * u)
                + p[1].to_vec2() * (3.0 * u * u * t)
                + p[2].to_vec2() * (3.0 * u * t * t)
                + p[3].to_vec2() * (t * t * t))
                .to_pos2()
        })
        .collect()
}

impl ProgramView {
    pub fn fit(&mut self, g: &GraphLayout, canvas: Vec2) {
        self.zoom = (canvas.x / g.size.x)
            .min(canvas.y / g.size.y)
            .clamp(0.2, 1.0);
        self.pan = Vec2::ZERO;
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
        let visuals = ui.visuals().clone();
        painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);

        if response.dragged() {
            self.pan += response.drag_delta();
        }
        if response.hovered() {
            let (zoom_delta, scroll) = ui.input(|i| (i.zoom_delta(), i.smooth_scroll_delta()));
            if zoom_delta != 1.0
                && let Some(p) = response.hover_pos()
            {
                let world = (p - rect.min - self.pan) / self.zoom;
                self.zoom = (self.zoom * zoom_delta).clamp(0.1, 3.0);
                self.pan = p - rect.min - world * self.zoom;
            } else {
                self.pan += scroll;
            }
        }
        let (zoom, origin) = (self.zoom, rect.min + self.pan);
        let to_screen = |p: Pos2| origin + p.to_vec2() * zoom;
        let to_world = |p: Pos2| ((p - origin) / zoom).to_pos2();

        let arrows = arrows(project, program, g);
        let pointer = response.hover_pos().map(to_world);
        let hovered_box = pointer.and_then(|p| g.boxes.iter().rev().find(|b| b.rect.contains(p)));
        let hovered_arrow = if hovered_box.is_none() {
            pointer.and_then(|p| {
                arrows
                    .iter()
                    .enumerate()
                    .filter_map(|(i, a)| {
                        let d = sample(&a.points, 24)
                            .windows(2)
                            .map(|s| dist(p, s[0], s[1]))
                            .fold(f32::INFINITY, f32::min);
                        (d < 6.0 / zoom).then_some((i, d))
                    })
                    .min_by(|a, b| a.1.total_cmp(&b.1))
                    .map(|(i, _)| i)
            })
        } else {
            None
        };

        if response.clicked() {
            self.selected = match hovered_box.map(|b| b.kind) {
                Some(BlockKind::Thread(t)) => Some(t),
                _ => None,
            };
        }
        if response.double_clicked()
            && let Some(BlockKind::Thread(t)) = hovered_box.map(|b| b.kind)
        {
            let th = &program.threads[t];
            action = match th.site {
                Some((f, node)) => ThreadsAction::ShowNode(f, node),
                None => th
                    .runs
                    .first()
                    .map_or(ThreadsAction::None, |&f| ThreadsAction::ShowFn(f)),
            };
        }

        // Arrows below the boxes.
        let focus = self.selected.map(BlockKind::Thread);
        for (i, a) in arrows.iter().enumerate() {
            let related = focus.is_none_or(|f| a.ends.0 == f || a.ends.1 == f);
            let alpha = if related { 1.0 } else { 0.15 };
            let width =
                (a.width + if hovered_arrow == Some(i) { 1.5 } else { 0.0 }) * zoom.max(0.5);
            let color = a.color.gamma_multiply(alpha);
            let pts: Vec<Pos2> = sample(&a.points, 24).into_iter().map(to_screen).collect();
            let stroke = Stroke::new(width, color);
            if a.dashed {
                painter.extend(Shape::dashed_line(&pts, stroke, 3.0, 4.0));
            } else {
                painter.add(Shape::line(pts.clone(), stroke));
            }
            if a.head {
                let n = pts.len();
                let dir = (pts[n - 1] - pts[n - 2]).normalized();
                if dir.is_finite() {
                    let size = 6.0 + width;
                    let side = vec2(-dir.y, dir.x) * (size * 0.5);
                    let back = pts[n - 1] - dir * size;
                    painter.add(Shape::convex_polygon(
                        vec![pts[n - 1], back + side, back - side],
                        color,
                        Stroke::NONE,
                    ));
                }
            }
            if let Some(label) = &a.label
                && zoom > 0.4
            {
                let mid = pts[pts.len() / 2];
                let galley =
                    painter.layout_no_wrap(label.clone(), FontId::monospace(11.0 * zoom), color);
                let r = Align2::CENTER_CENTER
                    .anchor_size(mid, galley.size())
                    .expand(2.0);
                painter.rect_filled(r, 3.0, visuals.extreme_bg_color.gamma_multiply(0.9));
                painter.galley(r.min + vec2(2.0, 2.0), galley, color);
            }
        }

        // Boxes.
        for b in &g.boxes {
            let r = Rect::from_min_max(to_screen(b.rect.min), to_screen(b.rect.max));
            if !r.intersects(rect) {
                continue;
            }
            let (border, width) = match b.kind {
                BlockKind::Thread(0) => (visuals.text_color(), 1.5),
                BlockKind::Thread(_) => (THREAD_COLOR, 2.0),
                BlockKind::Resource(_) => (SHARED_COLOR, 1.5),
            };
            let radius = CornerRadius::same((6.0 * zoom).min(255.0) as u8);
            painter.rect_filled(r, radius, visuals.panel_fill);
            painter.rect_stroke(r, radius, Stroke::new(width, border), StrokeKind::Inside);
            if focus == Some(b.kind) {
                painter.rect_stroke(
                    r.expand(2.0),
                    radius,
                    Stroke::new(2.5, visuals.selection.stroke.color),
                    StrokeKind::Outside,
                );
            } else if hovered_box.is_some_and(|h| h.kind == b.kind) {
                painter.rect_stroke(
                    r.expand(1.0),
                    radius,
                    Stroke::new(1.5, visuals.widgets.hovered.fg_stroke.color),
                    StrokeKind::Outside,
                );
            }
            if zoom < 0.35 {
                continue;
            }
            for (i, (text, style)) in b.lines.iter().enumerate() {
                let (font, color) = match style {
                    LineStyle::Title => (
                        FontId::proportional(13.0 * zoom),
                        visuals.strong_text_color(),
                    ),
                    LineStyle::Normal => (FontId::monospace(12.0 * zoom), visuals.text_color()),
                    LineStyle::Weak => {
                        (FontId::proportional(12.0 * zoom), visuals.weak_text_color())
                    }
                };
                let pos = to_screen(b.rect.min + vec2(PAD, PAD + i as f32 * LINE));
                let max_chars = 40;
                let shown: String = if text.chars().count() > max_chars {
                    text.chars().take(max_chars - 1).chain(['…']).collect()
                } else {
                    text.clone()
                };
                painter.text(pos, Align2::LEFT_TOP, shown, font, color);
            }
        }

        if let Some(b) = hovered_box {
            response.on_hover_ui_at_pointer(|ui| box_tooltip(ui, project, program, b.kind));
        } else if let Some(i) = hovered_arrow {
            let text = arrows[i].tooltip.clone();
            response.on_hover_ui_at_pointer(|ui| {
                ui.label(text);
            });
        }
        action
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
            for &f in th.runs.iter().take(15) {
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

fn dist(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// The program graph as SVG in light colors.
pub fn svg(project: &Project, program: &Program, g: &GraphLayout) -> String {
    use crate::export::Svg;
    let mut svg = Svg::new(g.size);
    for a in arrows(project, program, g) {
        let pts = sample(&a.points, 24);
        if a.dashed {
            svg.polyline(&pts, a.width, a.color, Some((3.0, 4.0)));
        } else if a.head {
            svg.arrow(&pts, a.width, a.color, None);
        } else {
            svg.polyline(&pts, a.width, a.color, None);
        }
        if let Some(label) = &a.label {
            let mid = pts[pts.len() / 2];
            let w = label.chars().count() as f32 * 6.6 + 4.0;
            svg.rect(
                Rect::from_center_size(mid, vec2(w, 15.0)),
                Color32::WHITE,
                None,
                3.0,
            );
            svg.text_fit(
                mid - vec2(w / 2.0 - 2.0, 6.5),
                label,
                11.0,
                a.color,
                true,
                false,
                Some(w - 4.0),
            );
        }
    }
    for b in &g.boxes {
        let border = match b.kind {
            BlockKind::Thread(0) => Color32::from_gray(60),
            BlockKind::Thread(_) => THREAD_COLOR,
            BlockKind::Resource(_) => SHARED_COLOR,
        };
        svg.rect(b.rect, Color32::WHITE, Some((2.0, border)), 6.0);
        for (i, (text, style)) in b.lines.iter().enumerate() {
            let (size, color, mono, bold) = match style {
                LineStyle::Title => (13.0, Color32::from_gray(20), false, true),
                LineStyle::Normal => (12.0, Color32::from_gray(40), true, false),
                LineStyle::Weak => (12.0, Color32::from_gray(120), false, false),
            };
            let shown: String = if text.chars().count() > 40 {
                text.chars().take(39).chain(['…']).collect()
            } else {
                text.clone()
            };
            svg.text(
                b.rect.min + vec2(PAD, PAD + i as f32 * LINE),
                &shown,
                size,
                color,
                mono,
                bold,
            );
        }
    }
    svg.finish()
}
