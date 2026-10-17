//! Call graph on module level (M5): modules as boxes, calls between them as
//! arrows, laid out in layers (callers left, callees right). Double-clicking
//! a module drills down to its functions with their callers and callees.

use std::collections::BTreeMap;

use egui::{
    Align2, Color32, CornerRadius, FontId, Pos2, Rect, RichText, Sense, Shape, Stroke, StrokeKind,
    Ui, Vec2, pos2, vec2,
};
use rv_analysis::{CallGraph, Facts, ModuleGraph};
use rv_ir::{FnId, Project};
use rv_layout::layered::{LayeredLayout, layered};

const LINE: f32 = 16.0;
const PAD: f32 = 7.0;
const CALLS_OUT: Color32 = Color32::from_rgb(40, 110, 220);
const CALLS_IN: Color32 = Color32::from_rgb(30, 150, 80);
const THREADS: Color32 = Color32::from_rgb(0, 160, 120);

pub enum CallGraphAction {
    None,
    ShowFn(FnId),
    /// Status message (e.g. after an export).
    Message(String),
}

#[derive(Default)]
pub struct CallGraphView {
    data: Option<(u64, Data)>,
    crate_index: Option<usize>,
    depth: usize,
    graph: Option<(GraphKey, ModuleGraph, LayeredLayout, Vec<Vec2>)>,
    /// Drill-down: node of the current graph.
    focus: Option<usize>,
    selected: Option<usize>,
    zoom: f32,
    pan: Vec2,
    fit: bool,
    /// Module to drill into once the graph exists (`--callgraph ec_task`).
    open_module: Option<String>,
}

struct Data {
    facts: Facts,
    calls: CallGraph,
    crates: Vec<String>,
    default_crate: usize,
}

#[derive(Clone, PartialEq)]
struct GraphKey {
    generation: u64,
    crate_name: String,
    depth: usize,
}

impl CallGraphView {
    pub fn opening(module: Option<&str>) -> Self {
        CallGraphView {
            open_module: module.filter(|m| !m.is_empty()).map(str::to_string),
            ..CallGraphView::default()
        }
    }

    pub fn ui(&mut self, ui: &mut Ui, project: &Project, generation: u64) -> CallGraphAction {
        if self.data.as_ref().is_none_or(|(g, _)| *g != generation) {
            let crates = rv_analysis::crates(project);
            // Default: the crate of the first `main`.
            let default_crate = rv_analysis::entry_points(project)
                .first()
                .and_then(|&f| {
                    let c = project
                        .function(f)
                        .qual_path
                        .split("::")
                        .next()?
                        .to_string();
                    crates.iter().position(|x| *x == c)
                })
                .unwrap_or(0);
            self.data = Some((
                generation,
                Data {
                    facts: Facts::new(project),
                    calls: CallGraph::new(project),
                    crates,
                    default_crate,
                },
            ));
        }
        if self.depth == 0 {
            self.depth = 2;
        }
        let data = &self.data.as_ref().expect("computed above").1;
        if data.crates.is_empty() {
            ui.weak("No modules.");
            return CallGraphAction::None;
        }
        let crate_index = self
            .crate_index
            .unwrap_or(data.default_crate)
            .min(data.crates.len() - 1);

        // Toolbar.
        let mut changed = false;
        let mut export = false;
        ui.horizontal_wrapped(|ui| {
            ui.label("crate:");
            let mut ci = crate_index;
            egui::ComboBox::from_id_salt("cg_crate")
                .selected_text(RichText::new(&data.crates[ci]).monospace())
                .show_ui(ui, |ui| {
                    for (i, c) in data.crates.iter().enumerate() {
                        ui.selectable_value(&mut ci, i, RichText::new(c).monospace());
                    }
                });
            if ci != crate_index {
                self.crate_index = Some(ci);
                changed = true;
            }
            ui.label("group modules to depth:");
            for d in 1..=4 {
                let label = if d == 4 {
                    "all".to_string()
                } else {
                    d.to_string()
                };
                if ui.selectable_label(self.depth == d, label).clicked() && self.depth != d {
                    self.depth = d;
                    changed = true;
                }
            }
            if self.focus.is_none() && ui.button("Fit").clicked() {
                self.fit = true;
            }
            if self.focus.is_none()
                && ui
                    .button("Export…")
                    .on_hover_text("Save the module graph as SVG or PNG")
                    .clicked()
            {
                export = true;
            }
            ui.weak("· double click a module: its functions · drag: move · Ctrl+wheel: zoom");
        });
        if changed {
            self.focus = None;
            self.selected = None;
            self.fit = true;
        }
        let crate_name = data.crates[self
            .crate_index
            .unwrap_or(data.default_crate)
            .min(data.crates.len() - 1)]
        .clone();
        let key = GraphKey {
            generation,
            crate_name: crate_name.clone(),
            depth: if self.depth >= 4 {
                usize::MAX
            } else {
                self.depth
            },
        };
        if self.graph.as_ref().is_none_or(|(k, ..)| *k != key) {
            let (g, l, sizes) =
                module_layout(project, &data.calls, &data.facts, &crate_name, key.depth);
            self.graph = Some((key, g, l, sizes));
            self.fit = true;
        }
        if let Some(wanted) = self.open_module.take()
            && let Some((_, g, ..)) = &self.graph
        {
            let suffix = format!("::{wanted}");
            self.focus = g
                .nodes
                .iter()
                .position(|n| n.key == wanted || n.key.ends_with(&suffix));
        }
        ui.separator();
        if export {
            let name = format!("{}_modules", crate::export::file_name(&crate_name));
            if let Some(msg) = crate::export::export(&name, || self.svg().unwrap_or_default()) {
                return CallGraphAction::Message(msg);
            }
        }
        match self.focus {
            Some(node) => self.focus_ui(ui, project, &crate_name, node),
            None => self.overview_ui(ui, project, &crate_name),
        }
    }

    fn overview_ui(&mut self, ui: &mut Ui, project: &Project, crate_name: &str) -> CallGraphAction {
        let (_, g, l, sizes) = self.graph.as_ref().expect("computed by caller");
        let (rect, response) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
        let painter = ui.painter_at(rect);
        let visuals = ui.visuals().clone();
        painter.rect_filled(rect, 0.0, visuals.extreme_bg_color);
        if std::mem::take(&mut self.fit) {
            let margin = Vec2::splat(20.0);
            let avail = rect.size() - 2.0 * margin;
            self.zoom = (avail.x / l.size.x.max(1.0))
                .min(avail.y / l.size.y.max(1.0))
                .clamp(0.15, 1.0);
            self.pan = margin;
        }
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
        let box_of = |i: usize| Rect::from_min_size(l.positions[i], sizes[i]);
        let pointer = response
            .hover_pos()
            .map(|p| ((p - origin) / zoom).to_pos2());
        let hovered_node =
            pointer.and_then(|p| (0..g.nodes.len()).find(|&i| box_of(i).contains(p)));

        // Edges.
        let mut hovered_edge = None;
        let curves: Vec<[Pos2; 4]> = g
            .edges
            .iter()
            .map(|e| connect(box_of(e.from), box_of(e.to)))
            .collect();
        if hovered_node.is_none()
            && let Some(p) = pointer
        {
            hovered_edge = curves
                .iter()
                .enumerate()
                .filter_map(|(i, c)| {
                    let d = sample(c)
                        .windows(2)
                        .map(|s| dist(p, s[0], s[1]))
                        .fold(f32::INFINITY, f32::min);
                    (d < 6.0 / zoom).then_some((i, d))
                })
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| i);
        }
        let focus = self.selected.or(hovered_node);
        for (i, (e, c)) in g.edges.iter().zip(&curves).enumerate() {
            let (color, alpha) = match focus {
                Some(f) if e.from == f => (CALLS_OUT, 1.0),
                Some(f) if e.to == f => (CALLS_IN, 1.0),
                Some(_) => (visuals.weak_text_color(), 0.15),
                None => (visuals.weak_text_color(), 0.7),
            };
            let width = (1.0 + (e.calls.len() as f32).log2()) * zoom.max(0.5)
                + if hovered_edge == Some(i) { 1.5 } else { 0.0 };
            let pts: Vec<Pos2> = sample(c).into_iter().map(to_screen).collect();
            let color = color.gamma_multiply(alpha);
            painter.add(Shape::line(pts.clone(), Stroke::new(width, color)));
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
        // Nodes.
        for (i, node) in g.nodes.iter().enumerate() {
            let r = Rect::from_min_max(to_screen(box_of(i).min), to_screen(box_of(i).max));
            if !r.intersects(rect) {
                continue;
            }
            let radius = CornerRadius::same((5.0 * zoom).min(255.0) as u8);
            painter.rect_filled(r, radius, visuals.panel_fill);
            let border = if node.spawns > 0 {
                THREADS
            } else {
                visuals.widgets.noninteractive.fg_stroke.color
            };
            painter.rect_stroke(
                r,
                radius,
                Stroke::new(if node.spawns > 0 { 2.0 } else { 1.0 }, border),
                StrokeKind::Inside,
            );
            if focus == Some(i) {
                painter.rect_stroke(
                    r.expand(2.0),
                    radius,
                    Stroke::new(2.0, visuals.selection.stroke.color),
                    StrokeKind::Outside,
                );
            }
            if zoom < 0.3 {
                continue;
            }
            for (k, (text, strong)) in node_lines(node, crate_name).iter().enumerate() {
                let (font, color) = if *strong {
                    (FontId::monospace(12.5 * zoom), visuals.strong_text_color())
                } else if text.starts_with('‖') {
                    (FontId::proportional(11.5 * zoom), THREADS)
                } else {
                    (FontId::proportional(11.5 * zoom), visuals.weak_text_color())
                };
                painter.text(
                    to_screen(box_of(i).min + vec2(PAD, PAD + k as f32 * LINE)),
                    Align2::LEFT_TOP,
                    text,
                    font,
                    color,
                );
            }
        }

        if response.clicked() {
            self.selected = hovered_node;
        }
        if response.double_clicked()
            && let Some(n) = hovered_node
        {
            self.focus = Some(n);
        }
        if let Some(n) = hovered_node {
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
            response.on_hover_ui_at_pointer(|ui| {
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
                        RichText::new(format!("starts threads/tasks at {} places", node.spawns))
                            .color(THREADS),
                    );
                }
                ui.weak("click: highlight · double click: show functions");
            });
        } else if let Some(i) = hovered_edge {
            let e = &g.edges[i];
            let text = format!(
                "{} → {}: {} calls",
                g.nodes[e.from].key,
                g.nodes[e.to].key,
                e.calls.len()
            );
            let calls: Vec<String> = e
                .calls
                .iter()
                .take(12)
                .map(|&(a, b)| {
                    format!(
                        "{} → {}",
                        project.function(a).name,
                        project.function(b).name
                    )
                })
                .collect();
            let more = e.calls.len().saturating_sub(12);
            response.on_hover_ui_at_pointer(|ui| {
                ui.label(text);
                for c in &calls {
                    ui.monospace(c);
                }
                if more > 0 {
                    ui.weak(format!("… {more} more"));
                }
            });
        }
        CallGraphAction::None
    }

    /// Drill-down: callers | functions | callees of one module.
    fn focus_ui(
        &mut self,
        ui: &mut Ui,
        project: &Project,
        crate_name: &str,
        node: usize,
    ) -> CallGraphAction {
        let (_, g, ..) = self.graph.as_ref().expect("computed by caller");
        let mut action = CallGraphAction::None;
        let mut new_focus: Option<Option<usize>> = None;
        let n = &g.nodes[node];
        ui.horizontal(|ui| {
            if ui.link("all modules").clicked() {
                new_focus = Some(None);
            }
            ui.label("›");
            ui.strong(RichText::new(&n.key).monospace());
            ui.weak(format!(
                "{} functions · {} lines",
                n.functions.len(),
                n.lines
            ));
        });
        ui.separator();
        let fn_label = |f: FnId| -> String {
            let q = &project.function(f).qual_path;
            q.strip_prefix(&format!("{crate_name}::"))
                .unwrap_or(q)
                .to_string()
        };
        // Calls by the module on the other side.
        let group = |incoming: bool| -> BTreeMap<usize, Vec<(FnId, FnId)>> {
            let mut m: BTreeMap<usize, Vec<(FnId, FnId)>> = BTreeMap::new();
            for e in &g.edges {
                if incoming && e.to == node {
                    m.entry(e.from).or_default().extend(e.calls.iter().copied());
                } else if !incoming && e.from == node {
                    m.entry(e.to).or_default().extend(e.calls.iter().copied());
                }
            }
            m
        };
        let incoming = group(true);
        let outgoing = group(false);
        let mut mid_action = None;
        ui.columns(3, |cols| {
            let mut side = |ui: &mut Ui,
                            title: &str,
                            color: Color32,
                            calls: &BTreeMap<usize, Vec<(FnId, FnId)>>,
                            incoming: bool,
                            salt: &str| {
                ui.label(RichText::new(title).strong().color(color));
                egui::ScrollArea::vertical()
                    .id_salt(salt)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        if calls.is_empty() {
                            ui.weak("(none)");
                        }
                        for (&other, list) in calls {
                            let header =
                                RichText::new(format!("{} ({})", g.nodes[other].key, list.len()))
                                    .monospace();
                            if ui.link(header).on_hover_text("show this module").clicked() {
                                new_focus = Some(Some(other));
                            }
                            ui.indent((salt, other), |ui| {
                                // Same pair called at several places: one line with a count.
                                let mut pairs: Vec<((FnId, FnId), usize)> = Vec::new();
                                for &pair in list {
                                    match pairs.iter_mut().find(|(p, _)| *p == pair) {
                                        Some((_, n)) => *n += 1,
                                        None => pairs.push((pair, 1)),
                                    }
                                }
                                for &((caller, callee), count) in pairs.iter().take(40) {
                                    let (outside, inside) = if incoming {
                                        (caller, callee)
                                    } else {
                                        (callee, caller)
                                    };
                                    ui.horizontal_wrapped(|ui| {
                                        if ui
                                            .link(RichText::new(fn_label(outside)).monospace())
                                            .clicked()
                                        {
                                            action = CallGraphAction::ShowFn(outside);
                                        }
                                        ui.weak(if incoming { "→" } else { "←" });
                                        ui.weak(
                                            RichText::new(&project.function(inside).name)
                                                .monospace(),
                                        );
                                        if count > 1 {
                                            ui.weak(format!("×{count}"));
                                        }
                                    });
                                }
                                if pairs.len() > 40 {
                                    ui.weak(format!("… {} more", pairs.len() - 40));
                                }
                            });
                        }
                    });
            };
            side(
                &mut cols[0],
                "called from",
                CALLS_IN,
                &incoming,
                true,
                "cg_in",
            );
            // Middle: the module's functions.
            cols[1].label(RichText::new(format!("functions in {}", n.key)).strong());
            egui::ScrollArea::vertical()
                .id_salt("cg_fns")
                .auto_shrink([false, false])
                .show(&mut cols[1], |ui| {
                    for &f in &n.functions {
                        let calls_in = incoming.values().flatten().filter(|(_, c)| *c == f).count();
                        let calls_out =
                            outgoing.values().flatten().filter(|(c, _)| *c == f).count();
                        ui.horizontal_wrapped(|ui| {
                            let label = fn_label(f);
                            let short = label
                                .strip_prefix(&format!(
                                    "{}::",
                                    n.key.strip_prefix(&format!("{crate_name}::")).unwrap_or("")
                                ))
                                .unwrap_or(&label);
                            if ui
                                .link(RichText::new(short).monospace())
                                .on_hover_text(&project.function(f).sig.text)
                                .clicked()
                            {
                                mid_action = Some(f);
                            }
                            if calls_in > 0 {
                                ui.label(RichText::new(format!("←{calls_in}")).color(CALLS_IN));
                            }
                            if calls_out > 0 {
                                ui.label(RichText::new(format!("→{calls_out}")).color(CALLS_OUT));
                            }
                            ui.weak(format!("{} lines", project.function(f).span.line_count()));
                        });
                    }
                });
            side(
                &mut cols[2],
                "calls into",
                CALLS_OUT,
                &outgoing,
                false,
                "cg_out",
            );
        });
        if let Some(f) = new_focus {
            self.focus = f;
            self.selected = f;
        }
        if let Some(f) = mid_action {
            action = CallGraphAction::ShowFn(f);
        }
        action
    }
}

fn node_lines(n: &rv_analysis::ModuleNode, crate_name: &str) -> Vec<(String, bool)> {
    let name = n
        .key
        .strip_prefix(&format!("{crate_name}::"))
        .unwrap_or(if n.key == crate_name {
            "(crate root)"
        } else {
            &n.key
        });
    let mut lines = vec![
        (name.to_string(), true),
        (
            format!("{} fns · {} lines", n.functions.len(), n.lines),
            false,
        ),
    ];
    if n.spawns > 0 {
        lines.push((format!("‖ starts threads ({})", n.spawns), false));
    }
    lines
}

fn node_size(lines: &[(String, bool)]) -> Vec2 {
    let chars = lines
        .iter()
        .map(|(t, _)| t.chars().count())
        .max()
        .unwrap_or(10);
    vec2(
        (chars as f32 * 7.6 + 2.0 * PAD).max(120.0),
        lines.len() as f32 * LINE + 2.0 * PAD,
    )
}

/// Bézier between two boxes: right side to left side when the target is to
/// the right, otherwise an arc around.
fn connect(a: Rect, b: Rect) -> [Pos2; 4] {
    if b.min.x > a.max.x {
        let (p0, p3) = (pos2(a.max.x, a.center().y), pos2(b.min.x, b.center().y));
        let dx = (p3.x - p0.x) * 0.5;
        [p0, p0 + vec2(dx, 0.0), p3 - vec2(dx, 0.0), p3]
    } else {
        // Back edge (cycle) or same column: leave below, come back above.
        let (p0, p3) = (pos2(a.center().x, a.max.y), pos2(b.center().x, b.min.y));
        let bend = 60.0;
        [p0, p0 + vec2(-bend, bend), p3 + vec2(-bend, -bend), p3]
    }
}

fn sample(p: &[Pos2; 4]) -> Vec<Pos2> {
    (0..=20)
        .map(|i| {
            let t = i as f32 / 20.0;
            let u = 1.0 - t;
            (p[0].to_vec2() * (u * u * u)
                + p[1].to_vec2() * (3.0 * u * u * t)
                + p[2].to_vec2() * (3.0 * u * t * t)
                + p[3].to_vec2() * (t * t * t))
                .to_pos2()
        })
        .collect()
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

impl CallGraphView {
    /// The module graph (as laid out) as SVG in light colors.
    fn svg(&self) -> Option<String> {
        let (key, g, l, sizes) = self.graph.as_ref()?;
        Some(module_svg(&key.crate_name, g, l, sizes))
    }
}

/// Module graph of a crate with its layered layout and box sizes.
pub(crate) fn module_layout(
    project: &Project,
    calls: &CallGraph,
    facts: &Facts,
    crate_name: &str,
    depth: usize,
) -> (ModuleGraph, LayeredLayout, Vec<Vec2>) {
    let g = rv_analysis::module_graph(project, calls, facts, crate_name, depth);
    let sizes: Vec<Vec2> = g
        .nodes
        .iter()
        .map(|n| node_size(&node_lines(n, crate_name)))
        .collect();
    let edges: Vec<(usize, usize)> = g.edges.iter().map(|e| (e.from, e.to)).collect();
    let l = layered(&sizes, &edges, vec2(90.0, 16.0));
    (g, l, sizes)
}

/// The module graph as SVG in light colors.
pub(crate) fn module_svg(
    crate_name: &str,
    g: &ModuleGraph,
    l: &LayeredLayout,
    sizes: &[Vec2],
) -> String {
    use crate::export::Svg;
    let box_of = |i: usize| Rect::from_min_size(l.positions[i], sizes[i]);
    let mut svg = Svg::new(l.size + vec2(80.0, 80.0));
    for e in &g.edges {
        let pts = sample(&connect(box_of(e.from), box_of(e.to)));
        let width = 1.0 + (e.calls.len() as f32).log2();
        svg.arrow(&pts, width, Color32::from_gray(150), None);
    }
    for (i, n) in g.nodes.iter().enumerate() {
        let (width, border) = if n.spawns > 0 {
            (2.0, THREADS)
        } else {
            (1.0, Color32::from_gray(90))
        };
        svg.rect(box_of(i), Color32::WHITE, Some((width, border)), 5.0);
        for (k, (text, strong)) in node_lines(n, crate_name).iter().enumerate() {
            let color = if *strong {
                Color32::from_gray(20)
            } else if text.starts_with('‖') {
                THREADS
            } else {
                Color32::from_gray(120)
            };
            let size = if *strong { 12.5 } else { 11.5 };
            svg.text(
                box_of(i).min + vec2(PAD, PAD + k as f32 * LINE),
                text,
                size,
                color,
                *strong,
                false,
            );
        }
    }
    svg.finish()
}
