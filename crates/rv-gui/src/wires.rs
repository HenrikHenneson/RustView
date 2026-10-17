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
    fn sample(&self) -> impl Iterator<Item = Pos2> + '_ {
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
    let vertical = orientation == Orientation::Vertical;
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
        let p3 = anchor(b, orientation);
        let mut p0 = anchor(a, orientation);
        if a.rect.contains_rect(b.rect) {
            // Parameters, pattern bindings of a case: start in the header
            // right above the target instead of at the container's far edge.
            p0 = if vertical {
                pos2(p3.x, a.rect.min.y + 11.0)
            } else {
                pos2(a.rect.min.x + 11.0, p3.y)
            };
        }
        // Clear every box between the two ends (but not the boxes that
        // contain an end).
        let (lo, hi, mut clear) = if vertical {
            (p0.y.min(p3.y), p0.y.max(p3.y), p0.x.max(p3.x))
        } else {
            (p0.x.min(p3.x), p0.x.max(p3.x), p0.y.max(p3.y))
        };
        for r in &obstacles {
            let (r_lo, r_hi, r_far) = if vertical {
                (r.min.y, r.max.y, r.max.x)
            } else {
                (r.min.x, r.max.x, r.max.y)
            };
            if r_hi > lo && r_lo < hi && !r.contains_rect(a.rect) && !r.contains_rect(b.rect) {
                clear = clear.max(r_far);
            }
        }
        // Longer wires go further out, so nested arcs stay apart; concurrency
        // wires run outside the data wires.
        let outer = if spec.kind.is_concurrency() {
            40.0
        } else {
            0.0
        };
        let target = clear + 10.0 + outer + (0.06 * (hi - lo)).min(60.0);
        // A cubic Bézier reaches 3/4 of the way to its control points.
        let near = if vertical {
            p0.x.min(p3.x)
        } else {
            p0.y.min(p3.y)
        };
        let c = (target - 0.25 * near) / 0.75;
        let points = if vertical {
            [p0, pos2(c, p0.y), pos2(c, p3.y), p3]
        } else {
            [p0, pos2(p0.x, c), pos2(p3.x, c), p3]
        };
        wires.push(Wire {
            spec: i,
            from: a.node,
            to: b.node,
            points,
            kind: spec.kind,
            certain: spec.certain,
            name: spec.name.clone(),
        });
    }
    wires
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

pub struct WireStyle {
    pub dark: bool,
    pub zoom: f32,
    pub origin: Pos2,
}

/// Color of a wire, stable across frames (data wires: per variable name).
pub fn color(name: &str, kind: WireKind, dark: bool) -> Color32 {
    let fixed = match kind {
        WireKind::Mutation => Some(Color32::from_rgb(225, 80, 40)),
        WireKind::PossibleMutation => Some(Color32::from_rgb(230, 140, 30)),
        WireKind::Channel => Some(Color32::from_rgb(0, 150, 165)),
        WireKind::Shared => Some(Color32::from_rgb(150, 90, 210)),
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

pub fn paint(
    painter: &Painter,
    wire: &Wire,
    style: &WireStyle,
    alpha: f32,
    highlighted: bool,
    clip: Rect,
) {
    let to_screen = |p: Pos2| style.origin + p.to_vec2() * style.zoom;
    let pts: Vec<Pos2> = wire.sample().map(to_screen).collect();
    let bounds = Rect::from_points(&pts);
    if !bounds.expand(4.0).intersects(clip) {
        return;
    }
    let color = color(&wire.name, wire.kind, style.dark).gamma_multiply(alpha);
    let mut width = match wire.kind {
        WireKind::Data | WireKind::PossibleMutation => 1.4,
        WireKind::Mutation => 1.6,
        WireKind::Shared => 2.0,
        WireKind::Channel => 3.0,
    };
    if highlighted {
        width += 1.5;
    }
    let stroke = Stroke::new(width, color);
    match wire.kind {
        WireKind::Shared => painter.extend(Shape::dashed_line(&pts, stroke, 2.0, 4.0)),
        _ if !wire.certain || wire.kind == WireKind::PossibleMutation => {
            painter.extend(Shape::dashed_line(&pts, stroke, 6.0, 4.0))
        }
        _ => {
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
