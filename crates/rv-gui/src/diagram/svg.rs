//! The function diagram as SVG in light colors: the whole scene, drawn in
//! the same order as on screen but without the interactive controls.

use egui::{Color32, Rect, vec2};
use rv_ir::{FnId, NodeId, NodeKind, Project, StmtKind, StructKind};
use rv_layout::{Item, ItemKind, Metrics, Orientation, StructItem, Text, ViewState};

use super::analysis::Analysis;
use super::fn_view::BUDGET;
use super::palette::{
    EXPR_RADIUS, EXPR_WIRE, FONT_SIZE, HEADER_TINT, ITEM_RADIUS, LANE_BAR, LANE_OUTLINE,
    MAX_LABEL_CHARS, OUTLINE, Palette, expr_color, structure_border,
};
use super::scene::{self, Scene, WireFilter};
use crate::export::{Svg, TextStyle};
use crate::wires::{self, Wire, WireKind, WireMode};

/// Space kept around wires that run outside of the boxes.
const WIRE_MARGIN: f32 = 12.0;
/// Size of an exit marker: how far it reaches back from its tip and to
/// each side.
const EXIT_MARK: egui::Vec2 = vec2(10.0, 5.0);

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
    let view = rv_layout::View {
        state: &state,
        defaults: &defaults,
        orientation: Orientation::Vertical,
    };
    let layout = rv_layout::layout(body, view, metrics, &analysis.notes);
    let wire_list = wires::geometry(body, &analysis.specs, &layout, Orientation::Vertical);
    let exit_marks = scene::exit_marks(body, &layout);
    let scene = Scene {
        body,
        layout: &layout,
        analysis: &analysis,
        wires: &wire_list,
        exit_marks: &exit_marks,
    };
    Some(diagram_svg(&scene, WireMode::Auto, None))
}

/// The scene as SVG, with the data wires the view shows in `mode` for the
/// `selected` node (without hovering).
pub(super) fn diagram_svg(scene: &Scene, mode: WireMode, selected: Option<NodeId>) -> String {
    let filter = WireFilter::new(*scene, mode, selected);
    let shown: Vec<&Wire> = scene.wires.iter().filter(|w| filter.shows(w)).collect();
    // Wires run outside of the boxes: make room for them.
    let mut size = scene.layout.size;
    for w in &shown {
        for p in w.points {
            size = size.max(p.to_vec2() + vec2(WIRE_MARGIN, WIRE_MARGIN));
        }
    }
    let mut out = Export {
        svg: Svg::new(size),
        style: Palette::new(&egui::Visuals::light()),
        scene,
    };
    out.items();
    out.exit_marks();
    out.expressions();
    for w in shown {
        out.wire(w);
    }
    out.svg.finish()
}

/// Draws a scene into an SVG.
struct Export<'a> {
    svg: Svg,
    style: Palette,
    scene: &'a Scene<'a>,
}

impl Export<'_> {
    /// A label, fitted to the width the layout measured for it.
    fn text(&mut self, t: &Text, color: Color32) {
        let style = TextStyle::new(FONT_SIZE, color, true, false);
        self.svg
            .label_fit(t.rect.min, &t.text, style, t.rect.width());
    }

    fn items(&mut self) {
        let scene = self.scene;
        for item in &scene.layout.items {
            match (&item.kind, &scene.body.node(item.node).kind) {
                (ItemKind::Structure(s), NodeKind::Structure { kind, .. }) => {
                    self.structure(item, s, kind);
                }
                (ItemKind::Frame { title }, NodeKind::Frame { .. }) => {
                    self.frame(item, title.as_ref());
                }
                (ItemKind::Stmt { .. }, NodeKind::Stmt { kind }) => self.stmt(item, kind),
                _ => {}
            }
        }
    }

    fn structure(&mut self, item: &Item, s: &StructItem, kind: &StructKind) {
        let r = item.rect;
        let color = self.style.structure(kind);
        let border = Some((structure_border(kind), color));
        self.svg
            .rect(r, self.style.frame_bg(item.depth), border, ITEM_RADIUS);
        self.svg.rect(
            s.header,
            color.gamma_multiply(HEADER_TINT),
            None,
            ITEM_RADIUS,
        );
        if let Some(&lane) = self.scene.analysis.lane_color.get(&item.node) {
            let outline = Some((LANE_OUTLINE, lane));
            self.svg
                .rect(r.expand(3.0), Color32::TRANSPARENT, outline, 6.0);
        }
        if let Some(p) = &s.prefix {
            self.text(p, self.style.weak);
        }
        self.text(&s.title, self.style.text);
        if let Some(n) = &s.note {
            self.text(n, self.scene.note_color(item.node, &self.style));
        }
        if let Some(i) = &s.info {
            self.text(i, self.style.info(&i.text));
        }
        for terminal in self.scene.register_terminals(item, s) {
            self.svg.rect(terminal, self.style.note(), None, 1.0);
        }
    }

    fn frame(&mut self, item: &Item, title: Option<&Text>) {
        let line = Some((OUTLINE, self.style.frame_line));
        self.svg.rect(
            item.rect,
            self.style.frame_bg(item.depth + 1),
            line,
            ITEM_RADIUS,
        );
        if let Some(t) = title {
            self.text(t, self.style.case_title);
        }
    }

    fn stmt(&mut self, item: &Item, kind: &StmtKind) {
        let ItemKind::Stmt { text, badges, note } = &item.kind else {
            return;
        };
        let r = item.rect;
        let line = Some((OUTLINE, self.style.stmt_line(kind)));
        self.svg.rect(r, self.style.stmt(kind), line, ITEM_RADIUS);
        self.text(text, self.style.text);
        if let Some(b) = badges {
            self.text(b, self.style.exit);
        }
        // A statement that starts a lane gets a bar and its note in the lane
        // color; other notes are left out here.
        if let Some(&lane) = self.scene.analysis.lane_color.get(&item.node) {
            let bar = Rect::from_min_size(r.min, vec2(LANE_BAR, r.height()));
            self.svg.rect(bar, lane, None, 0.0);
            if let Some(n) = note {
                self.text(n, lane);
            }
        }
    }

    /// Exit markers where control leaves a structure.
    fn exit_marks(&mut self) {
        for &tip in self.scene.exit_marks {
            let corners = [
                tip,
                tip + vec2(-EXIT_MARK.x, -EXIT_MARK.y),
                tip + vec2(-EXIT_MARK.x, EXIT_MARK.y),
            ];
            self.svg.polygon(&corners, self.style.exit);
        }
    }

    /// Expression trees: wires from operands to operations, then the boxes.
    fn expressions(&mut self) {
        let scene = self.scene;
        for &(a, b) in &scene.layout.expr_wires {
            self.svg.arrow(&[a, b], EXPR_WIRE, self.style.weak, None);
        }
        for e in &scene.layout.exprs {
            let n = scene.expr_node(e);
            let base = expr_color(n.kind, &n.text, self.style.dark, scene.body);
            let line = Some((OUTLINE, base));
            self.svg
                .rect(e.rect, self.style.expr_fill(base), line, EXPR_RADIUS);
            self.text(&e.text, self.style.text);
        }
    }

    fn wire(&mut self, w: &Wire) {
        let pts = w.sample().collect::<Vec<_>>();
        let color = wires::color(&w.name, w.kind, self.style.dark);
        let (width, dash) = stroke_pattern(w);
        // Shared state has no direction.
        if w.kind == WireKind::Shared {
            self.svg.polyline(&pts, width, color, dash);
        } else {
            self.svg.arrow(&pts, width, color, dash);
        }
    }
}

/// Line width and dash pattern (dash, gap) of a wire in the export.
fn stroke_pattern(w: &Wire) -> (f32, Option<(f32, f32)>) {
    match w.kind {
        WireKind::Channel => (3.0, None),
        WireKind::Shared => (2.0, Some((2.0, 4.0))),
        WireKind::LoopBack => (2.0, Some((9.0, 4.0))),
        WireKind::PossibleMutation => (1.4, Some((6.0, 4.0))),
        _ if !w.certain => (1.4, Some((6.0, 4.0))),
        WireKind::Mutation => (1.6, None),
        WireKind::Data => (1.4, None),
    }
}
