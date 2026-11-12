//! Wires: geometry, hit testing and painting.
//!
//! Wires are drawn as arcs beside the statement column (right of it when
//! statements run top-down, below it when they run left-right), so they do
//! not cross the statement boxes. Ends inside collapsed structures attach to
//! the nearest visible ancestor. Channel and shared-state wires run further
//! out than data wires.

use std::collections::HashSet;

use egui::{Color32, Painter, Pos2, Rect, Shape, Stroke, pos2, vec2};
use rv_ir::{Body, NodeId};
use rv_layout::{Item, ItemKind, Layout, Orientation};

/// `Auto` shows all data wires up to this many.
pub const AUTO_LIMIT: usize = 40;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WireMode {
    /// All data wires for small diagrams, otherwise like `Selected`.
    #[default]
    Auto,
    All,
    /// Only data wires of the selected (or hovered) node.
    Selected,
    Off,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum WireKind {
    Data,
    Mutation,
    PossibleMutation,
    /// Between the send and receive ends of a channel.
    Channel,
    /// Between places that use the same shared state from different lanes.
    Shared,
    /// Value carried into the next loop iteration (shift register).
    LoopBack,
}

impl WireKind {
    /// Concurrency wires are always shown and ignore the data wire mode.
    pub fn is_concurrency(self) -> bool {
        matches!(self, WireKind::Channel | WireKind::Shared)
    }
}

/// What to connect; built by the diagram from data flow and concurrency facts.
#[derive(Clone, Debug)]
pub struct WireSpec {
    pub from: NodeId,
    pub to: NodeId,
    pub kind: WireKind,
    pub certain: bool,
    /// Variable, channel or shared state name.
    pub name: String,
    /// Index into the source list (data edge, channel or shared group).
    pub source: usize,
}

pub struct Wire {
    /// Index into the specs.
    pub spec: usize,
    /// Visible items the wire attaches to.
    pub from: NodeId,
    pub to: NodeId,
    /// Cubic Bézier control points in diagram coordinates.
    pub points: [Pos2; 4],
    pub kind: WireKind,
    pub certain: bool,
    pub name: String,
}

const SAMPLES: usize = 20;

impl Wire {
    pub fn sample(&self) -> impl Iterator<Item = Pos2> + '_ {
        let [p0, p1, p2, p3] = self.points;
        (0..=SAMPLES).map(move |i| {
            let t = i as f32 / SAMPLES as f32;
            let u = 1.0 - t;
            let v = p0.to_vec2() * (u * u * u)
                + p1.to_vec2() * (3.0 * u * u * t)
                + p2.to_vec2() * (3.0 * u * t * t)
                + p3.to_vec2() * (t * t * t);
            v.to_pos2()
        })
    }

    pub fn touches(&self, node: NodeId, specs: &[WireSpec]) -> bool {
        let s = &specs[self.spec];
        self.from == node || self.to == node || s.from == node || s.to == node
    }
}

/// Visible item representing `node`: itself or the closest visible ancestor.
fn visible<'l>(body: &Body, layout: &'l Layout, mut node: NodeId) -> Option<&'l Item> {
    loop {
        if let Some(item) = layout.item(node) {
            return Some(item);
        }
        node = body.node(node).parent?;
    }
}

fn anchor(item: &Item, orientation: Orientation) -> Pos2 {
    let r = item.rect;
    match orientation {
        Orientation::Vertical => pos2(r.max.x, r.min.y + (r.height() / 2.0).min(11.0)),
        Orientation::Horizontal => pos2(r.min.x + (r.width() / 2.0).min(40.0), r.max.y),
    }
}

/// The statement direction of a diagram and the direction across it, in
/// which the wires bulge out.
#[derive(Clone, Copy)]
struct Axes {
    vertical: bool,
}

impl Axes {
    /// Coordinate in statement direction.
    fn along(self, p: Pos2) -> f32 {
        if self.vertical { p.y } else { p.x }
    }

    /// Coordinate in the direction wires bulge out.
    fn across(self, p: Pos2) -> f32 {
        if self.vertical { p.x } else { p.y }
    }

    fn point(self, along: f32, across: f32) -> Pos2 {
        if self.vertical {
            pos2(across, along)
        } else {
            pos2(along, across)
        }
    }
}

/// The wires for `specs` between the items that are visible in `layout`.
pub fn geometry(
    body: &Body,
    specs: &[WireSpec],
    layout: &Layout,
    orientation: Orientation,
) -> Vec<Wire> {
    // Boxes a wire must pass beside (frames are only backgrounds).
    let obstacles: Vec<Rect> = layout
        .items
        .iter()
        .filter(|i| !matches!(i.kind, ItemKind::Frame { .. }))
        .map(|i| i.rect)
        .collect();
    let mut seen = HashSet::new();
    let mut wires = Vec::new();
    for (i, spec) in specs.iter().enumerate() {
        let (Some(a), Some(b)) = (
            visible(body, layout, spec.from),
            visible(body, layout, spec.to),
        ) else {
            continue;
        };
        if a.node == b.node || !seen.insert((a.node, b.node, spec.name.clone(), spec.kind)) {
            continue;
        }
        wires.push(Wire {
            spec: i,
            from: a.node,
            to: b.node,
            points: route(a, b, spec.kind, &obstacles, orientation),
            kind: spec.kind,
            certain: spec.certain,
            name: spec.name.clone(),
        });
    }
    wires
}

/// Control points of the arc from item `a` to item `b` that passes beside
/// all `obstacles` between them.
fn route(
    a: &Item,
    b: &Item,
    kind: WireKind,
    obstacles: &[Rect],
    orientation: Orientation,
) -> [Pos2; 4] {
    let axes = Axes {
        vertical: orientation == Orientation::Vertical,
    };
    let p3 = anchor(b, orientation);
    let mut p0 = anchor(a, orientation);
    if a.rect.contains_rect(b.rect) {
        // Parameters, pattern bindings of a case: start in the header
        // right above the target instead of at the container's far edge.
        p0 = axes.point(axes.along(a.rect.min) + 11.0, axes.across(p3));
    }
    // Clear every box between the two ends (but not the boxes that
    // contain an end).
    let lo = axes.along(p0).min(axes.along(p3));
    let hi = axes.along(p0).max(axes.along(p3));
    let mut clear = axes.across(p0).max(axes.across(p3));
    for r in obstacles {
        let between = axes.along(r.max) > lo && axes.along(r.min) < hi;
        if between && !r.contains_rect(a.rect) && !r.contains_rect(b.rect) {
            clear = clear.max(axes.across(r.max));
        }
    }
    // Longer wires go further out, so nested arcs stay apart; concurrency
    // wires run outside the data wires.
    let outer = if kind.is_concurrency() { 40.0 } else { 0.0 };
    let target = clear + 10.0 + outer + (0.06 * (hi - lo)).min(60.0);
    // A cubic Bézier reaches 3/4 of the way to its control points.
    let near = axes.across(p0).min(axes.across(p3));
    let c = (target - 0.25 * near) / 0.75;
    [
        p0,
        axes.point(axes.along(p0), c),
        axes.point(axes.along(p3), c),
        p3,
    ]
}

/// Wire under `pos` (diagram coordinates) within `tolerance`.
pub fn hit(
    wires: &[Wire],
    visible: impl Fn(&Wire) -> bool,
    pos: Pos2,
    tolerance: f32,
) -> Option<usize> {
    let mut best: Option<(usize, f32)> = None;
    for (i, w) in wires.iter().enumerate().filter(|(_, w)| visible(w)) {
        let pts: Vec<Pos2> = w.sample().collect();
        for seg in pts.windows(2) {
            let d = distance_to_segment(pos, seg[0], seg[1]);
            if d <= tolerance && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
    }
    best.map(|(i, _)| i)
}

fn distance_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = if ab.length_sq() > 0.0 {
        ((p - a).dot(ab) / ab.length_sq()).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (p - (a + ab * t)).length()
}

/// How wires are drawn in one frame.
pub struct WireStyle {
    pub dark: bool,
    pub zoom: f32,
    /// Screen position of the diagram's origin.
    pub origin: Pos2,
    /// Visible part of the screen; wires outside of it are skipped.
    pub clip: Rect,
}

/// Color of a wire, stable across frames (data wires: per variable name).
pub fn color(name: &str, kind: WireKind, dark: bool) -> Color32 {
    let fixed = match kind {
        WireKind::Mutation => Some(Color32::from_rgb(225, 80, 40)),
        WireKind::PossibleMutation => Some(Color32::from_rgb(230, 140, 30)),
        WireKind::Channel => Some(Color32::from_rgb(0, 150, 165)),
        WireKind::Shared => Some(Color32::from_rgb(150, 90, 210)),
        WireKind::LoopBack => Some(Color32::from_rgb(70, 130, 220)),
        WireKind::Data => None,
    };
    if let Some(c) = fixed {
        return c;
    }
    const PALETTE: [Color32; 8] = [
        Color32::from_rgb(40, 100, 200),
        Color32::from_rgb(30, 140, 70),
        Color32::from_rgb(130, 70, 190),
        Color32::from_rgb(0, 140, 150),
        Color32::from_rgb(150, 100, 40),
        Color32::from_rgb(190, 60, 140),
        Color32::from_rgb(100, 120, 20),
        Color32::from_rgb(60, 90, 130),
    ];
    let hash = name
        .bytes()
        .fold(5381u32, |h, b| h.wrapping_mul(33) ^ u32::from(b));
    let c = PALETTE[hash as usize % PALETTE.len()];
    if dark {
        // Lighter variants on dark backgrounds.
        Color32::from_rgb(
            c.r().saturating_add(70),
            c.g().saturating_add(70),
            c.b().saturating_add(70),
        )
    } else {
        c
    }
}

/// Line width and dash pattern (dash, gap) of a wire on screen.
fn stroke_pattern(wire: &Wire) -> (f32, Option<(f32, f32)>) {
    let width = match wire.kind {
        WireKind::Data | WireKind::PossibleMutation => 1.4,
        WireKind::Mutation => 1.6,
        WireKind::Shared | WireKind::LoopBack => 2.0,
        WireKind::Channel => 3.0,
    };
    let dash = match wire.kind {
        WireKind::Shared => Some((2.0, 4.0)),
        WireKind::LoopBack => Some((9.0, 4.0)),
        _ if !wire.certain || wire.kind == WireKind::PossibleMutation => Some((6.0, 4.0)),
        _ => None,
    };
    (width, dash)
}

/// Paints `wire` with opacity `alpha`; a `highlighted` wire is drawn thicker.
pub fn paint(painter: &Painter, wire: &Wire, style: &WireStyle, alpha: f32, highlighted: bool) {
    let to_screen = |p: Pos2| style.origin + p.to_vec2() * style.zoom;
    // A Bézier curve stays within the hull of its control points, so wires
    // far off screen are skipped without sampling them (one unit more than
    // below, so rounding never skips a wire the exact test would keep).
    let hull = Rect::from_points(&wire.points.map(to_screen));
    if !hull.expand(5.0).intersects(style.clip) {
        return;
    }
    let pts: Vec<Pos2> = wire.sample().map(to_screen).collect();
    let bounds = Rect::from_points(&pts);
    if !bounds.expand(4.0).intersects(style.clip) {
        return;
    }
    let color = color(&wire.name, wire.kind, style.dark).gamma_multiply(alpha);
    let (mut width, dash) = stroke_pattern(wire);
    if highlighted {
        width += 1.5;
    }
    let stroke = Stroke::new(width, color);
    match dash {
        Some((dash, gap)) => painter.extend(Shape::dashed_line(&pts, stroke, dash, gap)),
        None => {
            painter.add(Shape::line(pts.clone(), stroke));
        }
    }
    // Arrow head at the receiving end (shared state has no direction).
    if wire.kind == WireKind::Shared {
        return;
    }
    let n = pts.len();
    let dir = (pts[n - 1] - pts[n - 2]).normalized();
    if dir.is_finite() {
        let size = 5.0 + width;
        let tip = pts[n - 1];
        let side = vec2(-dir.y, dir.x) * (size * 0.5);
        let back = tip - dir * size;
        painter.add(Shape::convex_polygon(
            vec![tip, back + side, back - side],
            color,
            Stroke::NONE,
        ));
    }
}
